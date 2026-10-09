//! Crash and stall diagnostics inside GTA's process.
//!
//! GTA's own unhandled-exception filter ends the process without a Windows
//! error report, and a Rust abort (a panic that cannot unwind) leaves
//! nothing in the log. This module records, in the runtime log's folder:
//! - every panic, synchronously, with its thread and backtrace (the panic
//!   hook runs before any abort);
//! - first-chance fatal exceptions (vectored handler, first in line): code,
//!   address as module + offset, thread; a minidump for the first few;
//! - a watchdog: a runtime thread busy in one stage for over 4 s, and the
//!   private heap growing past 1 GB (with a minidump, once).
#![cfg(windows)]

use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    record: *mut ExceptionRecord,
    address: *mut c_void,
    count: u32,
    info: [usize; 15],
}

#[repr(C)]
struct ExceptionPointers {
    record: *mut ExceptionRecord,
    context: *mut c_void,
}

#[repr(C, packed(4))]
struct DumpException {
    thread: u32,
    pointers: *mut ExceptionPointers,
    client: i32,
}

#[repr(C)]
#[derive(Default)]
struct MemoryCounters {
    cb: u32,
    page_faults: u32,
    peak_working_set: usize,
    working_set: usize,
    quota_peak_paged: usize,
    quota_paged: usize,
    quota_peak_nonpaged: usize,
    quota_nonpaged: usize,
    pagefile: usize,
    peak_pagefile: usize,
    private: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: unsafe extern "system" fn(*mut ExceptionPointers) -> i32) -> *mut c_void;
    fn GetModuleHandleExW(flags: u32, address: *const u16, module: *mut isize) -> i32;
    fn GetModuleFileNameW(module: isize, name: *mut u16, size: u32) -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentProcess() -> isize;
    fn GetCurrentProcessId() -> u32;
    fn K32GetProcessMemoryInfo(process: isize, counters: *mut MemoryCounters, cb: u32) -> i32;
    fn CreateEventW(attributes: *const c_void, manual: i32, initial: i32, name: *const u16) -> isize;
    fn SetEvent(event: isize) -> i32;
    fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
}

#[link(name = "dbghelp")]
unsafe extern "system" {
    fn MiniDumpWriteDump(
        process: isize,
        pid: u32,
        file: *mut c_void,
        kind: u32,
        exception: *const DumpException,
        user: *const c_void,
        callback: *const c_void,
    ) -> i32;
}

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
/// MiniDumpWithDataSegs | WithHandleData | WithIndirectlyReferencedMemory |
/// WithProcessThreadData | WithThreadInfo | WithUnloadedModules.
const DUMP_KIND: u32 = 0x1 | 0x4 | 0x40 | 0x100 | 0x1000 | 0x20;

struct State {
    dir: PathBuf,
    log: Mutex<Option<File>>,
    start: Instant,
}

static STATE: OnceLock<State> = OnceLock::new();
static LINES: AtomicUsize = AtomicUsize::new(0);
static DUMPS: AtomicUsize = AtomicUsize::new(0);
static OWN_BASE: AtomicUsize = AtomicUsize::new(0);

/// Runtime thread stages the watchdog watches: a stage code and when it
/// was entered (ms since install; 0 = idle).
pub struct Stage {
    pub name: &'static str,
    code: AtomicU32,
    since: AtomicU64,
}

impl Stage {
    const fn new(name: &'static str) -> Stage {
        Stage { name, code: AtomicU32::new(0), since: AtomicU64::new(0) }
    }
    pub fn enter(&self, code: u32) {
        self.code.store(code, Ordering::Relaxed);
        self.since.store(now_ms().max(1), Ordering::Relaxed);
    }
    pub fn leave(&self) {
        self.since.store(0, Ordering::Relaxed);
    }
}

pub static AUDIO: Stage = Stage::new("skatev-audio");
pub static WORKER: Stage = Stage::new("skatev-skate");

fn now_ms() -> u64 {
    STATE.get().map_or(0, |s| s.start.elapsed().as_millis() as u64)
}

fn write_line(text: &str) {
    let Some(s) = STATE.get() else { return };
    if let Ok(mut f) = s.log.try_lock()
        && let Some(f) = f.as_mut()
    {
        let _ = writeln!(f, "[{:>9.3}] {text}", s.start.elapsed().as_secs_f64());
        let _ = f.flush();
    }
}

fn module_of(address: usize) -> (usize, String) {
    let mut base = 0isize;
    // FROM_ADDRESS | UNCHANGED_REFCOUNT
    if unsafe { GetModuleHandleExW(0x4 | 0x2, address as *const u16, &mut base) } == 0 {
        return (0, "?".into());
    }
    let mut name = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(base, name.as_mut_ptr(), name.len() as u32) } as usize;
    let full = String::from_utf16_lossy(&name[..n.min(name.len())]);
    let short = full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string();
    (base as usize, short)
}

fn dump(tag: &str, thread: u32, pointers: usize) {
    let Some(s) = STATE.get() else { return };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let path = s.dir.join(format!("SkateVRuntime-{stamp}-{}-{tag}.dmp", DUMPS.load(Ordering::Relaxed)));
    let Ok(file) = File::create(&path) else { return };
    let info = DumpException { thread, pointers: pointers as *mut ExceptionPointers, client: 0 };
    let ok = unsafe {
        MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            file.as_raw_handle(),
            DUMP_KIND,
            if pointers != 0 { &info } else { std::ptr::null() },
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    write_line(&format!("crash: minidump {} {}", path.display(), if ok != 0 { "written" } else { "FAILED" }));
}

fn fatal(code: u32) -> Option<&'static str> {
    Some(match code {
        0xC000_0005 => "access violation",
        0xC000_00FD => "stack overflow",
        0xC000_001D => "illegal instruction",
        0xC000_0094 => "integer divide by zero",
        0xC000_0096 => "privileged instruction",
        0xC000_0374 => "heap corruption",
        0xC000_0409 => "fail fast",
        _ => return None,
    })
}

/// The pending exception, handed to the reporter thread (the faulting
/// thread's own stack may be spent).
static BUSY: AtomicBool = AtomicBool::new(false);
static P_CODE: AtomicU32 = AtomicU32::new(0);
static P_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static P_ACCESS: [AtomicUsize; 3] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];
static P_THREAD: AtomicU32 = AtomicU32::new(0);
static P_POINTERS: AtomicUsize = AtomicUsize::new(0);
static REQUEST: AtomicIsize = AtomicIsize::new(0);
static DONE: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn on_exception(p: *mut ExceptionPointers) -> i32 {
    let Some(rec) = (unsafe { p.as_ref() }).and_then(|p| unsafe { p.record.as_ref() }) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    if fatal(rec.code).is_none() || LINES.fetch_add(1, Ordering::Relaxed) >= 60 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let (request, done) = (REQUEST.load(Ordering::Acquire), DONE.load(Ordering::Acquire));
    if request == 0 || done == 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    while BUSY.swap(true, Ordering::Acquire) {
        std::hint::spin_loop();
    }
    P_CODE.store(rec.code, Ordering::Relaxed);
    P_ADDRESS.store(rec.address as usize, Ordering::Relaxed);
    let access = rec.code == 0xC000_0005 && rec.count >= 2;
    P_ACCESS[0].store(access as usize, Ordering::Relaxed);
    P_ACCESS[1].store(rec.info[0], Ordering::Relaxed);
    P_ACCESS[2].store(rec.info[1], Ordering::Relaxed);
    P_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    P_POINTERS.store(p as usize, Ordering::Release);
    unsafe {
        SetEvent(request);
        WaitForSingleObject(done, 15_000);
    }
    BUSY.store(false, Ordering::Release);
    EXCEPTION_CONTINUE_SEARCH
}

fn reporter() {
    let (request, done) = (REQUEST.load(Ordering::Acquire), DONE.load(Ordering::Acquire));
    loop {
        unsafe { WaitForSingleObject(request, u32::MAX) };
        let code = P_CODE.load(Ordering::Relaxed);
        let address = P_ADDRESS.load(Ordering::Relaxed);
        let thread = P_THREAD.load(Ordering::Relaxed);
        let pointers = P_POINTERS.load(Ordering::Acquire);
        let (base, module) = module_of(address);
        let own = base != 0 && base == OWN_BASE.load(Ordering::Relaxed);
        let access = if P_ACCESS[0].load(Ordering::Relaxed) != 0 {
            let kind = match P_ACCESS[1].load(Ordering::Relaxed) { 0 => "reading", 1 => "writing", _ => "executing" };
            format!(", {kind} {:#x}", P_ACCESS[2].load(Ordering::Relaxed))
        } else {
            String::new()
        };
        write_line(&format!(
            "crash: first-chance {} ({code:#010x}) at {module}+{:#x} ({address:#x}) on thread {thread}{access}",
            fatal(code).unwrap_or("?"),
            address - base
        ));
        // Our own module, any stack overflow, and the first two elsewhere.
        let n = DUMPS.load(Ordering::Relaxed);
        if (own || code == 0xC000_00FD || n < 2) && n < 4 {
            DUMPS.fetch_add(1, Ordering::Relaxed);
            dump(if own { "runtime" } else { "game" }, thread, pointers);
        }
        unsafe { SetEvent(done) };
    }
}

fn private_bytes() -> usize {
    let mut m = MemoryCounters { cb: std::mem::size_of::<MemoryCounters>() as u32, ..Default::default() };
    if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb) } == 0 {
        return 0;
    }
    m.private
}

fn watchdog() {
    let mut reported = [false; 2];
    let mut stall_dumped = false;
    let base_private = private_bytes();
    let mut next_memory = base_private + (1 << 30);
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let now = now_ms();
        for (k, stage) in [&AUDIO, &WORKER].into_iter().enumerate() {
            let since = stage.since.load(Ordering::Relaxed);
            let busy = if since == 0 { 0 } else { now.saturating_sub(since) };
            if busy > 4000 && !reported[k] {
                reported[k] = true;
                write_line(&format!("crash: {} busy {busy} ms in stage {}", stage.name, stage.code.load(Ordering::Relaxed)));
                if !stall_dumped {
                    stall_dumped = true;
                    dump("stall", 0, 0);
                }
            } else if busy == 0 && reported[k] {
                reported[k] = false;
                write_line(&format!("crash: {} running again", stage.name));
            }
        }
        let private = private_bytes();
        if private > next_memory {
            write_line(&format!("crash: process private bytes {} MB (at start {} MB)", private >> 20, base_private >> 20));
            next_memory = private + (1 << 30);
        }
    }
}

/// Once per process: the panic hook, the exception handler and the
/// watchdog, logging to `<log dir>/SkateVRuntime-crash.log`.
pub fn install(log_path: Option<&std::path::Path>) {
    let dir = log_path.and_then(|p| p.parent()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let file = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("SkateVRuntime-crash.log")).ok();
    if STATE.set(State { dir, log: Mutex::new(file), start: Instant::now() }).is_err() {
        return;
    }
    OWN_BASE.store(module_of(install as *const () as usize).0, Ordering::Relaxed);
    write_line(&format!("crash: diagnostics armed, private bytes {} MB", private_bytes() >> 20));
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let bt = std::backtrace::Backtrace::force_capture();
        write_line(&format!("crash: panic on thread '{}': {info}\n{bt}", thread.name().unwrap_or("?")));
        previous(info);
    }));
    let (request, done) = unsafe { (CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()), CreateEventW(std::ptr::null(), 0, 0, std::ptr::null())) };
    if request != 0 && done != 0 {
        REQUEST.store(request, Ordering::Release);
        DONE.store(done, Ordering::Release);
        let _ = std::thread::Builder::new().name("skatev-crash".into()).spawn(reporter);
        unsafe { AddVectoredExceptionHandler(1, on_exception) };
    }
    let _ = std::thread::Builder::new().name("skatev-watchdog".into()).spawn(watchdog);
}

#[cfg(test)]
mod tests {
    /// Run explicitly (it ends the test process with a real fault):
    /// `cargo test -p skatev-runtime crash::tests -- --ignored --nocapture`
    /// with SKATEV_CRASH_TEST=av|panic|overflow and SKATEV_CRASH_DIR.
    #[test]
    #[ignore]
    fn faults_are_recorded() {
        let dir = std::path::PathBuf::from(std::env::var("SKATEV_CRASH_DIR").unwrap());
        super::install(Some(&dir.join("x.log")));
        match std::env::var("SKATEV_CRASH_TEST").as_deref() {
            Ok("av") => unsafe {
                std::ptr::read_volatile(0x10 as *const u8);
            },
            Ok("overflow") => {
                fn deep(n: u64) -> u64 {
                    // Never true in practice: the stack overflows long before.
                    if std::hint::black_box(n) == u64::MAX {
                        return 0;
                    }
                    let a = [n; 512];
                    std::hint::black_box(&a);
                    deep(n + 1) + a[3]
                }
                std::thread::Builder::new().name("deep".into()).stack_size(256 << 10).spawn(|| deep(0)).unwrap().join().ok();
            }
            _ => {
                let _ = std::thread::spawn(|| -> u32 { let v: Vec<u32> = Vec::new(); v[3] }).join();
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
}

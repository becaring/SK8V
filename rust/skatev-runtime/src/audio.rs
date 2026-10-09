//! Skate 3 gameplay audio, dry (ABI 8 `audio` block).
//!
//! The Skate worker publishes the audio-relevant skater state after every
//! Skate tick (`publish`); a dedicated audio thread runs Skate 3's AEMS
//! sound scripts from it (`skate_aems::live::Engine`: the class handlers,
//! the AEMS world at its 30 Hz control rate and every voice through EA
//! Audio Core's dry chain) and renders 256-frame blocks at 48 kHz, paced by
//! the wall clock, into one ring buffer per emitter. The host pulls from
//! the rings (`sv_audio_pull`) and places them in GTA's audio engine. The
//! audio thread never touches the Skate session and the worker never waits
//! on audio: the hand-off is a bounded ordered queue written once per tick.
//!
//! Contract: `host/include/skatev_runtime.h`, audio block.

use std::collections::VecDeque;
use std::ffi::c_char;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bevy_math::Vec3;
use skate_aems::live::{self, EMITTERS};

use crate::SvVec3;

/// Skate session state -> the sound handlers' game variables.
#[path = "audio_map.rs"]
pub mod map;

pub const SAMPLE_RATE: u32 = 48_000;
/// Render lead over the wall clock (43 ms). The host keeps its own ~50 ms
/// buffer in front of GTA (`host/src/gta_audio.cpp`), so this only covers
/// render jitter.
pub const LATENCY_FRAMES: usize = 2048;
/// A ring holding more than the lead plus this (the host started pulling
/// after the render clock, or the clocks drifted) is trimmed back to the
/// lead: queued audio is latency, measured at 165 ms before this cap.
const RING_SLACK: usize = 1024;
const BLOCK: usize = 256;
/// Gameplay frames that may wait for the audio clock; older ones are
/// applied immediately.
const MAX_BACKLOG: usize = 2;

pub const EMITTER_ACTIVE: u32 = 1 << 0;
pub const EMITTER_SOUNDING: u32 = 1 << 1;
pub const EMITTER_LISTENER: u32 = 1 << 2;

pub const STATE_OFF: u32 = 0;
pub const STATE_LOADING: u32 = 1;
pub const STATE_READY: u32 = 2;
pub const STATE_ERROR: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SvAudioConfig {
    pub size: u32,
    pub sample_rate: u32,
    pub cache_dir_utf8: *const c_char,
    pub master_gain: f32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SvAudioEmitter {
    pub size: u32,
    pub id: u32,
    pub flags: u32,
    pub channels: u32,
    pub sample_rate: u32,
    pub queued_frames: u32,
    pub position: SvVec3,
    pub velocity: SvVec3,
    pub gain: f32,
    pub voices: u32,
    pub frame: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SvAudioStatus {
    pub size: u32,
    pub state: u32,
    pub voices: u32,
    pub underruns: u32,
    pub frames: u64,
    pub message_utf8: [u8; 96],
}

/// The audio-relevant state the worker publishes after each Skate tick.
#[derive(Clone, Default)]
pub struct Published {
    /// A Skate session is active (skate mode on).
    pub active: bool,
    /// Skate's game variables for the sound handlers.
    pub frame: live::Frame,
    /// Emitter positions and velocities, GTA space.
    pub positions: [Vec3; EMITTERS],
    pub velocities: [Vec3; EMITTERS],
    /// Skate tick of this publication.
    pub tick: u64,
}

/// Preserve one-tick edges even when the producer runs between audio blocks.
/// Overflow is an explicit error rather than silently erasing gameplay events.
#[derive(Default)]
struct Publications {
    /// The newest publication's emitter positions, velocities and tick (what
    /// a deactivation repeats; an inactive frame is never read).
    latest: ([Vec3; EMITTERS], [Vec3; EMITTERS], u64),
    pending: VecDeque<Published>,
    overflow: bool,
}
impl Publications {
    fn push(&mut self, p: Published) {
        if self.pending.len() == 240 { self.overflow = true; return; }
        self.latest = (p.positions, p.velocities, p.tick);
        self.pending.push_back(p);
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    #[test]
    fn short_event_and_deactivation_survive_a_delayed_consumer() {
        let mut q=Publications::default();
        for tick in 7..10 {q.push(Published{active:tick!=9,tick,..Default::default()});}
        let events:Vec<_>=q.pending.drain(..).map(|p|(p.tick,p.active)).collect();
        assert_eq!(events,[(7,true),(8,true),(9,false)]);
        assert!(!q.overflow);
    }
    #[test]
    fn stalled_consumer_fails_explicitly_at_the_bound() {
        let mut q=Publications::default();
        for tick in 0..241 {q.push(Published{active:true,tick,..Default::default()});}
        assert!(q.overflow);assert_eq!(q.pending.len(),240);
        assert_eq!(q.pending.front().unwrap().tick,0);
    }
}

/// One emitter's ring and the state at its newest frame.
#[derive(Default)]
struct Ring {
    samples: VecDeque<f32>,
    /// Stream position of the newest frame in `samples` (frames since configure).
    end: u64,
    info: EmitterInfo,
}

impl Ring {
    fn discard(&mut self) {
        self.samples.clear();
        self.info.flags = 0;
        self.info.voices = 0;
    }

    fn pull(&mut self, out: &mut [f32], paused: bool) -> usize {
        out.fill(0.0);
        if paused {
            self.discard();
            return 0;
        }
        let n = out.len().min(self.samples.len());
        for (o, v) in out.iter_mut().zip(self.samples.drain(..n)) { *o = v; }
        n
    }
}

#[derive(Clone, Copy, Default)]
struct EmitterInfo {
    flags: u32,
    voices: u32,
    position: Vec3,
    velocity: Vec3,
}

struct Shared {
    rings: [Mutex<Ring>; EMITTERS],
    published: Mutex<Publications>,
    paused: AtomicBool,
    stop: AtomicBool,
    state: AtomicU32,
    voices: AtomicU32,
    underruns: AtomicU32,
    frames: AtomicU64,
    message: Mutex<String>,
    master_gain: f32,
    /// What is sounding: per class and per emitter, refreshed 4 times a
    /// second (`sv_audio_debug_text`, the host's sound list).
    debug: Mutex<String>,
}

/// The process's audio engine (one runtime per process).
static ENGINE: OnceLock<Mutex<Option<Arc<Shared>>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Arc<Shared>>> {
    ENGINE.get_or_init(|| Mutex::new(None))
}

fn current() -> Option<Arc<Shared>> {
    slot().lock().ok()?.clone()
}

/// Called by the Skate worker after each published Skate tick (cheap: a
/// clone under a mutex). `None` when skate mode is inactive.
pub fn publish(p: Published) {
    if let Some(s) = current() {
        if let Ok(mut g) = s.published.lock() {
            g.push(p);
        }
    }
}

/// Called by the Skate worker after every Skate tick while skate mode is
/// active: reads the session's audio state (read-only) and publishes it.
pub fn on_tick(s: &skate_host::bridge::Session, pose: &skate_host::bridge::Pose, state: &mut map::State) {
    if !wanted() {
        return;
    }
    let (deck, _) = s.deck();
    let deck = crate::coords::from_skate(Vec3::from_array(deck));
    let (root, _) = crate::coords::transform_from_skate(pose.root);
    let velocity = crate::coords::basis() * pose.velocity;
    let mut frame = state.frame(s);
    crate::surfaces::apply_to_frame(&mut frame);
    crate::ground::apply_to_frame(&mut frame);
    publish(Published {
        active: true,
        frame,
        positions: [deck, root + Vec3::new(0.0, 0.0, 0.95), root + Vec3::new(0.0, 0.0, 1.6)],
        velocities: [velocity, velocity, velocity],
        tick: pose.tick,
    });
}

/// Whether anyone configured audio (the worker skips building the frame
/// otherwise).
pub fn wanted() -> bool {
    current().is_some_and(|s| s.state.load(Ordering::Relaxed) != STATE_ERROR)
}

/// Marks skate mode inactive (voices end, tails play out).
pub fn deactivate() {
    if let Some(s) = current() {
        if let Ok(mut g) = s.published.lock() {
            let (positions, velocities, tick) = g.latest;
            g.push(Published { active: false, positions, velocities, tick, ..Default::default() });
        }
    }
}

fn set_message(s: &Shared, m: impl Into<String>) {
    if let Ok(mut g) = s.message.lock() {
        *g = m.into();
    }
}

/// `sv_audio_configure`.
pub fn configure(cache: PathBuf, master_gain: f32, log: crate::worker::Log) -> bool {
    // Stop a previous engine.
    shutdown();
    let shared = Arc::new(Shared {
        rings: std::array::from_fn(|_| Mutex::new(Ring::default())),
        published: Mutex::new(Publications::default()),
        paused: AtomicBool::new(false),
        stop: AtomicBool::new(false),
        state: AtomicU32::new(STATE_LOADING),
        voices: AtomicU32::new(0),
        underruns: AtomicU32::new(0),
        frames: AtomicU64::new(0),
        message: Mutex::new(format!("loading {}", cache.display())),
        master_gain: if master_gain > 0.0 && master_gain.is_finite() { master_gain } else { 1.0 },
        debug: Mutex::new(String::new()),
    });
    let thread_shared = Arc::clone(&shared);
    let started = std::thread::Builder::new()
        .name("skatev-audio".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let s = thread_shared;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&s, cache, &log)));
            let msg = match result {
                Ok(Ok(())) => return,
                Ok(Err(e)) => e,
                Err(p) => format!(
                    "audio thread panicked: {}",
                    p.downcast_ref::<String>().map(String::as_str).or_else(|| p.downcast_ref::<&str>().copied()).unwrap_or("unknown")
                ),
            };
            log(&format!("audio: ERROR {msg}"));
            s.state.store(STATE_ERROR, Ordering::Relaxed);
            set_message(&s, msg);
        });
    if started.is_err() {
        return false;
    }
    if let Ok(mut g) = slot().lock() {
        *g = Some(shared);
    }
    true
}

fn run(s: &Shared, cache: PathBuf, log: &crate::worker::Log) -> Result<(), String> {
    let t = Instant::now();
    let mut engine = live::Engine::open(&cache, None)?;
    engine.driver.ensure_live_ready()?;
    log(&format!("audio: engine ready in {}ms: {}", t.elapsed().as_millis(), engine.summary()));
    s.state.store(STATE_READY, Ordering::Relaxed);
    set_message(s, "ready");
    let mut produced: u64 = 0;
    // Wall-clock pacing: frames due = elapsed * rate + latency.
    let mut base = Instant::now();
    let mut base_frames: u64 = 0;
    let mut was_paused = false;
    let mut published = Published::default();
    let mut next_game_frame = 0u64;
    let mut block = [[0.0f32; BLOCK]; EMITTERS];
    let mut problems_logged = 0usize;
    let mut debug_energy = [0.0f64; EMITTERS];
    let mut debug_frames = 0u64;
    while !s.stop.load(Ordering::Relaxed) {
        let paused = s.paused.load(Ordering::Relaxed);
        if paused {
            was_paused = true;
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        if was_paused {
            was_paused = false;
            base = Instant::now();
            base_frames = produced;
        }
        let due = base_frames + (base.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as u64 + LATENCY_FRAMES as u64;
        if produced + BLOCK as u64 > due {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        if produced >= next_game_frame || !published.active {
            // Frames beyond MAX_BACKLOG (a hitch, then Skate catching up)
            // are applied at once instead of one per 1/60 s of audio, which
            // would delay every later sound by the backlog for good.
            let batch: Vec<Published> = {
                let mut queue = s.published.lock().map_err(|_| "audio publications poisoned")?;
                if queue.overflow { return Err("audio gameplay frame queue overflow".into()); }
                let n = queue.pending.len().saturating_sub(MAX_BACKLOG).max(1).min(queue.pending.len());
                queue.pending.drain(..n).collect()
            };
            if let Some(next) = batch.last().cloned() {
                crate::crash::AUDIO.enter(1);
                for p in &batch {
                    engine.set_active(p.active);
                    if p.active { engine.set_frame(&p.frame); }
                }
                crate::crash::AUDIO.leave();
                published = next;
                next_game_frame = next_game_frame.max(produced.saturating_sub(BLOCK as u64)) + SAMPLE_RATE as u64 / 60;
            } else {
                next_game_frame = produced;
            }
        }
        if !published.active && engine.idle() {
            next_game_frame = produced;
            // Nothing to play: keep the clock aligned without producing.
            base = Instant::now();
            base_frames = produced;
            for r in &s.rings {
                if let Ok(mut r) = r.lock() {
                    r.info.flags = 0;
                    r.info.voices = 0;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        crate::crash::AUDIO.enter(2);
        let info = engine.render(&mut block);
        crate::crash::AUDIO.leave();
        for (e, b) in block.iter().enumerate() {
            debug_energy[e] += b.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>();
        }
        debug_frames += BLOCK as u64;
        if debug_frames >= u64::from(SAMPLE_RATE) / 4 {
            let mut text = String::new();
            let db = |energy: f64| if energy > 0.0 { 10.0 * (energy / debug_frames as f64).log10() } else { -120.0 };
            text.push_str(&format!("streams dBFS: board {:.0}  body {:.0}  speed {:.0}\n",
                db(debug_energy[0]), db(debug_energy[1]), db(debug_energy[2])));
            let mut meters = engine.take_meters(debug_frames);
            meters.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (class, rms, share, voices) in meters {
                text.push_str(&format!("{class}: {rms:.0} dBFS, {:.0}% of the time, {voices} voice{}\n",
                    share * 100.0, if voices == 1 { "" } else { "s" }));
            }
            if let Ok(mut d) = s.debug.lock() { *d = text; }
            debug_energy = [0.0; EMITTERS];
            debug_frames = 0;
        }
        produced += BLOCK as u64;
        s.frames.store(produced, Ordering::Relaxed);
        let mut total = 0;
        for (e, r) in s.rings.iter().enumerate() {
            total += info[e].voices;
            let mut r = r.lock().map_err(|_| "audio ring poisoned")?;
            // Check while holding the same lock used by set_paused: a block
            // that was rendering at the pause edge must not refill the ring.
            if s.paused.load(Ordering::Acquire) {
                r.discard();
                continue;
            }
            r.samples.extend(block[e].iter().copied());
            if r.samples.len() > LATENCY_FRAMES + RING_SLACK {
                let excess = r.samples.len() - LATENCY_FRAMES;
                r.samples.drain(..excess);
            }
            r.end = produced;
            let mut flags = 0;
            if published.active {
                flags |= EMITTER_ACTIVE;
            }
            if info[e].voices > 0 {
                flags |= EMITTER_SOUNDING;
            }
            if e == skate_aems::live::EMITTER_SPEED {
                flags |= EMITTER_LISTENER;
            }
            r.info = EmitterInfo { flags, voices: info[e].voices, position: published.positions[e], velocity: published.velocities[e] };
        }
        s.voices.store(total, Ordering::Relaxed);
        if problems_logged < 50 && engine.problem_count() > problems_logged {
            let problems = engine.problems();
            while problems_logged < problems.len() && problems_logged < 50 {
                log(&format!("audio: {}", problems[problems_logged]));
                problems_logged += 1;
            }
        }
    }
    Ok(())
}

pub fn set_paused(paused: bool) -> bool {
    match current() {
        Some(s) => {
            s.paused.store(paused, Ordering::Release);
            if paused {
                for r in &s.rings {
                    if let Ok(mut r) = r.lock() { r.discard(); }
                }
                s.voices.store(0, Ordering::Relaxed);
            }
            true
        }
        None => false,
    }
}

fn info_of(s: &Shared, id: usize, r: &Ring, frame: u64) -> SvAudioEmitter {
    SvAudioEmitter {
        size: size_of::<SvAudioEmitter>() as u32,
        id: id as u32,
        flags: r.info.flags,
        channels: 1,
        sample_rate: SAMPLE_RATE,
        queued_frames: r.samples.len() as u32,
        position: v3(r.info.position),
        velocity: v3(r.info.velocity),
        gain: s.master_gain,
        voices: r.info.voices,
        frame,
    }
}

fn v3(v: Vec3) -> SvVec3 {
    SvVec3 { x: v.x, y: v.y, z: v.z }
}

/// `sv_audio_emitters`: count, and the state of the first `out.len()`.
pub fn emitters(out: &mut [SvAudioEmitter]) -> u32 {
    let Some(s) = current() else { return 0 };
    for (id, o) in out.iter_mut().enumerate().take(EMITTERS) {
        if let Ok(r) = s.rings[id].lock() {
            *o = info_of(&s, id, &r, r.end);
        }
    }
    EMITTERS as u32
}

/// `sv_audio_pull`: copies up to `out.len()` queued frames.
pub fn pull(id: usize, out: &mut [f32], info: Option<&mut SvAudioEmitter>) -> u32 {
    let n_want = out.len();
    let Some(s) = current().filter(|_| id < EMITTERS) else {
        out.fill(0.0);
        return 0;
    };
    let Ok(mut r) = s.rings[id].lock() else {
        out.fill(0.0);
        return 0;
    };
    let n = r.pull(out, s.paused.load(Ordering::Acquire));
    if n < n_want && s.state.load(Ordering::Relaxed) == STATE_READY && r.info.flags & EMITTER_SOUNDING != 0 {
        s.underruns.fetch_add(1, Ordering::Relaxed);
    }
    let newest = r.end - r.samples.len() as u64;
    if let Some(info) = info {
        *info = info_of(&s, id, &r, newest);
    }
    n as u32
}

pub fn status(out: &mut SvAudioStatus) {
    out.size = size_of::<SvAudioStatus>() as u32;
    out.message_utf8 = [0; 96];
    match current() {
        None => {
            out.state = STATE_OFF;
            out.voices = 0;
            out.underruns = 0;
            out.frames = 0;
            crate::copy_str(&mut out.message_utf8, "not configured");
        }
        Some(s) => {
            out.state = s.state.load(Ordering::Relaxed);
            out.voices = s.voices.load(Ordering::Relaxed);
            out.underruns = s.underruns.load(Ordering::Relaxed);
            out.frames = s.frames.load(Ordering::Relaxed);
            let m = s.message.lock().map(|m| m.clone()).unwrap_or_default();
            crate::copy_str(&mut out.message_utf8, &m);
        }
    }
}

/// Stops the engine (runtime released).
pub fn shutdown() {
    if let Ok(mut g) = slot().lock() {
        if let Some(old) = g.take() {
            old.stop.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    #[test]
    fn pause_discards_queued_audio_without_replaying_it_on_resume() {
        let mut ring = Ring::default();
        ring.samples.extend([0.25, -0.5, 0.75]);
        ring.end = 3;
        ring.info.flags = EMITTER_ACTIVE | EMITTER_SOUNDING;
        ring.info.voices = 1;
        let mut out = [9.0; 4];
        assert_eq!(ring.pull(&mut out, true), 0);
        assert_eq!(out, [0.0; 4]);
        assert_eq!((ring.info.flags, ring.info.voices), (0, 0));
        assert_eq!(ring.end, 3);
        ring.samples.extend([0.125, 0.5]);
        assert_eq!(ring.pull(&mut out, false), 2);
        assert_eq!(out, [0.125, 0.5, 0.0, 0.0]);
    }

    /// Must match the static_asserts in the header's audio block.
    #[test]
    fn audio_abi_layout_matches_header() {
        assert_eq!(size_of::<SvAudioConfig>(), 24);
        assert_eq!(offset_of!(SvAudioConfig, cache_dir_utf8), 8);
        assert_eq!(offset_of!(SvAudioConfig, master_gain), 16);
        assert_eq!(size_of::<SvAudioEmitter>(), 64);
        assert_eq!(offset_of!(SvAudioEmitter, position), 24);
        assert_eq!(offset_of!(SvAudioEmitter, gain), 48);
        assert_eq!(offset_of!(SvAudioEmitter, frame), 56);
        assert_eq!(size_of::<SvAudioStatus>(), 120);
        assert_eq!(offset_of!(SvAudioStatus, frames), 16);
        assert_eq!(offset_of!(SvAudioStatus, message_utf8), 24);
    }
}

/// `sv_audio_debug_text`: what is sounding (classes and streams), refreshed
/// 4 times a second; empty when the engine is not running.
pub fn debug_text() -> String {
    current().and_then(|s| s.debug.lock().ok().map(|d| d.clone())).unwrap_or_default()
}

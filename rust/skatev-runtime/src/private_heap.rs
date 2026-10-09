//! A private Win32 heap for everything this DLL allocates.
//!
//! Rust's Windows allocator uses the process heap, which the MSVC CRT and the
//! game share. The Skate worker rebuilds 300k-triangle collision areas
//! (millions of small allocations, ~1 s) as the player moves, even outside
//! skate mode; on the shared heap every one of those takes the lock GTA's own
//! CRT allocations wait on. A private heap has its own lock. Alignment
//! handling follows std's Windows `System` allocator: HeapAlloc returns
//! 16-byte aligned blocks; larger alignments over-allocate and keep the
//! original pointer just before the aligned block.
use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicIsize, Ordering};

type Handle = isize;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn HeapCreate(options: u32, initial: usize, maximum: usize) -> Handle;
    fn HeapDestroy(heap: Handle) -> i32;
    fn HeapAlloc(heap: Handle, flags: u32, bytes: usize) -> *mut u8;
    fn HeapReAlloc(heap: Handle, flags: u32, mem: *mut u8, bytes: usize) -> *mut u8;
    fn HeapFree(heap: Handle, flags: u32, mem: *mut u8) -> i32;
}

const HEAP_ZERO_MEMORY: u32 = 0x8;
/// HeapAlloc's guaranteed alignment on x64 (MEMORY_ALLOCATION_ALIGNMENT).
const MIN_ALIGN: usize = 16;

static HEAP: AtomicIsize = AtomicIsize::new(0);

fn heap() -> Handle {
    let h = HEAP.load(Ordering::Acquire);
    if h != 0 {
        return h;
    }
    // Growable, serialized (thread-safe), low-fragmentation by default.
    let created = unsafe { HeapCreate(0, 0, 0) };
    if created == 0 {
        // Out of resources: fall back to failing allocations (null), which
        // Rust reports as an allocation error rather than corrupting memory.
        return 0;
    }
    match HEAP.compare_exchange(0, created, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => created,
        Err(existing) => {
            unsafe { HeapDestroy(created) };
            existing
        }
    }
}

fn simple(layout: Layout) -> bool {
    layout.align() <= MIN_ALIGN && layout.align() <= layout.size()
}

pub struct PrivateHeap;

unsafe impl GlobalAlloc for PrivateHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { allocate(layout, 0) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { allocate(layout, HEAP_ZERO_MEMORY) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let h = HEAP.load(Ordering::Acquire);
        let block = if simple(layout) { ptr } else { unsafe { *(ptr as *mut *mut u8).sub(1) } };
        unsafe { HeapFree(h, 0, block) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if simple(layout) && layout.align() <= new_size {
            return unsafe { HeapReAlloc(HEAP.load(Ordering::Acquire), 0, ptr, new_size) };
        }
        // Over-aligned: allocate, copy, free (std's default strategy).
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else { return std::ptr::null_mut() };
        let fresh = unsafe { self.alloc(new_layout) };
        if !fresh.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, fresh, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        fresh
    }
}

unsafe fn allocate(layout: Layout, flags: u32) -> *mut u8 {
    let h = heap();
    if h == 0 {
        return std::ptr::null_mut();
    }
    if simple(layout) {
        return unsafe { HeapAlloc(h, flags, layout.size()) };
    }
    // Room for the alignment slack plus the original pointer.
    let Some(total) = layout.size().checked_add(layout.align()) else { return std::ptr::null_mut() };
    let block = unsafe { HeapAlloc(h, flags, total) };
    if block.is_null() {
        return block;
    }
    let offset = layout.align() - (block as usize & (layout.align() - 1));
    let aligned = unsafe { block.add(offset) };
    unsafe { *(aligned as *mut *mut u8).sub(1) = block };
    aligned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_large_zeroed_and_overaligned_blocks_round_trip() {
        let a = PrivateHeap;
        for (size, align) in [(1, 1), (24, 8), (100, 16), (64, 64), (4096, 4096), (3, 32), (1 << 20, 16)] {
            let layout = Layout::from_size_align(size, align).unwrap();
            unsafe {
                let p = a.alloc_zeroed(layout);
                assert!(!p.is_null());
                assert_eq!(p as usize % align, 0, "{size}/{align}");
                assert!(std::slice::from_raw_parts(p, size).iter().all(|&b| b == 0));
                std::ptr::write_bytes(p, 0xAB, size);
                let q = a.realloc(p, layout, size * 3 + 1);
                assert!(!q.is_null());
                assert_eq!(q as usize % align, 0);
                assert!(std::slice::from_raw_parts(q, size).iter().all(|&b| b == 0xAB));
                a.dealloc(q, Layout::from_size_align(size * 3 + 1, align).unwrap());
            }
        }
        // The global allocator in this crate is this heap.
        let v: Vec<u64> = (0..100_000).collect();
        assert_eq!(v.iter().sum::<u64>(), 4_999_950_000);
    }
}

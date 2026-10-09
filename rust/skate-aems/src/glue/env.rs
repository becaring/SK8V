//! What a class handler reads: its component's packed parameters and fields,
//! the skater state, and the game helpers it calls, from the live Skate
//! session.

use super::Params;
use crate::live::driver::Component;
use crate::live::frame::{SeamGrid, SkaterImage};
use crate::live::tuning::Tuning;

/// A component's view of the live session: its own bytes, the skater state
/// and the tuning.
pub struct Env<'a> {
    pub comp: &'a mut Component,
    pub skater: &'a SkaterImage,
    pub tuning: &'a Tuning,
    pub params: Params,
}

impl Env<'_> {
    pub fn params(&self) -> &Params {
        &self.params
    }
    /// Component bytes / words / floats at an offset (`this + off`).
    pub fn c8(&self, off: usize) -> u8 {
        self.comp.image.get(off).copied().unwrap_or(0)
    }
    pub fn c32(&self, off: usize) -> u32 {
        if off + 4 <= self.comp.image.len() { self.comp.c32(off) } else { 0 }
    }
    pub fn cf(&self, off: usize) -> f32 {
        f32::from_bits(self.c32(off))
    }
    /// Skater state (`*(this + 36) + off`).
    pub fn s8(&self, off: usize) -> u8 {
        self.skater.r8(off)
    }
    pub fn s32(&self, off: usize) -> u32 {
        self.skater.r32(off)
    }
    pub fn sf(&self, off: usize) -> f32 {
        f32::from_bits(self.s32(off))
    }
    /// A game helper the port does not compute: `(r3, f1)` of `f(args)`.
    pub fn helper(&mut self, f: u32, args: &[u32]) -> (u32, f32) {
        crate::live::components::helpers(f, args, self.comp, self.skater, self.tuning)
    }

    /// Guest memory the handler reaches through pointers (`u8`/`u32`).
    pub fn g8(&self, addr: u32) -> u8 {
        self.tuning.g8(addr)
    }
    pub fn g32(&self, addr: u32) -> u32 {
        self.tuning.g32(addr)
    }
    pub fn g64(&self, addr: u32) -> u64 {
        ((self.g32(addr) as u64) << 32) | self.g32(addr + 4) as u64
    }
    /// Writes to the component (state a handler keeps, e.g. counters).
    pub fn set_c32(&mut self, off: usize, v: u32) {
        if off + 4 <= self.comp.image.len() {
            self.comp.set32(off, v);
            if self.comp.name == "seams" && off == 36 {
                self.comp.set32(40, self.tuning.collection_layout(v));
            }
        }
    }
    /// Byte write to the component (read-modify-write of its word).
    pub fn set_c8(&mut self, off: usize, v: u8) {
        let at = off & !3;
        let shift = (3 - (off & 3)) * 8;
        let w = (self.c32(at) & !(0xFF << shift)) | ((v as u32) << shift);
        self.set_c32(at, w);
    }

    /// Tuned value: `0x82B72420(*(*(*0x830CFDA4 + root)), key, 0)`, or the
    /// default `0x830D0850` (0.0) when the collection or key is missing.
    /// Returns the value's address.
    pub fn tuning(&self, root: u32, key: u64) -> u32 {
        self.tuning_at(root, key, 0)
    }
    /// Element `index` of an array value (`0x82B72420`'s third argument).
    pub fn tuning_at(&self, root: u32, key: u64, index: u32) -> u32 {
        self.tuning.tuning_at(root, key, index)
    }
    /// `0x82B72420(collection, key, index)` on a collection the handler holds
    /// itself; the default `0x830D0850` when missing.
    pub fn attrib(&self, collection: u32, key: u64) -> u32 {
        self.tuning.attrib(collection, key)
    }
    pub fn tuning_f32(&self, root: u32, key: u64) -> f32 {
        f32::from_bits(self.g32(self.tuning(root, key)))
    }
    pub fn tuning_i32(&self, root: u32, key: u64) -> i32 {
        self.g32(self.tuning(root, key)) as i32
    }

    /// `0x82B69B08(class, key)`: an AttribSys collection by keys (0 when
    /// missing).
    pub fn collection(&self, class_key: u64, key: u64) -> u32 {
        self.tuning.collection(class_key, key)
    }
    /// Recovered seam lines under a wheel (`None`: the authored grid).
    pub fn seam_grid(&self, wheel: i32) -> Option<SeamGrid> {
        self.skater.seam_grids.get(wheel as usize).copied().flatten()
    }
    /// Texture-space joint entry under a wheel this tick (`None`: no
    /// joint-mapped ground there).
    pub fn seam_hit(&self, wheel: i32) -> Option<bool> {
        self.skater.seam_hits.get(wheel as usize).copied().flatten()
    }

    /// `0x824C97B8(n)`: wheel table lookup (f32).
    pub fn wheel(&mut self, n: i32) -> f32 {
        self.helper(0x824C_97B8, &[n as u32]).1
    }
    /// `0x824C82A8(this, which)`: surface id (14 = none).
    pub fn surface(&mut self, which: u32) -> i32 {
        self.helper(0x824C_82A8, &[0, which]).0 as i32
    }
}

/// An AEMS object update a handler sends: the wrapper at `*(this + slot)`
/// and its words (from wrapper `+4`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Send {
    pub slot: u32,
    pub words: Vec<i32>,
    /// The handler released the object and deleted the wrapper instead.
    pub release: bool,
    /// The handler created the wrapper and its object with these words.
    pub create: bool,
}

impl Send {
    pub fn update(slot: u32, words: &[i32]) -> Send {
        Send { slot, words: words.to_vec(), release: false, create: false }
    }
    pub fn release(slot: u32) -> Send {
        Send { slot, words: Vec::new(), release: true, create: false }
    }
    pub fn create(slot: u32, words: &[i32]) -> Send {
        Send { slot, words: words.to_vec(), release: false, create: true }
    }
}

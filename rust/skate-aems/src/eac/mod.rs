//! EA Audio Core DSP: the plug-ins Skate 3's voice builder (`0x824A3140`)
//! chains for every AEMS voice, ported from the TU3 image.
//!
//! Each plug-in is a descriptor (`0x82FCD758`..`0x82FD34D0`: name, instance
//! size, constructor, attribute table) whose function table (descriptor
//! `-0x18`) holds `{0, init, process}`. `process(instance, context)` runs one
//! 256-sample block per channel; it reads its input buffer set at
//! `context + 28`, writes the set at `+ 32` and swaps the two, or leaves
//! them alone to pass its input through. The ports below process in place:
//! a pass-through leaves the buffer untouched.
//!
//! Float rules follow the recompiled game, which runs everything with
//! flush-to-zero and denormals-are-zero; run these under MXCSR FTZ/DAZ too.
//! Single-precision `fmuls`/`fadds`/`fsubs`/`fdivs` equal native `f32`
//! arithmetic; fused ops round twice (`crate::ops::fmadds`, `fnmsubs`).

pub mod gain;
pub mod frequency_shift;
pub mod grain;
pub mod iir2;
pub mod rechannel;
pub mod resample;
pub mod snr;
pub mod sndplayer;
pub mod xma;

/// Samples per channel in one EA Audio Core block.
pub const BLOCK: usize = 256;

/// A single-precision load (`lfs`) as the recomp performs it: widening under
/// denormals-are-zero turns a denormal into a zero of the same sign.
pub fn lfs(x: f32) -> f32 {
    if x.is_subnormal() { 0.0f32.copysign(x) } else { x }
}

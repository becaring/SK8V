//! `Resample` (request `0x82B2DAC8`, process `0x82B2DBA8`): pitch shifting by
//! linear interpolation on a 16.16 fixed-point position.
//!
//! EA Audio Core pulls: before a block, every plug-in's request turns the
//! samples it must produce into the samples it needs from upstream; the
//! player then delivers that many and the chain processes forward. The
//! resampler keeps the last input samples of each channel (`inst +
//! *(u16)(inst + 76) + 24·ch`, six slots) so interpolation runs across
//! blocks.
//!
//! Instance layout: `+42` channels, `+52` pitch (ratio), `+56` applied
//! ratio (capped at 4), `+60` ratio the step was made for, `+64` source
//! rate, `+68` step (16.16), `+72` position fraction (16 bits), `+78` block
//! size requested, `+80` history length, `+81` lookahead (2: the
//! interpolation reads one sample past each position, and the count keeps
//! that sample inside the data).

use super::lfs;
use crate::ops::fmadds;
use crate::ppc::fctiwz;

/// Fraction to `[0, 1)` (`0x822F87AC`; slightly below 2^-16).
const FRAC_SCALE: f32 = f32::from_bits(0x377F_FC9C);
const MAX_STEP: u32 = 0x4_0000;

/// History slots per channel.
pub const HISTORY: usize = 6;

/// One Resample instance.
#[derive(Clone, Debug)]
pub struct Resample {
    pub pitch: f32,
    pub ratio: f32,
    pub last_ratio: f32,
    pub source_rate: f32,
    pub step: u32,
    pub frac: u32,
    pub block: u16,
    pub history_len: u8,
    pub lookahead: u8,
    pub history: Vec<[f32; HISTORY]>,
}

/// What a process call did to the chain's buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The source rate changed: the block was not processed, the chain's
    /// rate became the mix rate and the input passes on unchanged.
    RateChanged,
    /// `count` resampled outputs were written.
    Resampled { count: u32 },
}

/// `0x82B43FB8`: `count` outputs from the samples `history` then `input`
/// (one run, read in place) starting at position 0 with fraction `frac`;
/// returns the integer position and fraction reached.
fn interpolate(count: u32, history: &[f32], input: &[f32], out: &mut [f32], step: u32, frac: u32) -> (u32, u32) {
    let at = |i: usize| if i < history.len() { history[i] } else { input[i - history.len()] };
    let mut index = 0u32;
    let mut frac = frac & 0xFFFF;
    for o in out.iter_mut().take(count as usize) {
        let x0 = lfs(at(index as usize));
        let x1 = lfs(at(index as usize + 1));
        let t = frac as f32 * FRAC_SCALE;
        *o = fmadds(x1 - x0, t, x0);
        let acc = frac.wrapping_add(step);
        index = index.wrapping_add(acc >> 16);
        frac = acc & 0xFFFF;
    }
    (index, frac)
}

impl Resample {
    /// As the constructor `0x82B2CA00` leaves it (`+60` 53839.332 forces
    /// the first request to compute a step; `+64` 48 kHz).
    pub fn new(channels: usize) -> Resample {
        Resample {
            pitch: 1.0,
            ratio: 0.0,
            last_ratio: f32::from_bits(0x4752_4F55),
            source_rate: 48_000.0,
            step: 0,
            frac: 0,
            block: 0,
            history_len: 0,
            lookahead: 2,
            history: vec![[0.0; HISTORY]; channels],
        }
    }

    /// The request: `outputs` samples wanted at `mix_rate`; multiplies the
    /// chain's rate factor (context `+56`) by the applied ratio and returns
    /// the input samples needed.
    pub fn request(&mut self, mix_rate: f32, outputs: u32, rate_factor: &mut f32) -> u32 {
        let ratio = lfs(self.source_rate) / mix_rate * lfs(self.pitch);
        if !(lfs(self.last_ratio) == ratio) {
            let x = ratio * 65536.0;
            let r = if x < 0.0 { fctiwz((x - 0.5) as f64) } else { fctiwz((x + 0.5) as f64) } as u32;
            if (r as i32) > MAX_STEP as i32 {
                self.ratio = 4.0;
                self.step = MAX_STEP;
            } else {
                self.ratio = ratio;
                self.step = r;
            }
            self.last_ratio = ratio;
        }
        let span = self.step.wrapping_mul(outputs);
        self.block = outputs as u16;
        *rate_factor *= lfs(self.ratio);
        let needed = (span.wrapping_add(self.frac) >> 16)
            .wrapping_sub(self.history_len as u32)
            .wrapping_add(self.lookahead as u32);
        if (needed as i32) < 0 { 0 } else { needed }
    }

    /// One block: `avail` input samples per channel at `in_rate` (context
    /// `+48`, `+52`) into `output`.
    pub fn process(&mut self, in_rate: f32, avail: u32, input: &[&[f32]], output: &mut [&mut [f32]]) -> Outcome {
        if !(in_rate == lfs(self.source_rate)) {
            self.source_rate = in_rate;
            return Outcome::RateChanged;
        }
        let total = self.history_len as u32 + avail;
        let span = total.wrapping_sub(self.lookahead as u32).wrapping_add(1);
        let mut count = if (span as i32) <= 0 {
            0
        } else {
            (span << 16).wrapping_sub(self.frac).wrapping_sub(1).checked_div(self.step).unwrap_or(8192)
        };
        if count > self.block as u32 {
            count = self.block as u32;
        }
        let mut left = 0u32;
        let mut frac = 0u32;
        for ch in 0..self.history.len() {
            let hl = self.history_len as usize;
            let history: [f32; HISTORY] = self.history[ch];
            let input = &input[ch][..avail as usize];
            let (index, f) = interpolate(count, &history[..hl], input, output[ch], self.step, self.frac);
            frac = f;
            left = total.wrapping_sub(index);
            // Groups of four go through lfs/stfs, the rest through memcpy.
            // More than six left over would overrun the history (the game
            // never delivers more input than the request asked for).
            let tail_len = (total as usize).saturating_sub(index as usize);
            assert!(tail_len <= HISTORY, "resampler history overrun");
            let fours = if left >= 4 { (left as usize - 4) / 4 * 4 + 4 } else { 0 };
            for k in 0..tail_len {
                let i = index as usize + k;
                let s = if i < hl { history[i] } else { input[i - hl] };
                self.history[ch][k] = if k < fours { lfs(s) } else { s };
            }
        }
        self.history_len = left as u8;
        self.frac = frac;
        Outcome::Resampled { count }
    }
}

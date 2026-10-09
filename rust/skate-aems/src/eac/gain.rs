//! `Gain` (process `0x82B23B50`): a linear gain that moves from its current
//! value (`+56`) to its target (`+52`, the gain attribute) over the first 64
//! samples of a block, through the VMX ramp kernel `0x82B3C098`.

use super::{lfs, BLOCK};
use crate::ops::fmadds;

/// `0x82B3C098(out, in, gain, step)`: samples 0..64 get the ramp, built four
/// lanes at a time and re-based every 32 samples exactly as the original's
/// `vmaddfp` chain does; samples 64..256 get `gain + 64·step`.
pub fn ramp(samples: &mut [f32], gain: f32, step: f32) {
    assert_eq!(samples.len(), BLOCK);
    let step4 = step * 4.0;
    let last = fmadds(step, 64.0, gain);
    let mut base = [gain, gain + step, fmadds(step, 2.0, gain), fmadds(step, 3.0, gain)];
    for t in 0..2 {
        for j in 0..8 {
            for l in 0..4 {
                let g = if j == 0 { base[l] } else { j as f32 * step4 + base[l] };
                let i = 32 * t + 4 * j + l;
                samples[i] *= g;
            }
        }
        base = base.map(|b| 8.0 * step4 + b);
    }
    for s in &mut samples[64..] {
        *s *= last;
    }
}

/// One Gain instance.
#[derive(Clone, Copy, Debug, Default)]
pub struct Gain {
    /// Target gain (`+52`).
    pub target: f32,
    /// Gain reached at the end of the last block (`+56`).
    pub current: f32,
}

impl Gain {
    /// One block. `first` is the process call's flag byte (`r5`), which
    /// starts at the target without a ramp.
    pub fn process(&mut self, first: bool, channels: &mut [&mut [f32]]) {
        if first {
            self.current = lfs(self.target);
        }
        let current = lfs(self.current);
        let step = (lfs(self.target) - current) * 0.015625;
        for samples in channels.iter_mut() {
            ramp(samples, current, step);
        }
        self.current = lfs(self.target);
    }
}

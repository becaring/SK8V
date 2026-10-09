//! Second-order IIR plug-ins `LowPassIir2` (process `0x82B27E20`) and
//! `HighPassIir2` (`0x82B26568`): RBJ biquads with Q = 1 on the cutoff
//! attribute (instance `+52`, Hz), sharing the block kernel `0x82B43AF8`.
//!
//! Instance layout: `+42` channel count, `+52` cutoff, `+56 + 16·ch` state
//! (x[n-1], x[n-2], y[n-1], y[n-2]), `+184` coefficients (a1, a2, b0, b1,
//! b2, normalised by a0), `+204` the angular frequency they were made for.
//!
//! `PeakingIir2` (`PI20`, descriptor `0x82FD17AC`, process `0x82B2C658`) is
//! the RBJ peaking EQ on the same kernel: `+52` frequency (Hz), `+60` gain
//! (linear), `+68` Q; `+72 + 16·ch` state, `+200` filtering, `+204`
//! coefficients, `+224..+232` the (angular frequency, gain, Q) they were
//! made for.

use crate::libm::{cos, sin};
use super::lfs;
use crate::ops::{fmadds, fnmsubs};

/// `2π` (`0x820B411C`).
const TWO_PI: f32 = f32::from_bits(0x40C9_0FDB);
/// Lowest and highest angular cutoff (`0x822F8E7C`, `0x822F8E80`).
const W_MIN: f32 = f32::from_bits(0x3B4D_E32F);
const W_MAX: f32 = f32::from_bits(0x4048_DC62);
/// Added before the feedback terms to keep the recursion out of denormals
/// (`0x822F87B0`).
const DC: f32 = f32::from_bits(0x2193_92EF);

/// Biquad coefficients `[a1, a2, b0, b1, b2]`, divided by a0.
pub type Coefs = [f32; 5];
/// Per-channel state `[x[n-1], x[n-2], y[n-1], y[n-2]]`.
pub type State = [f32; 4];

/// `0x82B43AF8`: filters `samples` in place, eight at a time (the caller
/// always passes 256; other lengths go to `0x82B43978`, never used here).
/// Each output is `b0·x + b1·x[n-1] + b2·x[n-2] + DC − a1·y[n-1] − a2·y[n-2]`
/// with the original's term order, which differs for the first two samples
/// of every group of eight.
pub fn biquad(state: &mut State, c: &Coefs, samples: &mut [f32]) {
    assert!(samples.len().is_multiple_of(8), "0x82B43978 path not ported");
    let [a1, a2, b0, b1, b2] = *c;
    let [mut xm1, mut xm2, mut ym1, mut ym2] = *state;
    for block in samples.chunks_exact_mut(8) {
        let x: [f32; 8] = std::array::from_fn(|k| lfs(block[k]));
        let mut ff = [0.0f32; 8];
        ff[0] = fmadds(xm1, b1, fmadds(xm2, b2, x[0] * b0));
        ff[1] = fmadds(xm1, b2, fmadds(x[0], b1, x[1] * b0));
        for k in 2..8 {
            ff[k] = fmadds(x[k], b0, fmadds(x[k - 2], b2, x[k - 1] * b1));
        }
        for k in 0..8 {
            let y = fnmsubs(ym2, a2, fnmsubs(ym1, a1, ff[k] + DC));
            ym2 = ym1;
            ym1 = y;
            block[k] = y;
        }
        xm1 = x[7];
        xm2 = x[6];
    }
    *state = [xm1, xm2, ym1, ym2];
}

/// The shared front of both coefficient sets: `(sin w, cos w)` in single
/// precision and the RBJ terms with alpha = sin(w) / 2.
fn trig(w: f32) -> (f32, f32, f32, f32, f32) {
    let s = sin(w as f64) as f32;
    let c = cos(w as f64) as f32;
    let alpha = s * 0.5;
    let a0 = alpha + 1.0;
    let one_minus_alpha = 1.0 - alpha;
    let inv_a0 = 1.0 / a0;
    let two_a0 = a0 * 2.0;
    (c, c * -2.0, one_minus_alpha, inv_a0, two_a0)
}

/// `0x82B43CC0`: low-pass coefficients for angular frequency `w`.
pub fn lowpass_coefs(w: f32) -> Coefs {
    let (c, m2c, one_minus_alpha, inv_a0, two_a0) = trig(w);
    let one_minus_c = 1.0 - c;
    [m2c * inv_a0, one_minus_alpha * inv_a0, one_minus_c / two_a0, one_minus_c * inv_a0, one_minus_c / two_a0]
}

/// High-pass coefficients, computed inline by `0x82B26568`.
pub fn highpass_coefs(w: f32) -> Coefs {
    let (c, m2c, one_minus_alpha, inv_a0, two_a0) = trig(w);
    let one_plus_c = c + 1.0;
    [m2c * inv_a0, one_minus_alpha * inv_a0, one_plus_c / two_a0, -(one_plus_c * inv_a0), one_plus_c / two_a0]
}

/// `Q` limits of `PeakingIir2` (`0x82099280`, `0x820996EC`).
const Q_MIN: f32 = f32::from_bits(0x3E4C_CCCD);
const Q_MAX: f32 = f32::from_bits(0x41A0_0000);

/// `0x82B2C658`'s coefficients: the RBJ peaking EQ for angular frequency
/// `w`, linear gain `gain` (A = sqrt(gain)) and `q` (clamped to 0.2..20,
/// NaN to 20), in the original's single-precision order.
pub fn peaking_coefs(w: f32, gain: f32, q: f32) -> Coefs {
    let q = if q < Q_MIN { Q_MIN } else if q <= Q_MAX { q } else { Q_MAX };
    let s = sin(w as f64) as f32;
    let c = cos(w as f64) as f32;
    let a = gain.sqrt();
    let alpha = s / (q * 2.0);
    let m2c = c * -2.0;
    let alpha_over_a = alpha / a;
    let alpha_a = a * alpha;
    let inv_a0 = 1.0 / (alpha_over_a + 1.0);
    [inv_a0 * m2c, (1.0 - alpha_over_a) * inv_a0, (alpha_a + 1.0) * inv_a0, inv_a0 * m2c, (1.0 - alpha_a) * inv_a0]
}

/// One `PeakingIir2` instance.
#[derive(Clone, Debug)]
pub struct Peaking {
    /// Centre frequency in Hz (`+52`).
    pub frequency: f32,
    /// Linear gain at the centre (`+60`); 1 passes the input through.
    pub gain: f32,
    /// `+68`.
    pub q: f32,
    pub state: Vec<State>,
    pub coefs: Coefs,
    /// `+200`: the previous block filtered.
    filtering: bool,
    /// `+224..+232`.
    made_for: [f32; 3],
}

impl Peaking {
    /// `0x82B2C540`; the attributes start at the values `made_for` copies.
    pub fn new(channels: usize, frequency: f32, gain: f32, q: f32) -> Peaking {
        Peaking { frequency, gain, q, state: vec![[0.0; 4]; channels], coefs: [0.0; 5], filtering: false,
            made_for: [frequency, gain, q] }
    }

    /// `0x82B2C658`: one block at `rate` Hz. Returns false when the gain is
    /// 1 and the block passes through (the state is cleared once on the way
    /// into the pass-through).
    pub fn process(&mut self, rate: f32, channels: &mut [&mut [f32]]) -> bool {
        let w = self.frequency / rate * TWO_PI;
        let w = if w < W_MIN { W_MIN } else if w <= W_MAX { w } else { W_MAX };
        if self.gain == 1.0 {
            if self.filtering {
                self.state.fill([0.0; 4]);
                self.filtering = false;
            }
            self.made_for = [w, self.gain, self.q];
            return false;
        }
        self.filtering = true;
        if !(w == self.made_for[0] && self.gain == self.made_for[1] && self.q == self.made_for[2]) {
            self.coefs = peaking_coefs(w, self.gain, self.q);
            self.made_for = [w, self.gain, self.q];
        }
        for (state, samples) in self.state.iter_mut().zip(channels.iter_mut()) {
            biquad(state, &self.coefs, samples);
        }
        true
    }
}

/// One IIR2 plug-in instance.
#[derive(Clone, Debug)]
pub struct Iir2 {
    pub high_pass: bool,
    /// Cutoff in Hz (`+52`).
    pub cutoff: f32,
    pub state: Vec<State>,
    pub coefs: Coefs,
    /// Angular frequency of `coefs` (`+204`).
    pub w: f32,
}

impl Iir2 {
    pub fn new(high_pass: bool, channels: usize) -> Iir2 {
        Iir2 { high_pass, cutoff: 0.0, state: vec![[0.0; 4]; channels], coefs: [0.0; 5], w: 0.0 }
    }

    /// One block at `rate` Hz over the channel buffers. Returns false when
    /// the filter is out of range and the block passes through unchanged
    /// (the original leaves the buffer pointers unswapped).
    pub fn process(&mut self, rate: f32, channels: &mut [&mut [f32]]) -> bool {
        let mut w = self.cutoff / rate * TWO_PI;
        if self.high_pass {
            if !(w > W_MIN) {
                if !(self.w <= W_MIN) {
                    self.clear();
                }
                self.w = w;
                return false;
            }
            if !(w <= W_MAX) {
                w = W_MAX;
            }
            if !(w == self.w) {
                self.coefs = highpass_coefs(w);
                self.w = w;
            }
        } else {
            if !(w < W_MAX) {
                if !(self.w >= W_MAX) {
                    self.clear();
                }
                self.w = w;
                return false;
            }
            if !(w >= W_MIN) {
                w = W_MIN;
            }
            if !(w == self.w) {
                self.coefs = lowpass_coefs(w);
                self.w = w;
            }
        }
        for (state, samples) in self.state.iter_mut().zip(channels.iter_mut()) {
            biquad(state, &self.coefs, samples);
        }
        true
    }

    fn clear(&mut self) {
        self.state.fill([0.0; 4]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaking_is_unity_at_gain_one_and_boosts_its_centre() {
        let mut p = Peaking::new(1, 1000.0, 1.0, 3.0);
        let mut x = [0.25f32; 256];
        assert!(!p.process(48_000.0, &mut [&mut x[..]]));
        assert!(x.iter().all(|&v| v == 0.25));
        // Gain 4 (+12 dB) at 1 kHz: a 1 kHz sine comes out four times louder.
        p.gain = 4.0;
        let tone = |n: usize| ((n as f32) * 1000.0 / 48_000.0 * TWO_PI).sin();
        let mut peak = 0.0f32;
        for block in 0..40 {
            let mut b: Vec<f32> = (0..256).map(|i| tone(block * 256 + i)).collect();
            assert!(p.process(48_000.0, &mut [&mut b[..]]));
            if block > 20 {
                peak = b.iter().fold(peak, |m, v| m.max(v.abs()));
            }
        }
        assert!((peak - 4.0).abs() < 0.05, "{peak}");
        // Back to unity clears the state and passes through.
        p.gain = 1.0;
        let mut y = [0.5f32; 256];
        assert!(!p.process(48_000.0, &mut [&mut y[..]]));
        assert!(p.state[0] == [0.0; 4] && y[0] == 0.5);
    }
}

//! The sense-of-speed component (`0x824E7CB0`; skater state at `+32`, gate
//! `*(this + 28) + 72`): SenseOfSpeed_wind at `+36` and SenseOfSpeed_rattle
//! at `+40`. Each maps a speed between two component floats to 0..1000.

use super::env::{Env, Send};
use super::{clamp, fsel, Wrappers};
use crate::ops::{fctiwz, fmsubs};

/// `clamp01((speed × 3.6 − lo) / (hi − lo)) × 1000` (0x822F8628 = 3.6,
/// 0x82256FE8 = 1000).
fn band(speed: f32, lo: f32, hi: f32) -> i32 {
    let f12 = fmsubs(speed, 3.6, lo);
    let f10 = f12 / (hi - lo);
    let a = fsel(-f10, 0.0, f10);
    let b = fsel(1.0 - a, a, 1.0);
    fctiwz(b * 1000.0)
}

/// `0x824E7CB0`. Also writes a 4-float filter vector to `*(this + 100)` when
/// byte `+128` is set; that is not an AEMS call and is left to the host.
pub fn update(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let mut out = Vec::new();
    if env.g8(env.c32(28).wrapping_add(72)) == 0 {
        return out;
    }
    let p = *env.params();
    let speed = env.sf(208);
    if env.c32(36) != 0 {
        let lo = env.cf(52);
        let hi = env.cf(56);
        let level = env.c32(44) as i32;
        let w = wrappers.words(36, 11);
        w[0] = clamp(level, 32767);
        w[1] = clamp(p.u16(0), 65535);
        w[2] = clamp(p.pitch(2), 8192);
        w[6] = 0;
        w[7] = clamp(p.u15(1), 32767);
        w[3] = clamp(band(speed, lo, hi), 1000);
        out.push(Send::update(36, w));
    }
    let speed = env.sf(212);
    if env.c32(40) != 0 {
        let (mut lo, mut hi) = (env.cf(60), env.cf(64));
        if env.s8(676) != 0 {
            lo = env.cf(68);
            hi = env.cf(72);
        }
        let level = env.c32(48) as i32;
        let w = wrappers.words(40, 13);
        w[0] = clamp(level, 32767);
        w[2] = clamp(p.pitch(3), 8192);
        w[6] = 0;
        w[7] = clamp(p.u15(4), 32767);
        w[3] = clamp(band(speed, lo, hi), 1000);
        out.push(Send::update(40, w));
    }
    out
}

//! `Class_rolling` (SK8_AEMS_rolling): the board's roll sound. The board
//! sound component owns three rolling objects: `+1304` and `+1308` (updated
//! by `0x824C9948`) and `+1332` (updated by `0x824CA038`).
//!
//! - Create `0x824C4C18(wrapper, a, b, c)`: words
//!   `[0, 0, 4096, clamp(a, 10000), clamp(b, 15), 0, clamp(c, 13), 0, 0, 25000, 0, 32767]`.
//! - Each update writes every word but w4 (kept from the create), then sends
//!   the wrapper's words when its object exists. The three objects differ in
//!   the level id of w11 (7, 9, 10) and the source of w3.

use super::env::{Env, Send};
use super::{clamp, fsel, Wrappers};
use crate::ops::fctiwz;

/// `0x824C4C18`: the create vector.
pub fn create_words(a: i32, b: i32, c: i32) -> Vec<i32> {
    vec![0, 0, 4096, clamp(a, 10000), clamp(b, 15), 0, clamp(c, 13), 0, 0, 25000, 0, 32767]
}

/// `clamp01(speed / wheel × 3.6) × 10000` (`0x824C9948` inline and
/// `0x824C6B30`). Constants: `0x822F8628` = 3.6, `0x821161A0` = 10000.
fn speed_term(speed: f32, wheel: f32) -> i32 {
    let f13 = speed / wheel * 3.6f32;
    let f11 = fsel(-f13, 0.0, f13);
    let f9 = fsel(1.0 - f11, f11, 1.0);
    fctiwz(f9 * 10000.0)
}

/// `0x824C6B30(this, n)`: speed (skater `+208`, times component `+1028`
/// unless byte `+1032` is set) over the wheel lookup `n` (45.0 at
/// `0x82256FE4` when `n` is -1).
pub fn wheel_speed(env: &mut Env, n: i32) -> i32 {
    let mut speed = env.sf(208);
    if env.c8(1032) == 0 {
        speed *= env.cf(1028);
    }
    let wheel = if n == -1 { 45.0 } else { env.wheel(n) };
    speed_term(speed, wheel)
}

/// The shared body: every word but w3 and w11.
fn common(env: &mut Env, w: &mut [i32]) {
    w[0] = 32767;
    w[1] = clamp(env.params().u16(0), 0x10000);
    w[2] = clamp(env.params().pitch(8), 8192);
    w[5] = 0;
    let which = env.c32(1500);
    let mut s = env.surface(which);
    let mut take = true;
    if s == 14 {
        s = env.surface((which == 0) as u32);
        if s == 14 {
            take = false;
            if env.s8(341) != 0 {
                w[6] = 13;
            }
        }
    }
    if take {
        w[6] = clamp(s, 13);
    }
    w[7] = clamp((env.s8(340) != 0 || env.s8(339) != 0) as i32, 1);
    w[8] = clamp(env.params().u15(19), 32767);
    w[9] = clamp(env.params().u15(17), 25000);
    w[10] = clamp(env.params().u15(18), 25000);
}

/// `0x824C9948`: the objects at `+1304` (wheel 0, level 7) and `+1308`
/// (wheel 3, level 9).
pub fn update_9948(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let mut out = Vec::new();
    for (slot, wheel, level) in [(1304u32, 0, 7), (1308, 3, 9)] {
        if env.c32(slot as usize) == 0 {
            continue;
        }
        let w = wrappers.words(slot, 12);
        w[0] = 32767;
        w[11] = clamp(env.params().u15(level), 32767);
        w[1] = clamp(env.params().u16(0), 0x10000);
        w[2] = clamp(env.params().pitch(8), 8192);
        let speed = env.sf(208);
        let wh = env.wheel(wheel);
        w[3] = clamp(speed_term(speed, wh), 10000);
        common(env, w);
        out.push(Send::update(slot, w));
    }
    out
}

/// `0x824CA038`: the object at `+1332` (level 10, speed from
/// `0x824C6B30(this, 5)`).
pub fn update_a038(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 1332u32;
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    let level = clamp(env.params().u15(10), 32767);
    let w = wrappers.words(slot, 12);
    w[0] = 32767;
    w[11] = level;
    w[1] = clamp(env.params().u16(0), 0x10000);
    w[2] = clamp(env.params().pitch(8), 8192);
    w[3] = clamp(wheel_speed(env, 5), 10000);
    common(env, w);
    vec![Send::update(slot, w)]
}

/// `0x824C6BD8` (the board component's update), its AEMS part: the rolling
/// objects at `+1312`/`+1316` for pairs whose enable byte `+1496 + k` is
/// set and stream mode `+1320 + 4k` is 0, with the wheel lookup at
/// `+1488 + 4k`. (Stream mode instead feeds the grain voices' parameter
/// blocks at `+1176 + 8k`; not AEMS.)
pub fn update_6bd8(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let mut out = Vec::new();
    if env.g8(env.c32(28).wrapping_add(52)) == 0 {
        return out;
    }
    for k in 0..2usize {
        if env.c8(1496 + k) == 0 || env.c32(1320 + 4 * k) != 0 {
            continue;
        }
        let slot = (1312 + 4 * k) as u32;
        if env.c32(slot as usize) == 0 {
            continue;
        }
        let wheel = env.c32(1488 + 4 * k) as i32;
        let speed = wheel_speed(env, wheel);
        let p = *env.params();
        let level = p.u15(1);
        let w = wrappers.words(slot, 12);
        w[0] = 32767;
        w[11] = clamp(level, 32767);
        w[1] = clamp(p.u16(0), 0x10000);
        w[2] = clamp(p.pitch(3), 8192);
        w[3] = clamp(speed, 10000);
        w[5] = 0;
        w[8] = if wheel == 1 { 0 } else { clamp(p.u15(13), 32767) };
        w[9] = clamp(p.u15(11), 25000);
        w[10] = clamp(p.u15(12), 25000);
        w[7] = clamp((env.s8(340) != 0 || env.s8(339) != 0) as i32, 1);
        out.push(Send::update(slot, w));
    }
    out
}

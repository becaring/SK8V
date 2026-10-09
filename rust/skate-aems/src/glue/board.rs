//! The board sound component's other classes (component vtable `0x822FC770`,
//! skater state at `+36`): rattle, wheel skid, squeaks, board slide.
//! Translated from the TU3 handlers; constants and tuned keys are named at
//! their use.

use super::env::{Env, Send};
use super::{clamp, fsel, Wrappers};
use crate::ops::fctiwz;

/// `clamp01(x) × scale` truncated, the handlers' `fneg/fsel/fsubs/fsel` idiom.
fn unit(x: f32, scale: f32) -> i32 {
    let a = fsel(-x, 0.0, x);
    let b = fsel(1.0 - a, a, 1.0);
    fctiwz(b * scale)
}

/// `0x824C80C0`: Rolling_Rattle_Class at `+1300`.
pub fn rattle(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 1300u32;
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    let p = *env.params();
    let pitch = p.pitch(3);
    let w = wrappers.words(slot, 12);
    w[0] = 32767;
    w[10] = clamp(p.u15(6), 32767);
    w[1] = clamp(p.u16(0), 65535);
    w[2] = clamp(pitch, 8192);
    w[7] = clamp(p.u15(16), 32767);
    w[8] = clamp(p.u15(14), 25000);
    w[9] = clamp(p.u15(15), 25000);
    vec![Send::update(slot, w)]
}

/// `0x824C7A20`: Class_wheels_skid at `+1288`. Keeps a ramp counter at
/// component `+1516` (+5 per update while skater byte `+690` is set, up to
/// 45; otherwise −15 per update down to 0). Released when `0x824C72F0`
/// says the skid is over.
pub fn wheels_skid(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 1288u32;
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    let this = env.c32(0);
    let alive = env.helper(0x824C_72F0, &[this]).0 as u8;
    if alive == 0 {
        return vec![Send::release(slot)];
    }
    // 0x8208EDA4 = 0.08, 0x821161A0 = 10000.
    let speed = env.sf(208) * 0.08f32;
    let level = unit(speed, 10000.0);
    let mut ramp = env.c32(1516) as i32;
    if env.s8(690) != 0 {
        ramp += 5;
        env.set_c32(1516, ramp as u32);
        if ramp > 45 {
            env.set_c32(1516, 45);
        }
    } else if ramp != 0 {
        if ramp > 0 {
            env.set_c32(1516, (ramp - 15) as u32);
        }
        if (env.c32(1516) as i32) < 0 {
            env.set_c32(1516, 0);
        }
    }
    let p = *env.params();
    let pitch = p.pitch(3);
    // 0x822F9018 = -90.0
    let angle = fctiwz(env.sf(232) * -90.0f32);
    let lean = env.helper(0x824C_7388, &[this]).0 as i32;
    let flag = env.g8(env.c32(16).wrapping_add(72));
    let extra = if flag != 0 { p.u15(20) } else { 0 };
    let ramp_now = env.c32(1516) as i32;
    let w = wrappers.words(slot, 18);
    w[7] = clamp(level, 10000);
    w[4] = clamp(pitch, 8192);
    w[0] = 32767;
    w[1] = clamp(p.u15(4), 32767);
    w[3] = clamp(p.u16(0), 0x10000);
    w[2] = clamp(p.u15(13), 32767);
    w[5] = clamp(p.u15(11), 25000);
    w[6] = clamp(p.u15(12), 25000);
    let mut d = ramp_now.wrapping_sub(angle);
    if d > 90 {
        d = 90;
    }
    w[10] = clamp(d, 90);
    w[9] = clamp(lean, 4);
    w[16] = clamp(extra, 32767);
    vec![Send::update(slot, w)]
}

/// `0x824C7DD0`: Class_Squeaks at `+1292`.
pub fn squeaks(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 1292u32;
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    // 0x8208EDA4 = 0.08, 0x82256FE8 = 1000.
    let level = unit(env.sf(208) * 0.08f32, 1000.0);
    let scale = env.tuning_f32(4, 0xAC87_D592_E760_1134);
    // Third float of the 16-byte vector at skater +480.
    let v = env.sf(488).abs() / scale * 1000.0f32;
    let mut twist = fctiwz(v);
    if twist > 1000 {
        twist = 1000;
    } else {
        // Below 50 counts as 0 (the compiler's `addc/subfe` form).
        let r9 = (twist as u32).wrapping_sub(50);
        let (_, carry) = r9.overflowing_add(50u32 ^ 0x8000_0000);
        let mask = if carry { 0 } else { -1 };
        twist &= mask;
    }
    let p = *env.params();
    let w = wrappers.words(slot, 11);
    w[7] = clamp(level, 1000);
    w[9] = clamp(twist, 1000);
    w[4] = clamp(p.pitch(3), 8192);
    w[0] = 32767;
    w[1] = clamp(p.u15(5), 32767);
    w[2] = 0;
    w[5] = clamp(p.u15(11), 25000);
    w[6] = clamp(p.u15(12), 25000);
    w[3] = clamp(p.u16(0), 0x10000);
    vec![Send::update(slot, w)]
}

/// `0x824CB4C0`: c_board_slide at `+1884`.
pub fn board_slide(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 1884u32;
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    let p = *env.params();
    let speed = env.sf(208);
    let top = env.tuning_f32(96, 0x9635_B780_C747_2A6E);
    // 0x8209975C = 0.5, 0x822F8628 = 3.6, 0x821161A0 = 10000.
    let level = unit((speed - 0.5) / top * 3.6f32, 10000.0);
    let mut mode = env.tuning_i32(96, 0x662C_EE73_D2E3_FE2F);
    if env.s32(780) as i32 == 2 {
        mode = env.tuning_i32(96, 0xAB87_C3D1_EDDD_DCBC);
    }
    let w = wrappers.words(slot, 12);
    w[0] = 32767;
    w[1] = clamp(p.u16(0), 65535);
    w[2] = clamp(p.pitch(23), 8192);
    w[3] = clamp(level, 10000);
    w[4] = clamp(p.u15(25), 25000);
    w[5] = clamp(p.u15(26), 25000);
    w[6] = clamp(p.u15(27), 32767);
    w[7] = clamp(p.u15(24), 32767);
    w[11] = clamp(mode, 32767);
    vec![Send::update(slot, w)]
}

/// `0x824BEEE8`: Class_foot_drag at `+128` (a component with the skater
/// state at `+32` and the gate `*(this + 28) + 72`). Released when the
/// skater's drag bytes `+336`/`+339` and the gated byte `+310` are all clear.
pub fn foot_drag(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 128u32;
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    let gated = env.g8(env.c32(28).wrapping_add(72)) != 0 && env.s8(310) != 0;
    if env.s8(336) == 0 && env.s8(339) == 0 && !gated {
        return vec![Send::release(slot)];
    }
    let top = env.tuning_f32(24, 0xB2C8_1577_8204_08BE);
    // 0x8209975C = 0.5, 0x822F8628 = 3.6, 0x821161A0 = 10000.
    let mut level = unit((env.sf(208) - 0.5) / top * 3.6f32, 10000.0);
    if gated {
        level = 500;
    }
    let p = *env.params();
    let id = if env.s8(336) != 0 { 4 } else { 5 };
    let this = env.c32(0);
    let which = env.s8(339) as u32;
    let style = env.helper(0x824B_A390, &[this, which]).0 as i32;
    let w = wrappers.words(slot, 15);
    w[7] = clamp(level, 10000);
    w[0] = 32767;
    w[1] = clamp(p.u15(id), 32767);
    w[2] = clamp(p.u15(18), 32767);
    w[5] = clamp(p.u15(16), 25000);
    w[6] = clamp(p.u15(17), 25000);
    w[3] = clamp(p.u16(0), 0x10000);
    w[4] = clamp(p.pitch(22), 8192);
    w[8] = clamp(style, 10);
    vec![Send::update(slot, w)]
}

/// The `addc/subfe` idiom the compiler uses for "below `lo` counts as 0":
/// keeps `v` when the carry of `(v - lo) + (lo ^ 0x80000000)` is clear.
fn floor_gate(v: i32, lo: i32) -> i32 {
    let a = (v as u32).wrapping_sub(lo as u32);
    let (_, carry) = a.overflowing_add(lo as u32 ^ 0x8000_0000);
    if carry { 0 } else { v }
}

/// `0x824B23C8(skater)`: 1/0 state the seams handler reads (skater `+684`
/// unless the skater's controller `*(s + 16) + 72` is clear and a game list
/// at `*(*0x830CFDC4 + 660) + 16` names an active entry).
pub fn skater_mode(env: &mut Env) -> i32 {
    let ctrl = env.s32(16);
    if env.g8(ctrl.wrapping_add(72)) != 0 {
        return env.s32(684) as i32;
    }
    let g = env.g32(env.g32(0x830C_FDC4).wrapping_add(660));
    if g == 0 {
        return env.s32(684) as i32;
    }
    let mut n = env.g32(g + 16);
    while n != 0 {
        if env.g8(n + 72) != 0 {
            return (env.g32(n + 84) == 0) as i32;
        }
        n = env.g32(n + 4);
    }
    1
}

/// `0x824C1F18`: Class_Seams, four wrappers at `+52..+64` (component with
/// the skater state at `+32`). Keeps a rate-limited value at `+152`
/// (±100 per wrapper update toward skater `+204` × 1000).
pub fn seams(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let mut out = Vec::new();
    if env.g8(env.c32(16).wrapping_add(52)) == 0 {
        return out;
    }
    for k in 0..4u32 {
        let slot = 52 + 4 * k;
        let p = *env.params();
        let mut level = p.u15(1);
        if skater_mode(env) == 1 {
            level = p.u15(6);
        }
        let base = p.u16(0);
        let pitch = p.pitch(2);
        let other = p.u15(5);
        let low = p.u15(3);
        let high = p.u15(4);
        // 0x8209975C = 0.5, 0x8208EDA4 = 0.08, 0x821161A0 = 10000.
        let speed = unit((env.sf(208) - 0.5) * 0.08f32, 10000.0);
        let state = skater_mode(env);
        // 0x82256FE8 = 1000
        let target = fctiwz(env.sf(204) * 1000.0);
        let held = env.c32(152) as i32;
        let mut v = target;
        if target > held {
            if target - held > 100 {
                v = held + 100;
            }
        } else if target < held && held - target > 100 {
            v = held - 100;
        }
        env.set_c32(152, v as u32);
        let mag = if v < 0 { v.wrapping_neg() } else { v };
        // 0x821747FC = 32767.0
        let obj = env.c32(40);
        let o_level = fctiwz(f32::from_bits(env.g32(obj)) * 32767.0);
        let o_kind = env.g32(obj.wrapping_add(16)) as i32;
        let tuned = f32::from_bits(env.g32(env.attrib(env.c32(36), 0x2F29_F403_8486_3C8C)));
        let w = wrappers.words(slot, 20);
        w[0] = 32767;
        w[1] = clamp(level, 32767);
        w[2] = clamp(other, 32767);
        w[5] = clamp(low, 25000);
        w[6] = clamp(high, 25000);
        w[3] = clamp(base, 0x10000);
        w[4] = clamp(pitch, 8192);
        w[8] = clamp(speed, 10000);
        w[11] = clamp(state, 1);
        w[15] = clamp(mag, 1000);
        w[16] = clamp(o_level, 32767);
        w[17] = clamp((env.s8(340) != 0 || env.s8(339) != 0) as i32, 1);
        w[18] = clamp(fctiwz(tuned * 32767.0), 32767);
        w[12] = clamp((env.s8(333) != 0 || env.s8(334) != 0) as i32, 1);
        w[13] = clamp(o_kind, 15);
        out.push(Send::update(slot, w));
    }
    out
}

/// `0x824CC7D8`: Class_Flips at `+36` (skater state at `+32`). Released when
/// the trick id (skater `+348`, or 34 while the gated byte `+310` is set)
/// differs from component `+52` or nothing is active.
pub fn flips(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 36u32;
    let in_trick = env.s8(332);
    let mut trick = env.s32(348) as i32;
    let mut gated = 0u8;
    if in_trick == 0 && env.g8(env.c32(28).wrapping_add(72)) != 0 {
        gated = env.s8(310);
    }
    if gated != 0 {
        trick = 34;
    }
    let active = in_trick != 0 || gated != 0;
    if !(active && trick == env.c32(52) as i32) {
        if env.c32(slot as usize) != 0 {
            return vec![Send::release(slot)];
        }
        if !active {
            return Vec::new();
        }
    }
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    let axis = |env: &mut Env, value: f32, thr_key: u64, div_key: u64| -> i32 {
        let thr = env.tuning_i32(72, thr_key);
        let div = env.tuning_f32(72, div_key);
        let mut v = fctiwz(value.abs() / div * 1000.0);
        if v > 1000 {
            v = 1000;
        }
        floor_gate(v, thr)
    };
    let x = env.sf(480);
    let y = env.sf(484);
    let z = env.sf(488);
    let a = axis(env, x, 0x5C73_CF6A_0D50_C8D8, 0x1494_BB20_854C_155C);
    let b = axis(env, y, 0xEE15_7886_DE5D_3C97, 0x02D3_9586_635F_B1A3);
    let c = axis(env, z, 0x9A03_1662_5B63_CD99, 0x8EDB_ACCB_A6FE_46AD);
    // 0x820BD5C4 = 500.0
    let spin = fctiwz(env.sf(220) * 500.0);
    let p = *env.params();
    let extra = if env.g8(env.c32(16).wrapping_add(72)) != 0 { p.u15(6) } else { 0 };
    let held = env.c32(72) as i32;
    let w = wrappers.words(slot, 28);
    w[9] = clamp(a, 1000);
    w[8] = clamp(b, 1000);
    w[7] = clamp(c, 1000);
    w[10] = clamp(spin, 1000);
    w[4] = clamp(p.pitch(2), 8192);
    w[0] = clamp(p.u15(1), 32767);
    w[3] = clamp(p.u16(0), 0x10000);
    w[1] = 32767;
    w[2] = 0;
    w[5] = clamp(p.u15(3), 25000);
    w[6] = 0;
    w[16] = clamp((env.s8(224) == 0) as i32, 1);
    w[26] = clamp(extra, 32767);
    w[20] = clamp(p.u15(8), 32767);
    w[22] = clamp(p.u15(7), 32767);
    w[12] = clamp(held, 1000);
    vec![Send::update(slot, w)]
}

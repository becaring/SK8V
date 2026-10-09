//! `0x824C39E0`: Class_grind (component with skater state at `+32`, two
//! wrappers at `+36` and `+40`). The `+40` object exists while the grind
//! type (skater `+192`) is 0 and is created / released on type changes.
//! Grind start/stop one-shots (`0x824C3FC8`, `0x824C4138`) and the closing
//! `0x824C42A8` go through the game's own sound triggers, not AEMS.

use super::env::{Env, Send};
use super::{clamp, Wrappers};
use crate::ops::fctiwz;

/// AttribSys class of the per-surface grind collections, and the table of
/// collection keys per grind surface class at `0x82249F90`.
const GRIND_CLASS: u64 = 0x0498_61E8_F9A8_D16B;
const SURFACE_KEYS: u32 = 0x8224_9F90;

/// `0x82494E18(surface)`: grind surface class (`+16` of the surface table
/// entry; entry 94 when out of range).
pub fn surface_class(env: &mut Env, surface: i32) -> i32 {
    let index = if (0..94).contains(&surface) { surface as u32 } else { 94 };
    env.g32(env.tuning_at(64, 0x4CA6_0755_8B1C_F440, index).wrapping_add(16)) as i32
}

/// The grind class for skater surface `+692` (143 means none → 4).
fn grind_class(env: &mut Env) -> i32 {
    let s = env.s32(692) as i32;
    if s == 143 { 4 } else { surface_class(env, s) }
}

pub fn surface_collection(env: &mut Env, class: i32) -> u32 {
    let class = if class >= 14 { 4 } else { class };
    let key = env.g64(SURFACE_KEYS.wrapping_add(8 * class as u32));
    env.collection(GRIND_CLASS, key)
}

/// `0x824C2D00(class, mode)`: level factor for the grind mode (1.0 for
/// modes above 3; the 0.0 default when the surface has no collection).
pub fn mode_factor(env: &mut Env, class: i32, mode: i32) -> f32 {
    let c = surface_collection(env, class);
    if mode as u32 > 3 {
        return 1.0;
    }
    if c == 0 {
        return f32::from_bits(env.g32(0x830D_0850));
    }
    let key = [0x6996_9AF1_BE6B_B367, 0x0555_484D_6D4A_3128, 0xC219_83A2_160E_D3AC, 0x69A5_FC53_091E_D1F8][mode as usize];
    f32::from_bits(env.g32(env.attrib(c, key)))
}

/// `0x824AF8C8(wrapper, level, class, mode, volume, a, b, c, d, e)`: the
/// create words.
#[allow(clippy::too_many_arguments)]
pub fn create_words(level: i32, class: i32, mode: i32, volume: i32, a: i32, b: i32, c: i32, d: i32, e: i32) -> Vec<i32> {
    vec![
        0,
        32767,
        0,
        0,
        0,
        25000,
        0,
        clamp(level, 10000),
        1024,
        clamp(class, 14),
        clamp(mode, 3),
        clamp(volume, 32767),
        clamp(a, 1),
        clamp(b, 1),
        clamp(c, 1),
        clamp(d, 32767),
        clamp(e, 32767),
    ]
}

pub fn update(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let mut out = Vec::new();
    if env.s8(341) == 0 || env.c32(36) == 0 {
        return out;
    }
    let kind = env.s32(192) as i32;
    let mode = match kind {
        1 | 2 | 4 => 0,
        5 => 3,
        _ => 2,
    };
    if kind != env.c32(56) as i32 {
        if kind == 0 {
            if env.c32(40) == 0 {
                let class = grind_class(env);
                let c = surface_collection(env, class);
                // 0x821747FC = 32767.0
                let volume = if c == 0 { 0.0 } else { f32::from_bits(env.g32(env.attrib(c, 0x5807_0BF5_1180_9903))) };
                let volume = fctiwz(volume * 32767.0);
                let ctrl = env.c32(16);
                let flagged = env.g8(ctrl.wrapping_add(72)) != 0;
                let (a, d) = if flagged { (1, env.params().u15(6)) } else { (0, 0) };
                let g = env.g32(0x830C_FDC4);
                let mut b = 0;
                if env.g8(g.wrapping_add(564)) != 0 {
                    let key = env.g64(g.wrapping_add(568));
                    let c2 = env.collection(0x11A6_3187_8B23_9355, key);
                    if c2 != 0 && env.g8(env.attrib(c2, 0xC317_F204_5035_CD24)) != 0 {
                        b = 1;
                    }
                }
                let c_flag = (flagged && env.g32(ctrl.wrapping_add(64)) == 0) as i32;
                let e = env.g32(env.tuning(140, 0xD489_344C_EDEE_5036)) as i32;
                let level = env.c32(140) as i32;
                let words = create_words(level, class, 1, volume, b, c_flag, a, d, e);
                *wrappers.words(40, 17) = words.clone();
                env.set_c32(40, 1);
                out.push(Send::create(40, &words));
            }
        } else if env.c32(56) == 0 && env.c32(40) != 0 {
            out.push(Send::release(40));
            env.set_c32(40, 0);
        }
        env.set_c32(56, kind as u32);
    }
    let class = grind_class(env);
    let factor = mode_factor(env, class, mode);
    let p = *env.params();
    let level = fctiwz(p.u15(1) as f32 * factor);
    let base = p.u16(0);
    let pitch = p.pitch(2);
    for (k, slot) in [36u32, 40].into_iter().enumerate() {
        if env.c32(slot as usize) == 0 {
            continue;
        }
        let held = env.c32(140) as i32;
        let other = p.u15(5);
        let extra = if env.g8(env.c32(16).wrapping_add(72)) != 0 { p.u15(6) } else { 0 };
        let w = wrappers.words(slot, 17);
        w[7] = clamp(held, 10000);
        w[0] = 32767;
        w[1] = clamp(level, 32767);
        w[2] = clamp(other, 32767);
        w[15] = clamp(extra, 32767);
        w[5] = clamp(p.u15(3), 25000);
        w[6] = clamp(p.u15(4), 25000);
        w[3] = clamp(base, 0x10000);
        w[4] = clamp(pitch, 8192);
        if k == 0 {
            w[10] = clamp(mode, 3);
        } else if kind == 0 {
            w[10] = 1;
        }
        out.push(Send::update(slot, w));
    }
    out
}

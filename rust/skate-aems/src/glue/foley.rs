//! Foley components (skater state at `+32`): cloth falls, trick cloth, body
//! slide.

use super::env::{Env, Send};
use super::{clamp, Wrappers};
use crate::ops::fctiwz;

/// `0x824DCA48`: c_cloth_falls at `+40` (level from component `+44`).
pub fn cloth_falls(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 40u32;
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    let p = *env.params();
    let held = env.c32(44) as i32;
    let w = wrappers.words(slot, 10);
    w[1] = clamp(p.u16(0), 65535);
    w[2] = clamp(p.pitch(1), 8192);
    w[3] = clamp(held, 1000);
    w[0] = 32767;
    w[7] = clamp(p.u15(2), 32767);
    vec![Send::update(slot, w)]
}

/// The trick-cloth words both handlers send.
fn cloth_words(env: &mut Env, wrappers: &mut Wrappers, slot: u32) -> Vec<Send> {
    let p = *env.params();
    let w = wrappers.words(slot, 11);
    w[0] = 32767;
    w[4] = 25000;
    w[5] = 0;
    w[6] = 0;
    w[7] = clamp(p.u15(4), 32767);
    w[1] = clamp(p.u16(0), 65535);
    w[2] = clamp(p.pitch(5), 8192);
    vec![Send::update(slot, w)]
}

/// `0x824CCE48`: cloth_trick at `+40`; released while skater `+676` is set
/// or when no trick is running (`+348` = -1 and `+332` clear).
pub fn cloth_trick_a(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 40u32;
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    if env.s8(676) != 0 || (env.s32(348) as i32 == -1 && env.s8(332) == 0) {
        return vec![Send::release(slot)];
    }
    cloth_words(env, wrappers, slot)
}

/// `0x824CCFE8`: cloth_trick at `+44`; released while skater `+676` is set.
pub fn cloth_trick_b(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 44u32;
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    if env.s8(676) != 0 {
        return vec![Send::release(slot)];
    }
    cloth_words(env, wrappers, slot)
}

/// `0x824DC2B0(this, &class, &level, &touching)`: the skater's six contact
/// slots (`+528..+548` forces, `+560..+580` surfaces, `+593` sliding).
/// Returns (surface class 0..4, level 0..1000ish, any contact).
pub fn body_contacts(env: &mut Env) -> (i32, i32, bool) {
    let mut touching = false;
    let mut sliding = false;
    let mut surface = 0i32;
    for k in 0..6usize {
        if env.sf(528 + 4 * k).abs() > 0.0 {
            touching = true;
            if env.s8(593) != 0 {
                sliding = true;
            }
            let s = env.s32(560 + 4 * k) as i32;
            if s != 0 && (k < 2 || surface == 0) {
                surface = s;
            }
        }
    }
    let speed = env.sf(212);
    let scale = env.tuning_f32(36, 0x7871_71EC_D02D_BBC3);
    // 0x82256FE8 = 1000
    let level = fctiwz(speed / scale * 1000.0);
    let mut index = surface.wrapping_sub(1);
    if surface == 0 || !(0..=143).contains(&index) {
        index = 143;
    }
    let class = if index == 143 {
        2
    } else if sliding {
        4
    } else if index < 94 {
        env.g32(env.tuning_at(64, 0x4CA6_0755_8B1C_F440, index as u32).wrapping_add(40)) as i32
    } else {
        env.g32(env.tuning_at(64, 0x4CA6_0755_8B1C_F440, 94).wrapping_add(40)) as i32
    };
    (class, level, touching)
}

/// `0x824DC578`: c_body_slide at `+60`.
pub fn body_slide(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let slot = 60u32;
    if env.c32(slot as usize) == 0 {
        return Vec::new();
    }
    let (class, level, _) = body_contacts(env);
    let scale = env.tuning_f32(36, 0x5D2F_244E_82CF_7255);
    let rub = fctiwz(env.sf(328) / scale * 1000.0);
    let p = *env.params();
    let flag = env.s8(676) as i32;
    let w = wrappers.words(slot, 12);
    w[0] = 32767;
    w[1] = clamp(p.u16(0), 65535);
    w[2] = clamp(p.pitch(6), 8192);
    w[3] = clamp(level, 1000);
    w[7] = clamp(p.u15(5), 32767);
    w[8] = clamp(class, 4);
    w[10] = clamp(rub, 1000);
    w[9] = clamp(flag, 1);
    vec![Send::update(slot, w)]
}

/// `0x82494F58(surface)`: the footstep class of a surface (element
/// `surface` of the surface table, 94 when out of range; its `+24`).
pub fn footstep_class(env: &mut Env, surface: i32) -> i32 {
    let index = if (0..94).contains(&surface) { surface as u32 } else { 94 };
    env.g32(env.tuning_at(64, 0x4CA6_0755_8B1C_F440, index).wrapping_add(24)) as i32
}

/// `0x824EAEA8`: playercharacter_footstep, one wrapper per foot (`+36` and
/// `+220`; both must exist). Per foot: flag byte `+52`/`+236`, levels
/// `+420`/`+424` and `+412`/`+416`, contact `+464`/`+468`, surface
/// `+56`/`+240`.
pub fn player_footsteps(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    if env.c32(220) == 0 || env.c32(36) == 0 {
        return Vec::new();
    }
    let gait = fctiwz(env.sf(796));
    let pace = env.s32(740) as i32;
    let mut out = Vec::new();
    for (slot, flag, a, b, contact, surface) in [(36u32, 52, 420, 412, 464, 56), (220, 236, 424, 416, 468, 240)] {
        let p = *env.params();
        let w0 = clamp(p.u16(0), 65535);
        let w1 = clamp(p.pitch(1), 8192);
        let low = clamp(p.u15(4), 25001);
        let high = clamp(p.u15(5), 25001);
        let w6 = clamp(p.u15(6), 32767);
        let w7 = clamp(p.u15(2), 32767);
        let flag = clamp((env.c8(flag) != 0) as i32, 1);
        let a = clamp(env.c32(a) as i32, 1000);
        let b = clamp(env.c32(b) as i32, 1000);
        let touching = clamp((env.c32(contact) as i32 > 0 || env.c32(444) != 0) as i32, 1);
        let stance = (env.s32(300) as i32).clamp(1, 4);
        let gait = gait.clamp(1, 99);
        let shared = clamp(env.c32(408) as i32, 1000);
        let s = env.c32(surface) as i32;
        let class = footstep_class(env, s).clamp(1, 7);
        let pace = pace.clamp(1, 5);
        let mut tuned = [0i32; 6];
        for (k, t) in tuned.iter_mut().enumerate() {
            *t = clamp(env.g32(env.tuning_at(92, 0x6364_64FB_AD0D_71A3, k as u32)) as i32, 32767);
        }
        let w = wrappers.words(slot, 25);
        w[0] = 32767;
        w[1] = w0;
        w[2] = w1;
        w[4] = low;
        w[5] = high;
        w[6] = w6;
        w[7] = w7;
        w[8] = flag;
        w[9] = a;
        w[10] = b;
        w[11] = touching;
        w[12] = stance;
        w[13] = gait;
        w[14] = shared;
        w[15] = 1;
        w[16] = class;
        w[17] = pace;
        w[18..24].copy_from_slice(&tuned);
        out.push(Send::update(slot, w));
    }
    out
}

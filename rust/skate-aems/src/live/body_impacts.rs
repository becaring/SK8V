//! The board foley component's world collision events (`0x82486EF0`, played
//! by `super::collisions`): the skater's body hitting things (`0x824BC188`),
//! the board hitting things (`0x824BD000`), the landing (`0x824BA630`'s
//! impact) and the grind start (`0x824BB0E0`).
//!
//! Component fields (the game's offsets): `+124` grind start cooldown,
//! `+260 + 4k` body region cooldowns, `+292` board impact cooldown,
//! `+296..+336` the body pseudo-surface thresholds (`0x824BBE40`, root 36),
//! `+406..+409` crash flags and `+412`/`+416` their timers, `+420..+422`
//! the replay and speech latches, `+444 + 4·type` the local player's impact
//! counts, `+460..+472` the running impact statistics. The flags, counts
//! and statistics feed the speech system, which is not ported.
//!
//! Skater fields: `+496 + 4k` a body region's normalized impulse (the
//! four-frame maximum), `+560 + 4k` its material (+1, 0 none), `+592`/`+593`
//! specific contacts, `+596`/`+600` group-8 and other-skater forces, `+220`
//! the cooldown step, `+660`/`+668` the board's surface and impact level,
//! `+676`/`+677` crash states, `+716` board off, `+192` grind family, `+228`
//! grind entry speed, `+692` the grind surface.
//!
//! Off in free skate (documented, not ported): the replay and network
//! playback paths (`0x824BCEB0`, inputs 7 and 8, `+420`), the
//! game-mode pseudo-surface (global `+859`: `0x824BFB70` gives 143, so the
//! fourth body event and its gate `0x824BFBE0` never run) and the speech
//! calls (`0x824BDA90`, `0x824BF5F8`).

use super::collisions::{Collisions, Event};
use super::driver::Component;
use super::frame::SkaterImage;
use super::impacts::{self, NONE};
use super::splices::Splices;
use super::{EMITTER_BOARD, EMITTER_BODY};
use super::tuning::Tuning;
use crate::glue::fsel;
use crate::ops::{fctiwz, fmadds};

fn word(t: &Tuning, root: u32, key: u64) -> u32 {
    t.g32(t.tuning_at(root, key, 0))
}

fn float(t: &Tuning, root: u32, key: u64) -> f32 {
    f32::from_bits(word(t, root, key))
}

fn local(c: &Component, t: &Tuning) -> bool {
    t.g8(c.c32(28).wrapping_add(72)) != 0
}

/// Every producer here posts with byte `+40` clear. The event position is
/// the skater's (`+48`: the body regions, the grind start) or the board's
/// (`+144`: the board impacts, the landing).
fn post(col: &mut Collisions, sp: &mut Splices, emitter: usize, surfaces: [i32; 2], types: [i32; 2], levels: [i32; 2]) {
    col.post(Event { surfaces, types, levels, alternate: false, emitter }, sp);
}

/// `0x824BBE40`, at construction: the body pseudo-surface thresholds.
pub fn initialize(c: &mut Component, t: &Tuning) {
    for (off, key) in [(296, 0xAFA4_B509_0F1B_CF36), (300, 0x0096_0FED_B3EF_EE9C),
        (304, 0xD120_03A6_0E98_7B9D), (308, 0xFA2A_A5A0_C048_1D00),
        (312, 0x3695_327C_FB5E_1AC3), (316, 0x35FE_E8A9_5523_D812),
        (320, 0x076E_9081_CA17_59E9), (324, 0xDF53_9915_EB7E_883E),
        (328, 0x8E30_25BA_A686_F721), (332, 0x504D_3B73_5059_72D4),
        (336, 0x08B0_4D2D_8F89_B9A7)] {
        c.set32(off, word(t, 36, key));
    }
}

/// `0x824BC188`: each body region that took an impulse and is off its
/// cooldown raises the impact of the body part against the material it
/// hit, plus the body part's own sounds.
pub fn body(c: &mut Component, s: &SkaterImage, t: &Tuning, col: &mut Collisions, sp: &mut Splices) {
    c.inputs[7] = 0;
    // Input 8 and `+420` follow replay playback, which is off.
    c.inputs[8] = 0;
    c.set8(421, 0);
    let crashed = s.r8(676) != 0;
    if !crashed {
        c.set8(422, 1);
    } else if s.r8(677) != 0 {
        return;
    }
    for k in 0..6 {
        let f = s.rf(496 + 4 * k);
        let cooldown = 260 + 4 * k;
        if f > 0.0 && !(c.rf(cooldown) > 0.0) {
            region(c, s, t, col, sp, k, f, crashed);
        }
        let remaining = c.rf(cooldown);
        if remaining > 0.0 {
            let step = s.rf(220);
            c.setf(cooldown, remaining - if step <= 1.0 { step } else { 1.0 });
        }
    }
    for off in [412, 416] {
        let n = c.c32(off) as i32;
        c.set32(off, if n > 0 { n - 1 } else { n } as u32);
    }
}

/// `0x824BCBA0`: region `k`'s body pseudo-surface, its extra surface
/// (`sp112`) and its contact surface (`sp120`).
fn pseudo_surfaces(c: &mut Component, s: &SkaterImage, t: &Tuning, k: usize, f: f32) -> (i32, i32, i32) {
    match k {
        0 => {
            if f > float(t, 44, 0xFC2C_ACAA_802A_674F) && s.r8(676) != 0 {
                c.set8(406, 1);
            }
            (97, NONE, if s.r8(593) != 0 { 112 } else { 110 })
        }
        1 => (98, 109, 110),
        2 | 3 => (100, 107, 111),
        _ => (99, 108, 111),
    }
}

#[allow(clippy::too_many_arguments)]
fn region(c: &mut Component, s: &SkaterImage, t: &Tuning, col: &mut Collisions, sp: &mut Splices,
    k: usize, f: f32, crashed: bool) {
    if s.r8(592) != 0 && crashed {
        c.set8(407, 1);
    }
    if s.rf(596) > float(t, 44, 0x7503_A1AD_356B_FCCD) {
        if crashed {
            c.set8(408, 1);
        }
        c.set32(412, 5);
    }
    if s.rf(600) > float(t, 44, 0x359E_C8EF_5259_8AA8) {
        if crashed {
            c.set8(409, 1);
        }
        c.set32(416, 5);
    }
    let (body, extra, contact) = pseudo_surfaces(c, s, t, k, f);
    let material = match s.r32(560 + 4 * k) as i32 {
        0 => NONE,
        v => match v.wrapping_sub(1) {
            w if !(0..=NONE).contains(&w) => NONE,
            w => w,
        },
    };
    let local = local(c, t);
    let count = |c: &mut Component, ty: i32| {
        if local && ty < 3 {
            let off = 444 + 4 * ty as usize;
            c.set32(off, c.c32(off).wrapping_add(1));
        }
    };
    let (mut body_low, mut body_high) = (0.0, 0.0);
    let body_type = if body < NONE { impacts::strength(t, body, f, &mut body_low, &mut body_high) } else { 3 };
    count(c, body_type);
    let (mut low, mut high) = (0.0, 0.0);
    let material_type = if material < NONE { impacts::strength(t, material, f, &mut low, &mut high) } else { 3 };
    count(c, material_type);
    if body_type == 3 && material_type == 3 {
        return;
    }
    c.setf(260 + 4 * k, word(t, 36, 0x6DD8_5F43_C1B6_E6AA) as i32 as f32);
    let level_a = if body < NONE {
        impacts::level(t, body, material, body_type, body_low, body_high, f)
    } else { 0 };
    let level_b = if material < NONE {
        impacts::level(t, material, body, material_type, low, high, f)
    } else { 0 };
    post(col, sp, EMITTER_BODY, [body, material], [body_type, material_type], [level_a, level_b]);
    // The crash speech (`0x824BF5F8`) is not ported; its latch is.
    if c.image[422] != 0 && s.r8(676) != 0 {
        c.set8(422, 0);
    }
    secondary(col, sp, t, [body, material], [body_type, material_type], [level_a, level_b]);
    let n = c.c32(472) as i32;
    if n == 0 {
        c.setf(460, f);
        c.set32(472, 1);
    } else {
        c.set32(472, (n + 1) as u32);
        c.setf(460, fmadds(n as f32, c.rf(460), f) / (n + 1) as f32);
    }
    if c.rf(460) > c.rf(464) {
        c.setf(464, c.rf(460));
    }
    if f > c.rf(468) {
        c.setf(468, f);
    }
    if extra != NONE {
        let level = if body < NONE {
            impacts::level(t, body, material, 0, body_low, body_high, f)
        } else { 0 };
        post(col, sp, EMITTER_BODY, [extra, NONE], [0, 0], [level, 0]);
    }
    if contact != NONE {
        let (ty, low, high) = thresholds(c, t, body, contact, f);
        if !(ty >= 1 && f > high) && ty != 3 {
            let level = if contact < NONE { impacts::level(t, contact, material, ty, low, high, f) } else { 0 };
            post(col, sp, EMITTER_BODY, [contact, NONE], [ty, 0], [level, 0]);
        }
    }
}

/// The hard impacts' second event: strength 2 plays again as 1, scaled by
/// each surface's level scale (`0x82496F50`).
fn secondary(col: &mut Collisions, sp: &mut Splices, t: &Tuning, surfaces: [i32; 2], types: [i32; 2], levels: [i32; 2]) {
    let types = types.map(|ty| if ty == 2 { 1 } else { 3 });
    if types == [3, 3] {
        return;
    }
    let a = fctiwz(impacts::scale(t, surfaces[0]) * levels[0] as f32);
    let b = fctiwz(impacts::scale(t, surfaces[1]) * levels[1] as f32);
    post(col, sp, EMITTER_BODY, surfaces, types, [a, b]);
}

/// `0x824BCCF8`: the contact surface's strength and range, from the
/// component's thresholds or the surface's own.
fn thresholds(c: &Component, t: &Tuning, body: i32, surface: i32, f: f32) -> (i32, f32, f32) {
    let (mut ty, mut low, mut high) = (3, 0.0, 0.0);
    let mut above = |at: usize, strength: i32| {
        if f > c.rf(at) {
            low = c.rf(at);
            high = c.rf(at + 4);
            ty = strength;
            true
        } else {
            false
        }
    };
    match surface {
        102..=106 => ty = impacts::strength(t, surface, f, &mut low, &mut high),
        110 if body == 97 => { above(304, 1); }
        110 if body == 98 => { above(296, 1); }
        111 => { above(312, 1); }
        112 if !above(328, 1) => { above(320, 0); }
        _ => {}
    }
    (ty, low, high)
}

/// `0x824BD000`: the board hitting something, off its cooldown.
pub fn board(c: &mut Component, s: &SkaterImage, t: &Tuning, col: &mut Collisions, sp: &mut Splices) {
    let f = s.rf(668);
    if f > 0.0 && !(c.rf(292) > 0.0) {
        let deck = if s.r8(716) != 0 || s.r8(676) != 0 { 113 } else { 95 };
        let surface = s.r32(660) as i32;
        let (mut deck_low, mut deck_high) = (0.0, 0.0);
        let deck_type = impacts::strength(t, deck, f, &mut deck_low, &mut deck_high);
        let (mut low, mut high) = (0.0, 0.0);
        let surface_type = if surface < NONE { impacts::strength(t, surface, f, &mut low, &mut high) } else { 3 };
        if deck_type != 3 || surface_type != 3 {
            c.setf(292, word(t, 36, 0x27D3_C5DC_3282_B59D) as i32 as f32);
            let level_a = impacts::level(t, deck, surface, deck_type, deck_low, deck_high, f);
            let level_b = if surface < NONE {
                impacts::level(t, surface, deck, surface_type, low, high, f)
            } else { 0 };
            post(col, sp, EMITTER_BOARD, [deck, surface], [deck_type, surface_type], [level_a, level_b]);
            secondary(col, sp, t, [deck, surface], [deck_type, surface_type], [level_a, level_b]);
        }
    }
    let remaining = c.rf(292);
    if remaining > 0.0 {
        let step = s.rf(220);
        c.setf(292, remaining - if step <= 1.0 { step } else { 1.0 });
    }
}

/// `0x824BA630`'s impact: the first landed wheel's surface against the
/// board, by the airborne time; then mix input 6 for the local player.
pub fn landing(c: &mut Component, s: &SkaterImage, t: &Tuning, col: &mut Collisions, sp: &mut Splices) {
    let mut wheel = NONE;
    for i in 0..4 {
        if s.r8(464 + i) != 0 {
            wheel = s.r32(620 + 4 * i) as i32;
            if wheel != NONE {
                break;
            }
        }
    }
    let x = c.rf(340) / float(t, 24, 0x6D68_BC2D_1A23_C29A);
    let x = fsel(-x, 0.0, x);
    let air = fsel(1.0 - x, x, 1.0);
    if !((0..NONE).contains(&wheel) && impacts::landing_event(t, wheel)) {
        return;
    }
    let board = word(t, 24, 0x85FD_C8BF_696B_CA5C) as i32;
    let threshold = float(t, 24, 0x3462_CBB1_6DCA_696E);
    let (ty, low, high) = if air >= threshold {
        (1, threshold, float(t, 24, 0x5904_95E4_20B3_99E5))
    } else {
        (0, 0.0, threshold)
    };
    let wheel_level = impacts::level(t, wheel, board, ty, low, high, air);
    let board_level = if board < NONE { impacts::level(t, board, wheel, ty, low, high, air) } else { 0 };
    if wheel_level != 0 && board_level != 0 {
        // The levels go out crossed, as natively.
        let a = fctiwz(wheel_level as f32 * float(t, 24, 0x31DE_EF8F_A219_950F));
        let b = fctiwz(board_level as f32 * float(t, 24, 0x0EC6_EEF5_366F_EA85));
        post(col, sp, EMITTER_BOARD, [board, wheel], [ty, ty], [a, b]);
    }
    if local(c, t) {
        c.inputs[6] = 32767;
    }
}

/// `0x824BB0E0`: the grind start, off its cooldown (`+124`, counted down
/// with the instances).
pub fn grind_start(c: &mut Component, s: &SkaterImage, t: &Tuning, col: &mut Collisions, sp: &mut Splices) {
    if c.rf(124) > 0.0 {
        return;
    }
    let surface = match s.r32(692) as i32 { NONE => 10, v => v };
    let board = if matches!(s.r32(192), 1 | 2 | 5) { 95 } else { 96 };
    let speed = s.rf(228);
    let threshold = float(t, 40, 0x086B_66C3_D4FF_EE8F);
    let (ty, low, high) = if speed > threshold {
        (1, threshold, float(t, 40, 0xB2AC_AFDB_CD96_3C93))
    } else {
        (0, 0.0, threshold)
    };
    let a = impacts::level(t, board, surface, ty, low, high, speed);
    let b = if surface < NONE { impacts::level(t, surface, board, ty, low, high, speed) } else { 0 };
    post(col, sp, EMITTER_BODY, [board, surface], [ty, ty], [a, b]);
    // 0x8209975C.
    c.setf(124, 0.5);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_thresholds_follow_component_fields() {
        let mut c = Component::new("foot_drag", 512, vec![]);
        let t = Tuning::default();
        c.setf(296, 0.5);
        c.setf(304, 0.2);
        c.setf(308, 0.6);
        c.setf(320, 0.1);
        c.setf(324, 0.3);
        c.setf(328, 0.5);
        c.setf(332, 0.9);
        assert_eq!(thresholds(&c, &t, 97, 110, 0.3), (1, 0.2, 0.6));
        assert_eq!(thresholds(&c, &t, 98, 110, 0.3).0, 3);
        assert_eq!(thresholds(&c, &t, 97, 112, 0.2), (0, 0.1, 0.3));
        assert_eq!(thresholds(&c, &t, 97, 112, 0.7), (1, 0.5, 0.9));
        assert_eq!(thresholds(&c, &t, 97, 108, 0.7).0, 3);
    }

    #[test]
    fn body_region_cooldown_gates_repeat_impacts() {
        let mut c = Component::new("foot_drag", 512, vec![]);
        let mut t = Tuning::default();
        t.set_tuned(36, 0x6DD8_5F43_C1B6_E6AA, 0, 2);
        let controller = t.alloc(80);
        c.set32(28, controller);
        let mut col = Collisions::default();
        let mut sp = Splices::default();
        let mut s = SkaterImage::default();
        s.wf(220, 0.5);
        s.wf(496, 0.4);
        s.w32(560, 6);
        body(&mut c, &s, &t, &mut col, &mut sp);
        // Zero thresholds: both sides are hard impacts, which play again
        // as medium ones.
        let events: Vec<Event> = col.events().copied().collect();
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].surfaces, events[0].types), ([97, 5], [2, 2]));
        assert_eq!(events[1].types, [1, 1]);
        // Cooldown 2, stepped once by 0.5.
        assert_eq!(c.rf(260), 1.5);
        let posted = col.active();
        body(&mut c, &s, &t, &mut col, &mut sp);
        assert_eq!(col.active(), posted);
        assert_eq!(c.rf(260), 1.0);
    }
}

//! The footsteps component's splice sounds (vtable `0x822FCF98`, mix-map
//! bank 9): the local skater's foot strikes, the jump-off scuffs and the
//! airborne/landing whooshes. The AEMS footstep objects (`+36`/`+220`) are
//! `super::foley_life` and `crate::glue::foley::player_footsteps`.
//!
//! Each foot owns a wrapper (`0x824D7CE8`; A at `+36`, B at `+220`) of four
//! 40-byte sound slots (`+24`, `+64`, `+104`, `+144`): `+0` the instance,
//! `+4..+20` the filter attributes (high-pass, low-pass, peak frequency,
//! gain, Q), `+24` the gain, `+32` the slot's filter submix
//! (`Splices::chain`). On a foot's rising contact edge (`0x824E9FD8`) the
//! first two slots play the surface's step layers, filtered by the
//! envelope the sound names, and the last two the pace layers.
//!
//! Not ported here: other skaters' foot sounds (`0x824EBA08`), water
//! splashes (`0x824EBB58`, `+484`; no water state is modelled), the aux
//! send levels and pan words the wrappers keep for the submix's Sen0/Pn21
//! (`+8`/`+12`; routing and placement, the host's).

use super::driver::Component;
use super::frame::SkaterImage;
use super::impacts;
use super::splices::{Splices, SLOTS};
use super::tuning::Tuning;
use crate::splice::Params;

/// `0x822F8898`, `0x822F890C`, `0x822F8C64`.
const LEVEL: f32 = 1.0 / 32767.0;
const PITCH: f32 = 1.0 / 4096.0;
const PAN: f32 = 360.0 / 65536.0;

/// The two feet: wrapper, contact flag, previous flag, surface.
const FEET: [(usize, usize, usize, usize); 2] = [(36, 52, 54, 56), (220, 236, 238, 240)];
/// Sound slot offsets in a wrapper, in creation order.
const SLOT_AT: [usize; 4] = [24, 64, 104, 144];

/// Envelope collections (class `370AF2704BFA6866`); layout `+0..+20`.
const ENVELOPE: u64 = 0x370A_F270_4BFA_6866;
const ENVELOPE_FIELDS: [u64; 6] = [0x411D_1D4C_E3AF_A346, 0xDE9B_F5C1_0AB2_C866, 0x7D32_94EC_1C44_3CBD,
    0x805A_BC23_217F_AC46, 0x79D8_AB26_EA21_7567, 0x4E0F_0083_8F47_2B4A];

fn local(c: &Component, t: &Tuning) -> bool {
    t.g8(c.c32(28).wrapping_add(72)) != 0
}

fn word(t: &Tuning, root: u32, key: u64) -> i32 {
    t.g32(t.tuning_at(root, key, 0)) as i32
}

/// `0x82493E60`: a step layer's sound and splice slot (`layer` 0 or 1),
/// -1 for none.
fn step_sound(t: &Tuning, band: bool, stance: i32, layer: i32, surface: i32, slot: &mut i32) -> i32 {
    let kind = impacts::kind(t, surface);
    if kind == 7 || kind == 5 {
        *slot = if kind == 7 { 0 } else { 1 };
        let hard = stance > 1;
        let key = match (kind, hard, band, layer != 0) {
            (7, true, _, false) => 0xFFA7_D4C0_4566_5743,
            (7, true, _, true) => 0xB849_8AE9_DE4D_37FB,
            (7, false, false, false) => 0xA925_081B_59FD_4279,
            (7, false, false, true) => 0xC747_4D8E_4FF9_2CD3,
            (7, false, true, false) => 0xB990_1274_0033_005F,
            (7, false, true, true) => 0x6426_2DAC_FE5A_8566,
            (_, true, _, false) => 0x40DC_16AB_4521_33CE,
            (_, true, _, true) => 0xBE9E_B998_C9A0_D6DF,
            (_, false, false, false) => 0x72E9_E3D5_F10B_C458,
            (_, false, false, true) => 0xE939_BD39_0E23_B51D,
            (_, false, true, false) => 0x2D4A_B518_B123_DFCC,
            (_, false, true, true) => 0x07D7_7C3F_B9CA_800E,
        };
        return word(t, 92, key);
    }
    if layer != 0 || surface >= impacts::NONE {
        return -1;
    }
    let (s, id) = impacts::footstep(t, surface, band as i32, stance);
    if id == -1 {
        return -1;
    }
    *slot = s;
    id
}

/// `0x82493690`: a pace layer's sound (`layer` 0: splice slot 7 by surface
/// kind 1..7; 1: slot 0, kinds 3 and 4 only) by pace (`740`, odd or even)
/// and impact band (2 or not), -1 for none.
fn pace_sound(t: &Tuning, band: i32, surface: i32, pace: i32, layer: i32, slot: &mut i32) -> i32 {
    const ODD_HARD: [u64; 7] = [0xF8DC_5242_ECDD_251E, 0x2F15_A576_57F4_6419, 0x64C5_D156_8CC2_30C7,
        0xB17D_B39A_B5D8_E2A9, 0x2A48_57B6_917A_6DA5, 0x0AE1_D8B3_B204_AD4F, 0x84DC_3A5A_C20F_8BAB];
    const ODD: [u64; 7] = [0xAB20_E146_838D_7C78, 0x3BBE_87D4_AD06_3373, 0x4C9E_A044_EBE2_FDF1,
        0xB7E9_3C9B_41D3_623E, 0x04B9_1290_FBDC_9B0C, 0x2EAB_3B3B_F179_220E, 0x8782_6DD0_C10E_F899];
    const EVEN_HARD: [u64; 7] = [0x8555_FF5F_7BBC_9A91, 0xC73B_4B7C_3E75_036A, 0xBAA1_E01C_C885_DDB8,
        0xFED8_9B4D_0E22_EE7F, 0xE327_E0FE_1D34_B2C9, 0xBD7C_C72E_ED7A_1F8E, 0x35B7_9DF4_BE6A_E58E];
    const EVEN: [u64; 7] = [0x834C_9CF1_9DCD_F2E9, 0x26E8_5F53_65DC_A946, 0x9B5C_C5F7_18BB_378C,
        0x8EFD_80E6_E75E_6030, 0x8B4D_0DCD_88C6_9351, 0xA41C_2767_8531_2BBC, 0x0730_458A_1980_12BB];
    let kind = impacts::kind(t, surface);
    let odd = matches!(pace, 1 | 3 | 5);
    let hard = band == 2;
    if layer == 0 {
        *slot = 7;
        let n = kind.wrapping_sub(1) as u32;
        if n > 6 {
            return -1;
        }
        let table = match (odd, hard) {
            (true, true) => ODD_HARD,
            (true, false) => ODD,
            (false, true) => EVEN_HARD,
            (false, false) => EVEN,
        };
        return word(t, 92, table[n as usize]);
    }
    *slot = 0;
    let key = match (odd, hard, kind) {
        (true, true, 4) => 0xA1EF_70EA_F73E_5EF7,
        (true, true, 3) => 0x530A_7EFC_3304_6E1E,
        (true, false, 4) => 0xCE7A_E03E_0CCC_6177,
        (true, false, 3) => 0x867E_209A_CD37_725B,
        (false, true, 4) => 0x3ACF_F1FA_0DD4_2376,
        (false, true, 3) => 0xCA06_A591_0C60_AB99,
        (false, false, 4) => 0x39A4_6D66_ECDC_F93F,
        (false, false, 3) => 0xB4B4_2D93_7519_4F70,
        _ => return -1,
    };
    word(t, 92, key)
}

/// `0x82493448`: the envelope collection key of sound `id` (root 92 array
/// `{id, .., key at +16}` by splice slot; a default key when absent).
fn envelope_key(t: &Tuning, id: i32, slot: i32) -> u64 {
    let array = if slot == 0 { 0x60B0_43CC_A6F2_11C3 } else { 0xCA94_5189_6540_51D3 };
    let n = t.root_record(92, array).map_or(0, |r| r.count);
    for i in 0..n {
        let e = t.tuning_at(92, array, i);
        if t.g32(e) as i32 == id {
            return (t.g32(e + 16) as u64) << 32 | t.g32(e + 20) as u64;
        }
    }
    0xD7ED_BD36_2D7D_2152
}

/// The envelope's six words (`+0` gain, `+4` Q, `+8` peak gain, `+12` peak
/// frequency, `+16` low-pass, `+20` high-pass).
fn envelope(t: &Tuning, id: i32, slot: i32) -> [f32; 6] {
    let c = t.collection(ENVELOPE, envelope_key(t, id, slot));
    ENVELOPE_FIELDS.map(|k| f32::from_bits(t.record(c, k).map_or(0, |r| t.g32(r.address))))
}

/// `0x824940F8`: the material's own step gain, when the surface is a plain
/// material with steps.
fn material_gain(t: &Tuning, surface: i32, stance: i32) -> Option<f32> {
    let kind = impacts::kind(t, surface);
    if kind == 7 || kind == 5 || surface >= impacts::NONE {
        return None;
    }
    if impacts::footstep(t, surface, 0, stance).1 == -1 {
        return None;
    }
    Some(impacts::footstep_gain(t, surface, stance))
}

/// `0x82494A68`: a filter submix for every wrapper slot without one.
fn chains(c: &mut Component, sp: &mut Splices) {
    for (wrapper, ..) in FEET {
        for at in SLOT_AT {
            if c.c32(wrapper + at + 32) == 0 {
                let chain = sp.chain();
                c.set32(wrapper + at + 32, chain);
            }
        }
    }
}

/// `0x82494550` (the filter attributes), then `0x82975700` into the slot's
/// submix and `0x82975A60`.
fn start(c: &mut Component, sp: &mut Splices, at: usize, bank: i32, id: i32, dt: f32) {
    let chain = c.c32(at + 32);
    sp.set_chain(chain, [4, 8, 12, 16, 20].map(|o| c.rf(at + o)));
    let bank = bank as i8;
    let h = if (0..SLOTS as i8).contains(&bank) { sp.create_into(bank as usize, id, chain) } else { 0 };
    c.set32(at, h);
    sp.play(h, 0, Params::at_play(dt));
}

/// Stops a wrapper's playing step layers (`0x82494B80`).
fn stop(c: &mut Component, sp: &mut Splices, wrapper: usize) {
    for at in SLOT_AT {
        let h = c.c32(wrapper + at);
        if h != 0 {
            sp.destroy(h);
            c.set32(wrapper + at, 0);
        }
    }
}

/// `0x824E9FD8`'s local part, one foot: the step layers and pace layers.
#[allow(clippy::too_many_arguments)] // the game's per-foot routine takes these
fn strike(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, foot: usize, band: bool, pace_band: i32, dt: f32) {
    let (wrapper, _, _, surface_at) = FEET[foot];
    stop(c, sp, wrapper);
    let stance = s.r32(300) as i32;
    let surface = c.c32(surface_at) as i32;
    let mut slot = -1;
    let id = step_sound(t, band, stance, 0, surface, &mut slot);
    if id != -1 {
        let e = envelope(t, id, slot);
        let at = wrapper + SLOT_AT[0];
        for (o, v) in [4, 8, 12, 16, 20].into_iter().zip([e[5], e[4], e[3], e[2], e[1]]) {
            c.setf(at + o, v);
        }
        // Foot A's surface for both feet, as natively.
        c.setf(at + 24, material_gain(t, c.c32(56) as i32, stance).unwrap_or(e[0]));
        start(c, sp, at, slot, id, dt);
        let id = step_sound(t, band, stance, 1, surface, &mut slot);
        if id != -1 {
            let e = envelope(t, id, slot);
            let at = wrapper + SLOT_AT[1];
            for (o, v) in [4, 8, 12, 16, 20, 24].into_iter().zip([e[5], e[4], e[3], e[2], e[1], e[0]]) {
                c.setf(at + o, v);
            }
            start(c, sp, at, slot, id, dt);
        }
    }
    let pace = s.r32(740) as i32;
    for (layer, at) in [(0, SLOT_AT[2]), (1, SLOT_AT[3])] {
        let id = pace_sound(t, pace_band, surface, pace, layer, &mut slot);
        if id != -1 {
            start(c, sp, wrapper + at, slot, id, dt);
        }
    }
}

/// `0x824E9FD8` after the AEMS objects: the local skater's foot strikes on
/// each foot's rising contact edge.
pub fn strikes(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    if !local(c, t) {
        return;
    }
    chains(c, sp);
    let speed = c.c32(408) as i32;
    let band = speed > word(t, 92, 0x729D_6290_FB6A_8E3B);
    let pace_band = if speed >= word(t, 92, 0xE822_37E2_D18B_5C5F) { 2 } else { 0 };
    for (foot, (_, now, prior, _)) in FEET.into_iter().enumerate() {
        if c.image[prior] == 0 && c.image[now] != 0 {
            strike(c, s, t, sp, foot, band, pace_band, dt);
        }
    }
}

/// `0x82494840`: the jump-off sounds by board speed (skater `+212`) against
/// root 136 `E12AF885D3C3A168[0..1]`: (slot 7 sound, slot 0 sound).
fn scuff_sounds(t: &Tuning, speed: f32) -> (i32, i32) {
    let limit = |i| f32::from_bits(t.g32(t.tuning_at(136, 0xE12A_F885_D3C3_A168, i)));
    let (a, b) = if !(speed <= limit(0)) {
        (0x9D6D_2863_CFE9_08C4, 0x6771_7E23_88A8_36ED)
    } else if !(speed <= limit(1)) {
        (0xEC33_99A4_9055_DD8D, 0xE420_F7DD_48E0_1E0E)
    } else {
        (0x6B61_C043_E53C_44CB, 0xF029_2A62_D280_EB40)
    };
    (word(t, 92, a), word(t, 92, b))
}

fn restart(c: &mut Component, sp: &mut Splices, at: usize, bank: usize, id: i32, dt: f32) {
    let h = c.c32(at);
    if h != 0 {
        sp.destroy(h);
        c.set32(at, 0);
    }
    let h = sp.create(bank, id);
    c.set32(at, h);
    sp.play(h, 0, Params::at_play(dt));
}

/// `0x824E9D10` (off the board, `+716`): each foot's rising contact edge
/// plays a scuff (A `+432`, B `+428`), and for the local skater still
/// holding the off-board flag (`+308`) a second one (A `+440`, B `+436`).
pub fn scuffs(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    let second = local(c, t) && s.r8(716) != 0 && s.r8(308) != 0;
    for ((_, now, prior, _), (first_at, second_at)) in FEET.into_iter().zip([(432, 440), (428, 436)]) {
        if !(c.image[prior] == 0 && c.image[now] != 0) {
            continue;
        }
        let (a, b) = scuff_sounds(t, s.rf(212));
        restart(c, sp, first_at, 7, a, dt);
        if second {
            restart(c, sp, second_at, 0, b, dt);
        }
    }
}

/// `0x824E9678`: the airborne whoosh (`+444`, on a fresh filtered state
/// `+718` off the board, a grind-scorable rise `+372`, or `armed`) and its
/// landing (`+448`) when the centre of mass starts to fall (`+100`).
pub fn whooshes(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, off_board: bool, armed: bool, dt: f32) {
    let filtered = s.r8(718) != 0;
    let fresh = off_board && filtered && c.image[405] == 0;
    let scorable = s.r8(372);
    let falling = s.rf(100) <= 0.0;
    let fall_edge = falling && c.c32(456) == 0;
    c.set32(456, falling as u32);
    let land = c.c32(444) != 0 && fall_edge;
    let rise = c.image[452] == 0 && scorable != 0;
    let params = Params { pan_scale: if local(c, t) { 1.0 } else { 0.0 }, ..Params::at_play(dt) };
    if fresh || rise || armed {
        let h = c.c32(444);
        if h != 0 {
            sp.destroy(h);
            c.set32(444, 0);
        }
        let ground = s.r32(304);
        c.set32(472, ground);
        let key = if fresh {
            0x200A_B824_13E0_FF3D
        } else {
            match ground {
                1 => 0x65E8_997F_265A_4191,
                2 => 0x76DE_6529_A45A_C896,
                _ => 0xFCBA_FB90_598F_3B83,
            }
        };
        let h = sp.create(7, word(t, 136, key));
        c.set32(444, h);
        sp.play(h, 0, params);
        c.set8(476, fresh as u8);
    }
    if land {
        for at in [444, 448] {
            let h = c.c32(at);
            if h != 0 {
                sp.destroy(h);
                c.set32(at, 0);
            }
        }
        let key = if c.image[476] != 0 {
            0xA3A1_BB4E_3ACD_6927
        } else {
            match c.c32(472) {
                1 => 0x9913_D205_D51E_5F24,
                2 => 0x8AA9_F40C_9CEA_BCA6,
                _ => 0xF0F4_6BA2_D527_E511,
            }
        };
        let h = sp.create(7, word(t, 136, key));
        c.set32(448, h);
        sp.play(h, 0, params);
    }
    c.set8(452, scorable);
    c.set8(405, filtered as u8);
}

/// Slot 10 (`0x824E9628`) after the AEMS updates: the strike gains
/// (`0x82494C08` from `0x824EAEA8`, mix output 7, or 11 on a plain
/// material), then the scuffs (`0x824EA9C8` output 3, `0x824EAC38` output
/// 12) and the whooshes (`0x824E9AB8` outputs 9 and 10).
pub fn instances(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    let p = c.params;
    let pan_scale = if local(c, t) { 1.0 } else { 0.0 };
    let base = Params { gain: 0.0, pitch: p.pitch(1) as f32 * PITCH, pan: p.u16(0) as f32 * PAN, dt, pan_scale, stretch: 1.0 };
    if local(c, t) {
        let output = if material_gain(t, c.c32(56) as i32, s.r32(300) as i32).is_some() { 11 } else { 7 };
        let g = p.u15(output) as f32 * LEVEL;
        for (wrapper, ..) in FEET {
            for at in [SLOT_AT[0], SLOT_AT[2], SLOT_AT[1], SLOT_AT[3]] {
                let h = c.c32(wrapper + at);
                if h == 0 {
                    continue;
                }
                if !sp.is_playing(h) {
                    sp.destroy(h);
                    c.set32(wrapper + at, 0);
                } else {
                    sp.update(h, Params { gain: c.rf(wrapper + at + 24) * g, pan: 0.0, ..base });
                }
            }
        }
    }
    for (offsets, output, keep) in [([432, 428], 3, false), ([440, 436], 12, false), ([444, 0], 9, true), ([448, 0], 10, false)] {
        let params = Params { gain: p.u15(output) as f32 * LEVEL, ..base };
        for at in offsets {
            let h = if at == 0 { 0 } else { c.c32(at) };
            if h == 0 {
                continue;
            }
            if sp.is_playing(h) {
                sp.update(h, params);
            } else if !keep {
                sp.destroy(h);
                c.set32(at, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::splices::{SLOT_COLLISIONS, SLOT_FOLEY, SLOT_METAL};
    use std::path::Path;

    #[test]
    fn a_strike_plays_through_its_filter_submix() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/audio-cache");
        let Ok(mut t) = Tuning::load(&dir) else { return eprintln!("SKIP: no audio cache") };
        let mut sp = Splices::default();
        for (slot, stem) in [(SLOT_COLLISIONS, "Skate_Collisions"), (SLOT_METAL, "Skate_Metal"), (SLOT_FOLEY, "sk8_foley")] {
            sp.load(&dir, slot, stem).unwrap();
        }
        let controller = t.alloc(80);
        t.w8(controller + 72, 1);
        t.w8(controller + 52, 1);
        let mut c = Component::new("footsteps", 512, vec![]);
        c.set32(16, controller);
        c.set32(28, controller);
        super::super::foley_life::init_footsteps(&mut c);
        c.params.words = Some([16384; 16]);
        let s = SkaterImage::default();
        // The first plain surface with steps: its envelope fills slot 0.
        let surface = (0..impacts::NONE)
            .find(|&n| impacts::kind(&t, n) != 7 && impacts::kind(&t, n) != 5
                && matches!(impacts::footstep(&t, n, 0, 0), (0 | 1, id) if id > 0))
            .expect("a surface with footsteps");
        let (bank, id) = impacts::footstep(&t, surface, 0, 0);
        // Most step sounds use the default envelope; it must exist.
        assert_ne!(t.collection(ENVELOPE, envelope_key(&t, id, bank)), 0, "sound {id}: no envelope");
        let e = envelope(&t, id, bank);
        c.set32(56, surface as u32);
        c.set8(52, 1);
        let dt = 1.0 / 60.0;
        strikes(&mut c, &s, &t, &mut sp, dt);
        assert_ne!(c.c32(36 + 24), 0, "surface {surface}: bank {bank} id {id}");
        assert_eq!(c.rf(36 + 24 + 4), e[5]);
        assert_eq!(c.rf(36 + 24 + 24), material_gain(&t, surface, 0).unwrap());
        // Every slot got its own submix, 8 in all.
        assert!((1..=8).all(|n| FEET.iter().any(|&(w, ..)| SLOT_AT.iter().any(|&a| c.c32(w + a + 32) == n))));
        // Held contact: no restart.
        let h = c.c32(36 + 24);
        c.set8(54, 1);
        strikes(&mut c, &s, &t, &mut sp, dt);
        assert_eq!(c.c32(36 + 24), h);
        let mut energy = 0.0f64;
        let mut now = 0.0;
        for _ in 0..120 {
            sp.service();
            instances(&mut c, &s, &t, &mut sp, dt);
            for _ in 0..3 {
                let mut b = [0.0; crate::eac::BLOCK];
                sp.render(now, &mut b);
                energy += b.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
                now += crate::eac::BLOCK as f64 / 48_000.0;
            }
        }
        assert!(energy > 0.0);
        assert!(sp.problems.is_empty(), "{:?}", sp.problems);
    }

    #[test]
    fn whooshes_follow_the_fall_edge() {
        let mut t = Tuning::default();
        let controller = t.alloc(80);
        t.w8(controller + 72, 1);
        let mut c = Component::new("footsteps", 512, vec![]);
        c.set32(28, controller);
        let mut sp = Splices::default();
        let mut s = SkaterImage::default();
        s.wf(100, 1.0);
        // No bank: the handles stay 0 but the latches run.
        whooshes(&mut c, &s, &t, &mut sp, true, true, 1.0 / 60.0);
        assert_eq!((c.c32(456), c.image[476]), (0, 0));
        s.w8(718, 1);
        whooshes(&mut c, &s, &t, &mut sp, true, false, 1.0 / 60.0);
        assert_eq!((c.image[476], c.image[405]), (1, 1));
        s.wf(100, -1.0);
        whooshes(&mut c, &s, &t, &mut sp, true, false, 1.0 / 60.0);
        assert_eq!(c.c32(456), 1);
    }
}

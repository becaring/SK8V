//! Skate 3's impact material tables: which collision sound a surface makes,
//! how hard an impact is, and at what level it plays. These are the helpers
//! the world collision events (`0x82486EF0`, `super::collisions`) and their
//! producers (`super::body_impacts`) share.
//!
//! A surface index (0..142, 143 = none) selects a row of the fixed table
//! `0x8302D6E8` (stride 16: splice slot word, pad, material collection key).
//! The material collection (class `D40CB4C0FFE45676`) names the sound ids,
//! the voice's gain (`+52`) and pitch (`+68`), its class (`+72`) and two
//! references: impact levels (`+0`/`+8`, class `13E20D398E385A56`) and
//! impact thresholds (`+24`/`+32`, class `7DAFF70B3A91CD5D`). A missing
//! collection reads AttribSys's zero page, as natively.

use super::tuning::Tuning;
use crate::ops::fctiwz;

pub const NONE: i32 = 143;
const TABLE: u32 = 0x8302_D6E8;
const MATERIAL: u64 = 0xD40C_B4C0_FFE4_5676;
const LEVELS: u64 = 0x13E2_0D39_8E38_5A56;
const THRESHOLDS: u64 = 0x7DAF_F70B_3A91_CD5D;
/// The surface map (root 64) whose row `+28` is a surface's other-side class.
const SURFACE_MAP: u64 = 0x4CA6_0755_8B1C_F440;

/// Material layout fields (native offsets in comments).
mod field {
    pub const LEVELS: u64 = 0x82B1_451A_9015_2514; // +0 RefSpec, key +8
    pub const THRESHOLDS: u64 = 0xE228_508F_E0F5_3970; // +24 RefSpec, key +32
    pub const GAIN: u64 = 0x875B_A753_41DC_8391; // +52
    pub const ALTERNATE: u64 = 0x57E3_031C_18C7_69A1; // +56 bool
    pub const PITCH: u64 = 0xC090_F2C1_F048_F17B; // +68
    pub const CLASS: u64 = 0xD5EF_6862_87A5_7AFE; // +72
    pub const LANDING: u64 = 0x1EBF_9D2E_B0DD_56BA; // +124 bool
    pub const STEPS: u64 = 0xB1CF_62EA_632C_F13F; // +125 bool
    pub const STEP_GAIN: u64 = 0x3732_0DF0_0471_F91A; // +48
    /// Attrib (not a layout field) replacing `+48` for a hard landing.
    pub const HARD_STEP_GAIN: u64 = 0x9D60_68AB_1670_3650;
    /// Attrib (not a layout field) replacing the pitch when `+56` is set.
    pub const ALTERNATE_PITCH: u64 = 0x3A3D_D47E_8DAF_E796;
}

fn row(t: &Tuning, surface: i32) -> (i32, u64) {
    let at = TABLE + 16 * surface as u32;
    (t.g32(at) as i32, (t.g32(at + 8) as u64) << 32 | t.g32(at + 12) as u64)
}

fn word(t: &Tuning, collection: u32, key: u64) -> u32 {
    t.record(collection, key).map_or(0, |r| t.g32(r.address))
}

fn byte(t: &Tuning, collection: u32, key: u64) -> u8 {
    t.record(collection, key).map_or(0, |r| t.g8(r.address))
}

fn reference(t: &Tuning, collection: u32, key: u64) -> u64 {
    t.record(collection, key).filter(|r| r.stride >= 16)
        .map_or(0, |r| (t.g32(r.address + 8) as u64) << 32 | t.g32(r.address + 12) as u64)
}

/// `0x82482580`: the surface's material collection (0 when absent).
pub fn material(t: &Tuning, surface: i32) -> u32 {
    if !(0..NONE).contains(&surface) {
        return 0;
    }
    t.collection(MATERIAL, row(t, surface).1)
}

/// Material `+124`: whether a landing on the surface raises an impact event.
pub fn landing_event(t: &Tuning, surface: i32) -> bool {
    byte(t, material(t, surface), field::LANDING) != 0
}

/// `0x82494F58`: the surface's footstep kind (surface map row `+24`).
pub fn kind(t: &Tuning, surface: i32) -> i32 {
    let index = if (0..94).contains(&surface) { surface as u32 } else { 94 };
    t.g32(t.tuning_at(64, SURFACE_MAP, index).wrapping_add(24)) as i32
}

/// `0x824975D8`: the footstep splice slot and sound of `surface` by speed
/// band and landing class (`stance`, skater `+300`); id -1 when the
/// material has no steps (`+125`), 0 for a slot word above 2.
pub fn footstep(t: &Tuning, surface: i32, band: i32, stance: i32) -> (i32, i32) {
    if surface < 0 {
        return (0, -1);
    }
    let m = material(t, surface);
    if byte(t, m, field::STEPS) == 0 {
        return (0, -1);
    }
    let slot = row(t, surface).0;
    let key = match slot as u32 {
        0 if stance <= 1 && band == 0 => 0xBFAB_F634_D2B1_E45A, // +64
        0 if stance <= 1 && band == 1 => 0xEF9B_D81F_9CFF_725F, // +60
        0 => 0xA3AD_CA7B_1928_7B5D,                              // +104
        1 if stance <= 1 && band == 0 => 0x66A9_5889_604D_ED36, // +80
        1 if stance <= 1 && band == 1 => 0xB722_B88F_E44B_046E, // +76
        1 => 0x1411_108A_7E9C_C74A,                              // +84
        // Slot 2 reads collection attribs (`0x824825D0` for band 0).
        2 if band == 0 => 0x137C_683B_FB50_6ECA,
        2 if band == 1 => 0x5432_B352_24E4_B1C1,
        2 => 0x3FCB_E040_7F83_3721,
        _ => return (slot, 0),
    };
    let id = if slot == 2 { t.g32(t.attrib(m, key)) } else { word(t, m, key) };
    (slot, id as i32)
}

/// `0x824977C8`: a footstep's material gain, `+48` (or the hard-landing
/// attrib above class 1) over 32767; 0 for no surface.
pub fn footstep_gain(t: &Tuning, surface: i32, stance: i32) -> f32 {
    const LEVEL: f32 = 1.0 / 32767.0; // 0x822F8898
    if surface < 0 {
        return 0.0;
    }
    let m = material(t, surface);
    let v = if stance > 1 { t.g32(t.attrib(m, field::HARD_STEP_GAIN)) } else { word(t, m, field::STEP_GAIN) };
    v as i32 as f32 * LEVEL
}

/// `0x82496FD0`: the surface's class (`+72`); 8 outside 0..142.
pub fn class(t: &Tuning, surface: i32) -> i32 {
    if !(0..NONE).contains(&surface) {
        return 8;
    }
    word(t, material(t, surface), field::CLASS) as i32
}

/// `0x82497910`: the class (0..2) of the surface on the other side.
pub fn other_class(t: &Tuning, surface: i32) -> i32 {
    match surface {
        95 | 97 | 99..=102 | 104..=106 | 113 => 1,
        96 => 2,
        98 | 103 | 107..=109 => 0,
        _ => {
            let index = if (0..94).contains(&surface) { surface as u32 } else { 94 };
            t.g32(t.tuning_at(64, SURFACE_MAP, index).wrapping_add(28)) as i32
        }
    }
}

/// `0x82497088` (no alternate): the impact strength type (0 light, 1
/// medium, 2 hard, 3 below the lowest threshold) and the range it falls in.
pub fn strength(t: &Tuning, surface: i32, impact: f32, low: &mut f32, high: &mut f32) -> i32 {
    if surface < 0 {
        return 3;
    }
    let m = material(t, surface);
    let c = t.collection(THRESHOLDS, reference(t, m, field::THRESHOLDS));
    let f = |key| f32::from_bits(word(t, c, key));
    let (l0, l4, l8, l12) = (f(0xC8DE_D1BC_20B9_D6A5), f(0xB887_0D20_0103_3E0F),
        f(0x7D8D_EDD3_38D4_5482), f(0xD660_AC45_9139_BDF4));
    if !(impact >= l12) {
        return 3;
    }
    *low = l12;
    *high = l8;
    if impact > l0 {
        *low = l0;
        *high = l4;
        2
    } else if impact > l8 {
        *low = l8;
        *high = l0;
        1
    } else {
        0
    }
}

fn levels(t: &Tuning, surface: i32) -> u32 {
    t.collection(LEVELS, reference(t, material(t, surface), field::LEVELS))
}

/// `0x82496C58` (no alternate): the event level (0..32767) of an impact of
/// strength `ty` within `[low, high]`, by the other side's class.
pub fn level(t: &Tuning, surface: i32, other: i32, ty: i32, low: f32, high: f32, impact: f32) -> i32 {
    if surface < 0 || ty == 3 {
        return 0;
    }
    let c = levels(t, surface);
    let w = |key| word(t, c, key) as i32;
    let clamped = if impact < high { impact } else { high };
    let other = if surface < 97 && other != NONE { other_class(t, other) } else { 0 };
    let (lo, hi) = match (ty as u32, other as u32) {
        (0, 0) => (w(0xE24D_9CB4_000A_53AC), w(0x1A83_AD03_3097_6744)),
        (0, 1) => (w(0x75DC_915E_876A_9DC9), w(0x711F_1F54_903E_76C9)),
        (0, _) => (w(0x166B_AB2F_D5B6_0560), w(0x0D6E_F57A_39AF_0C93)),
        (1, 0) => (w(0x036F_313C_EBC6_64FD), w(0x8001_982D_A2E9_1D6A)),
        (1, 1) => (w(0x24B7_25E0_5CEB_027E), w(0xCBBB_19E3_02CE_1A17)),
        (1, _) => (w(0x41F2_E245_6A97_752A), w(0x2638_FF12_C8FB_BCCB)),
        (2, _) => (w(0xC8DE_D1BC_20B9_D6A5), w(0xB887_0D20_0103_3E0F)),
        _ => (0, 32767),
    };
    if high > low {
        let slope = (hi - lo) as f32 / (high - low);
        fctiwz(slope.mul_add(clamped - low, lo as f32))
    } else {
        hi
    }
}

/// `0x82496F50`: the surface's secondary-event level scale (1 for none).
pub fn scale(t: &Tuning, surface: i32) -> f32 {
    if !(0..NONE).contains(&surface) {
        return 1.0;
    }
    f32::from_bits(word(t, levels(t, surface), 0x1A5F_7E8C_CABB_B0A2))
}

/// What `0x824965D0` chooses for one side of an event.
pub struct Choice {
    pub slot: i32,
    pub id: i32,
    /// The voice's gain and pitch (`+44`/`+48`), when the material set them.
    pub gain: Option<i32>,
    pub pitch: Option<i32>,
}

/// `0x824965D0`: the splice slot, sound, gain and pitch for `surface` hit
/// against `other` with strength `ty`. `alternate` is the event's byte
/// `+40`. The hall-of-meat override (global `-572 + 564`) is off outside
/// that mode.
pub fn choose(t: &Tuning, surface: i32, other: i32, ty: i32, alternate: bool) -> Choice {
    let none = Choice { slot: -1, id: -1, gain: None, pitch: None };
    if !(0..NONE).contains(&surface) {
        return none;
    }
    let slot = row(t, surface).0;
    if slot == -1 {
        return none;
    }
    let other = if other == NONE { 0 } else { other_class(t, other) };
    let m = material(t, surface);
    let id = sound(t, m, slot, other, ty);
    let gain = (word(t, m, field::GAIN) as i32).clamp(0, 32767);
    let mut pitch = word(t, m, field::PITCH) as i32;
    if alternate && byte(t, m, field::ALTERNATE) != 0 {
        pitch = word(t, m, field::ALTERNATE_PITCH) as i32;
    }
    Choice { slot, id, gain: Some(gain), pitch: Some(pitch) }
}

/// `0x824967F8`: the sound id by slot word (1, 2, else layout 0),
/// strength (0, 2, else 1) and other class (0..2, else none: id 0).
fn sound(t: &Tuning, m: u32, slot: i32, other: i32, ty: i32) -> i32 {
    let ty = match ty { 0 | 2 => ty, _ => 1 };
    let slot = match slot { 1 | 2 => slot, _ => 0 };
    let key = match (slot, ty, other) {
        (0, 2, _) => 0x9203_DF6F_D029_B377,
        (0, 0, 0) => 0xBFAB_F634_D2B1_E45A,
        (0, 0, 1) => 0x9ABF_C645_74AB_2F9F,
        (0, 0, 2) => 0xBCD5_E888_294F_7B15,
        (0, 1, 0) => 0xEF9B_D81F_9CFF_725F,
        (0, 1, 1) => 0xA3AD_CA7B_1928_7B5D,
        (0, 1, 2) => 0xC676_C87F_862C_0490,
        (1, 2, _) => 0xF542_77A8_3E01_70FD,
        (1, 0, 0) => 0x66A9_5889_604D_ED36,
        (1, 0, 1) => 0x5955_37EB_A019_6BE7,
        (1, 0, 2) => 0x79DD_0E66_5979_3D0E,
        (1, 1, 0) => 0xB722_B88F_E44B_046E,
        (1, 1, 1) => 0x1411_108A_7E9C_C74A,
        (1, 1, 2) => 0x5079_6F92_F3DE_449B,
        // Slot 2 (hall of meat) reads collection attribs, not the layout.
        (2, 2, _) => 0x3EA2_579C_2F3B_B23B,
        (2, 0, 0) => 0x137C_683B_FB50_6ECA,
        (2, 0, 1) => 0x2A28_3013_7430_BB02,
        (2, 0, 2) => 0x79BD_E00B_04DF_51B9,
        (2, 1, 0) => 0x5432_B352_24E4_B1C1,
        (2, 1, 1) => 0x3FCB_E040_7F83_3721,
        (2, 1, 2) => 0x5331_1C67_61F1_35A1,
        _ => return 0,
    };
    word(t, m, key) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_class_table_and_level_interpolation() {
        let t = Tuning::default();
        assert_eq!((other_class(&t, 96), other_class(&t, 103), other_class(&t, 113)), (2, 0, 1));
        // An absent material reads zero thresholds: any impact is hard.
        let (mut low, mut high) = (9.0, 9.0);
        assert_eq!(strength(&t, 5, 0.5, &mut low, &mut high), 2);
        assert_eq!((low, high), (0.0, 0.0));
        assert_eq!(strength(&t, -1, 0.5, &mut low, &mut high), 3);
        assert_eq!(level(&t, 5, NONE, 3, 0.0, 1.0, 0.5), 0);
        assert_eq!(scale(&t, NONE), 1.0);
        assert_eq!(choose(&t, NONE, 0, 0, false).id, -1);
    }
}

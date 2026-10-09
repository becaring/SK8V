//! Seam clicks (`0x824C14C8` and its helpers): the board component's
//! "wheel crossed a seam" logic that drives the four Class_Seams objects
//! (`+52..+64`, one per wheel). Each wheel's position (skater `+384 + 16·w`)
//! is rotated by the seam grid's angle and divided by the surface's seam
//! spacing (`*(this + 40)`: `+4` angle, `+8`/`+12` spacing, `+16` type);
//! a new grid cell is a click. Seam types (`spidercrack`, `square_N_x_N`,
//! `irregular_*`, `slats`, `sidewalk`, `brick_tile_random_size`,
//! `mini_tile`, `special_*`) select AttribSys collections by name.

use super::board::skater_mode;
use super::env::{Env, Send};
use super::{clamp, Wrappers};
use crate::libm::{cos, sin};
use crate::ops::{fctiwz, fmadds, fmsubs};

/// Bob Jenkins' lookup8 64-bit hash (`0x82B73AC0(bytes, len)`, level
/// `0xABCDEF0011223344`).
pub fn name_hash(k: &[u8]) -> u64 {
    fn mix(a: &mut u64, b: &mut u64, c: &mut u64) {
        macro_rules! step {
            ($x:ident, $y:ident, $z:ident, $op:tt, $s:expr) => {
                *$x = $x.wrapping_sub(*$y);
                *$x = $x.wrapping_sub(*$z);
                *$x ^= (*$z) $op $s;
            };
        }
        step!(a, b, c, >>, 43);
        step!(b, c, a, <<, 9);
        step!(c, a, b, >>, 8);
        step!(a, b, c, >>, 38);
        step!(b, c, a, <<, 23);
        step!(c, a, b, >>, 5);
        step!(a, b, c, >>, 35);
        step!(b, c, a, <<, 49);
        step!(c, a, b, >>, 11);
        step!(a, b, c, >>, 12);
        step!(b, c, a, <<, 18);
        step!(c, a, b, >>, 22);
    }
    let le = |s: &[u8]| -> u64 { s.iter().enumerate().fold(0u64, |acc, (i, b)| acc | (*b as u64) << (8 * i)) };
    let (mut a, mut b) = (0xABCD_EF00_1122_3344u64, 0xABCD_EF00_1122_3344u64);
    let mut c = 0x9E37_79B9_7F4A_7C13u64;
    let mut rest = k;
    while rest.len() >= 24 {
        a = a.wrapping_add(le(&rest[0..8]));
        b = b.wrapping_add(le(&rest[8..16]));
        c = c.wrapping_add(le(&rest[16..24]));
        mix(&mut a, &mut b, &mut c);
        rest = &rest[24..];
    }
    c = c.wrapping_add(k.len() as u64);
    let n = rest.len();
    if n > 16 {
        c = c.wrapping_add(le(&rest[16..n]) << 8);
    }
    if n > 8 {
        b = b.wrapping_add(le(&rest[8..n.min(16)]));
    }
    if n > 0 {
        a = a.wrapping_add(le(&rest[0..n.min(8)]));
    }
    mix(&mut a, &mut b, &mut c);
    c
}

/// Seam type names by `0x82497A58` index (type id − 1), read from the
/// image's table at `0x8224DDDC − 212`.
pub const SEAM_TYPES: [&str; 15] = [
    "spidercrack",
    "square_2_x_2",
    "square_4_x_4",
    "square_8_x_8",
    "square_12_x_12",
    "square_24_x_24",
    "irregular_small",
    "irregular_medium",
    "irregular_large",
    "slats",
    "sidewalk",
    "brick_tile_random_size",
    "mini_tile",
    "special_1",
    "special_2",
];

/// `0x824C2428(this, wheel, alt)`: seam class of a wheel's surface (skater
/// `+620 + 4·wheel`): surface table `+32`, or `+36` + 5 with `alt`; 0 for
/// surfaces ≥ 143.
pub fn wheel_class(env: &mut Env, wheel: i32, alt: bool) -> i32 {
    let s = env.s32(620 + 4 * wheel as usize) as i32;
    if s >= 143 {
        return 0;
    }
    let index = if (0..94).contains(&s) { s as u32 } else { 94 };
    let e = env.tuning_at(64, 0x4CA6_0755_8B1C_F440, index);
    if alt { (env.g32(e + 36) as i32).wrapping_add(5) } else { env.g32(e + 32) as i32 }
}

/// `0x824C1DF8(this, wheel, double, alt)`: one click on a wheel's object.
fn click(env: &mut Env, wrappers: &mut Wrappers, wheel: i32, double: bool, alt: bool) {
    let slot = 52 + 4 * wheel as u32;
    let toggle = env.c8(68 + wheel as usize);
    let class = wheel_class(env, wheel, alt);
    let mode = skater_mode(env);
    let w = wrappers.words(slot, 20);
    w[9] = clamp(double as i32, 2);
    w[7] = clamp((toggle == 0) as i32 + 1, 2);
    w[10] = clamp(class, 8);
    w[11] = clamp(mode, 1);
    env.set_c8(68 + wheel as usize, (toggle == 0) as u8);
}

/// `0x824C1BA8`: rotate a wheel position by the grid angle (degrees), with
/// the game's sine and cosine. Returns (x', z').
fn rotate(x: f32, z: f32, angle: i32) -> (f32, f32) {
    // 0x8206D110 = π/180
    let a = angle as f32 * 0.017_453_292_f32;
    let c = cos(a as f64) as f32;
    let s = sin(a as f64) as f32;
    let x2 = fmsubs(c, x, s * z);
    let s2 = sin(a as f64) as f32;
    let c2 = cos(a as f64) as f32;
    let z2 = fmadds(c2, z, s2 * x);
    (x2, z2)
}

/// `0x82F4DE80`: floor (double).
fn floor(x: f64) -> f64 {
    x.floor()
}

/// `0x824C1CA0(this, wheel, axis)`: did the wheel enter a new seam cell?
fn crossed(env: &mut Env, wheel: i32, axis: i32) -> bool {
    if env.s8(332) != 0 || env.s8(341) != 0 {
        return false;
    }
    let base = 384 + 16 * wheel as usize;
    let (x, z) = (env.sf(base), env.sf(base + 8));
    let at = 72 + 16 * axis as usize + 4 * wheel as usize;
    if let Some(hit) = env.seam_hit(wheel) {
        // SkateV texture-space joints: the
        // runtime traced this wheel's path through the visual ground's own
        // UVs over the texture's joint map. Entering a joint is one crossing,
        // reported on axis 0 (both axes play the same click).
        return axis == 0 && hit;
    }
    if let Some(g) = env.seam_grid(wheel) {
        // SkateV aligned grid: the cell is
        // counted on GTA's visible joint lines under this wheel. An axis
        // without a recovered line family never crosses.
        let Some(f) = g.families[axis.clamp(0, 1) as usize] else { return false };
        let along = f.a as f64 * x as f64 + f.b as f64 * z as f64 - f.phase as f64;
        let cell = (along / f.spacing as f64).floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
        if env.c32(at) as i32 != cell {
            env.set_c32(at, cell as u32);
            return true;
        }
        return false;
    }
    let grid = env.c32(40);
    let (x2, z2) = rotate(x, z, env.g32(grid + 4) as i32);
    let mut scale = 1.0f32;
    if wheel_class(env, wheel, false) == 3 && env.g32(grid + 16) as i32 == 10 {
        scale = env.tuning_f32(28, 0x1911_1761_87FB_9B1F);
    }
    let (pos, spacing) = if axis == 1 {
        (z2, f32::from_bits(env.g32(grid + 8)) * scale)
    } else {
        (x2, f32::from_bits(env.g32(grid + 12)) * scale)
    };
    let cell = fctiwz(floor((pos / spacing) as f64) as f32);
    if env.c32(at) as i32 != cell {
        env.set_c32(at, cell as u32);
        true
    } else {
        false
    }
}

/// The current seam type (skater `+636`, or `+648` while grinding on the
/// board without `+464`); `None` when 0. Otherwise the type's collection
/// is stored at component `+36` (0 when missing).
fn seam_collection(env: &mut Env) -> Option<u32> {
    let mut t = env.s32(636) as i32;
    if (env.s8(340) != 0 || env.s8(339) != 0) && env.s8(464) == 0 {
        t = env.s32(648) as i32;
    }
    if t == 0 {
        return None;
    }
    let key = if ((t - 1) as u32) <= 14 { name_hash(SEAM_TYPES[(t - 1) as usize].as_bytes()) } else { 0 };
    let c = env.collection(0x7242_F328_31ED_3332, key);
    env.set_c32(36, c);
    Some(c)
}

/// Clears word 13 of the four objects (`+56`).
fn quiet(env: &mut Env, wrappers: &mut Wrappers) {
    for k in 0..4u32 {
        if env.c32(52 + 4 * k as usize) != 0 {
            wrappers.words(52 + 4 * k, 20)[13] = 0;
        }
    }
}

/// `0x824C1698(this, axis)`.
fn trigger(env: &mut Env, wrappers: &mut Wrappers, axis: i32) {
    let Some(c) = seam_collection(env) else {
        quiet(env, wrappers);
        return;
    };
    let mode = env.g32(env.attrib(c, 0xCA81_764B_F5A8_5E34)) as i32;
    if mode == 0 {
        quiet(env, wrappers);
        return;
    }
    let now = env.c32(132) as i32 + 1;
    env.set_c32(132, now as u32);
    if now > 32767 {
        for off in [132, 136, 140, 144, 148] {
            env.set_c32(off, 0);
        }
    }
    if mode == 1 {
        let a = crossed(env, 0, axis);
        let b = crossed(env, 1, axis);
        let (mut c2, mut d2) = (false, false);
        if !(env.s8(340) != 0 || env.s8(339) != 0) {
            c2 = crossed(env, 2, axis);
            d2 = crossed(env, 3, axis);
        }
        let gap = |env: &mut Env| env.g32(env.attrib(env.c32(36), 0xDAE8_03A0_CBD2_86D1)) as i32;
        let both = a && b;
        if a {
            let t = gap(env);
            if (env.c32(132) as i32).wrapping_sub(env.c32(140) as i32) > t {
                click(env, wrappers, 0, !both, false);
                env.set_c32(136, env.c32(132));
            }
        } else if b {
            let t = gap(env);
            if (env.c32(132) as i32).wrapping_sub(env.c32(136) as i32) > t {
                click(env, wrappers, 1, !both, false);
                env.set_c32(140, env.c32(132));
            }
        }
        let both = c2 && d2;
        if c2 {
            let t = gap(env);
            if (env.c32(132) as i32).wrapping_sub(env.c32(148) as i32) > t {
                click(env, wrappers, 2, !both, false);
                env.set_c32(144, env.c32(132));
            }
        } else if d2 {
            let t = gap(env);
            if (env.c32(132) as i32).wrapping_sub(env.c32(144) as i32) > t {
                click(env, wrappers, 3, !both, false);
                env.set_c32(148, env.c32(132));
            }
        }
    } else if mode == 2 && env.s8(332) == 0 && axis == 0 {
        // Distance mode: travel accumulates at component +104 (and counts
        // down +108) by speed x 100 x min(frame factor, 2) x (*(this+16)+60).
        let speed = env.sf(208);
        let frame = f32::from_bits(env.g32(env.g32(0x8308_3C38).wrapping_add(0x2F070)));
        let k = f32::from_bits(env.g32(env.c32(16).wrapping_add(60)));
        // 0x820ED57C = 100.0, 0x82060C50 = 2.0
        let f12 = speed * 100.0;
        let f0 = if frame > 2.0 { 2.0 } else { frame };
        let step = f0 * f12 * k;
        let travelled = env.cf(104) + step;
        env.set_c32(104, travelled.to_bits());
        // 0x821FE13C
        const BIG: f32 = 3.994_265_7e24;
        let left = env.cf(108);
        if !(left > BIG) {
            env.set_c32(108, (left - step).to_bits());
        }
        let every = f32::from_bits(env.g32(env.attrib(env.c32(36), 0xA3BC_1976_039F_A00A)));
        if env.cf(104) > every {
            click(env, wrappers, 0, false, false);
            click(env, wrappers, 1, false, false);
            env.set_c32(104, 0f32.to_bits());
            env.set_c32(108, BIG.to_bits());
        }
        if !(env.cf(108) > 0.0) {
            click(env, wrappers, 2, false, false);
            click(env, wrappers, 3, false, false);
            // 0x82324510 = 999.0
            env.set_c32(108, 999.0f32.to_bits());
        }
    }
}

/// `0x824C14C8`: Class_Seams clicks; sends all four objects.
pub fn clicks(env: &mut Env, wrappers: &mut Wrappers) -> Vec<Send> {
    let mut out = Vec::new();
    if env.g8(env.c32(16).wrapping_add(52)) == 0 {
        return out;
    }
    for k in 0..4u32 {
        if env.c32(52 + 4 * k as usize) != 0 {
            wrappers.words(52 + 4 * k, 20)[7] = 0;
        }
    }
    if env.s8(332) != 0 {
        env.set_c8(128, 0);
    }
    let mut changed = false;
    if env.c8(128) != 0 {
        for w in 0..4i32 {
            if env.c32(112 + 4 * w as usize) != env.s32(620 + 4 * w as usize) {
                click(env, wrappers, w, false, true);
                changed = true;
            }
        }
    }
    for w in 0..4usize {
        let s = env.s32(620 + 4 * w);
        env.set_c32(112 + 4 * w, s);
    }
    env.set_c8(128, 1);
    if !changed {
        let speed = env.sf(208);
        let min = f32::from_bits(env.g32(env.attrib(env.c32(36), 0xF7BC_A67F_0A1F_C92E)));
        if speed > min {
            trigger(env, wrappers, 0);
            trigger(env, wrappers, 1);
        }
    }
    for k in 0..4u32 {
        let slot = 52 + 4 * k;
        if env.c32(slot as usize) != 0 {
            let w = wrappers.words(slot, 20);
            out.push(Send::update(slot, w));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glue::Params;
    use crate::glue::env::Env;
    use crate::live::driver::Component;
    use crate::live::frame::{SeamFamily, SeamGrid, SkaterImage};
    use crate::live::tuning::Tuning;

    fn at(x: f32, z: f32, grid: Option<SeamGrid>) -> SkaterImage {
        let mut s = SkaterImage::default();
        s.wf(384, x);
        s.wf(392, z);
        s.seam_grids[0] = grid;
        s
    }

    #[test]
    fn aligned_grid_counts_cells_on_the_recovered_lines() {
        // Lines every 1.5 m along a 30-degree normal, offset 0.4 m.
        let (a, b) = (30f32.to_radians().cos(), 30f32.to_radians().sin());
        let fam = SeamFamily { a, b, spacing: 1.5, phase: 0.4 };
        let grid = Some(SeamGrid { families: [Some(fam), None] });
        let tuning = Tuning::default();
        let mut comp = Component::new("seams", 192, vec![]);
        let cross = |d: f32, comp: &mut Component| {
            let s = at(a * d, b * d, grid);
            let mut env = Env { comp, skater: &s, tuning: &tuning, params: Params::default() };
            (crossed(&mut env, 0, 0), crossed(&mut env, 0, 1))
        };
        // First sample settles the cell (it differs from the zeroed state).
        cross(0.5, &mut comp);
        assert_eq!(cross(1.8, &mut comp), (false, false), "still before the line at 1.9 m");
        assert_eq!(cross(2.0, &mut comp), (true, false), "crossed n.p = 1.9; no second family");
        assert_eq!(cross(3.3, &mut comp), (false, false));
        assert_eq!(cross(3.5, &mut comp), (true, false), "next line at 3.4 m");
        assert_eq!(cross(3.0, &mut comp), (true, false), "back across it");
    }

    #[test]
    fn texture_joints_click_once_on_axis_zero_and_override_grids() {
        let fam = SeamFamily { a: 1.0, b: 0.0, spacing: 0.1, phase: 0.0 };
        let tuning = Tuning::default();
        let mut comp = Component::new("seams", 192, vec![]);
        for (hit, want) in [(Some(true), (true, false)), (Some(false), (false, false))] {
            let mut s = at(0.55, 0.0, Some(SeamGrid { families: [Some(fam), Some(fam)] }));
            s.seam_hits[0] = hit;
            let mut env = Env { comp: &mut comp, skater: &s, tuning: &tuning, params: Params::default() };
            assert_eq!((crossed(&mut env, 0, 0), crossed(&mut env, 0, 1)), want);
        }
    }
}

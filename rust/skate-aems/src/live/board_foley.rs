//! The board foley component's splice sounds (vtable `0x822FC698`, mix-map
//! bank 1, the component `foot_drag` lives in): the takeoff pop, the
//! landing, the wheel landings, the catch, the foot slaps, the push plants
//! and the sounds that go with them.
//!
//! Component fields (the game's offsets): `+36` manual landing (`+40` its
//! surface class), `+52` local-player pop layer, `+56` local-player landing
//! layer, `+60` pop, `+64` pop category, `+68` pop table, `+80`/`+84` push
//! plant (surface type, handle), `+88`/`+92` push release, `+96` speed
//! sound, `+100..+108` catch (right foot, left foot, both), `+112`/`+116`
//! foot slaps, `+120` previous skater `+332`, `+121` manual latch, `+122`
//! previous skater `+341`, `+123` push latch, `+132..+135` per-wheel landing
//! latches, `+136` second-layer latch, `+140 + 24k` four wheel landings
//! (`+144` mode, `+148` class, `+152` surface class), `+236` the heavy
//! layer (`+240` mode, `+248` surface class), `+340` skater `+236` while
//! airborne, `+344` skater `+676`, `+348..+368` catch timers, `+372..+400`
//! the skeleton edge sounds and their previous flags, `+424` frames since
//! the component started, `+476`/`+480` the catch override, `+484` slap
//! variant, `+485`/`+488` trick 38, `+496` the landing's positional layer,
//! `+500..+503` wheel-landing latches. The pop, the wheel and manual
//! landings, the catches and the positional landing play into their own
//! submix: unity to its bus, with an aux send from a mix output that the
//! host's own reverb replaces; the positional sounds' placement is routing
//! too, which the host does.
//!
//! The drags (`0x824C01E8` on skater `+310` at `+456`, `0x824C07D8` on
//! `+320` at `+368`) and the `+440` timer run here too; the world collision
//! events (`0x82486EF0`) are `super::collisions`, raised from
//! `super::body_impacts`. Replays are not modelled: mix input 9
//! (`replay_drag`) always takes its outside-a-replay path.

use super::body_impacts;
use super::collisions::Collisions;
use super::driver::Component;
use super::frame::SkaterImage;
use super::splices::{Splices, SLOT_FOLEY};
use super::tuning::Tuning;
use crate::ops::fctiwz;
use crate::splice::Params;

/// `0x822F8898`, `0x822F890C`, `0x822F8C64`.
const LEVEL: f32 = 1.0 / 32767.0;
const PITCH: f32 = 1.0 / 4096.0;
const PAN: f32 = 360.0 / 65536.0;

fn word(t: &Tuning, root: u32, key: u64, index: u32) -> u32 {
    t.g32(t.tuning_at(root, key, index))
}

fn id(t: &Tuning, root: u32, key: u64, index: u32) -> i32 {
    word(t, root, key, index) as i32
}

fn float(t: &Tuning, key: u64) -> f32 {
    f32::from_bits(word(t, 24, key, 0))
}

fn float_at(t: &Tuning, root: u32, key: u64, index: u32) -> f32 {
    f32::from_bits(word(t, root, key, index))
}

fn local(c: &Component, t: &Tuning) -> bool {
    t.g8(c.c32(28).wrapping_add(72)) != 0
}

/// `0x82494D78` / `0x82494EB8`: field `off` of the surface map row (index
/// 94 holds every surface outside 0..94).
fn surface_field(t: &Tuning, surface: i32, off: u32) -> u32 {
    let index = if (0..94).contains(&surface) { surface as u32 } else { 94 };
    t.g32(t.tuning_at(64, 0x4CA6_0755_8B1C_F440, index).wrapping_add(off))
}

/// Starts `id` in `slot` and plays it (`0x82975700`, `0x82975A60`).
fn start(sp: &mut Splices, slot: usize, id: i32, dt: f32) -> u32 {
    let h = sp.create(slot, id);
    sp.play(h, 0, Params::at_play(dt));
    h
}

/// `0x82497F48`: a positional sound plays with no elapsed time.
fn start_positional(sp: &mut Splices, id: i32) -> u32 {
    start(sp, 0, id, 0.0)
}

fn release(c: &mut Component, sp: &mut Splices, off: usize) {
    let h = c.c32(off);
    if h != 0 {
        sp.destroy(h);
        c.set32(off, 0);
    }
}

/// `0x824B8218`, once per frame: the component's splice triggers, in the
/// game's order.
pub fn update(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, col: &mut Collisions, dt: f32) {
    catch(c, s, t, sp, dt);
    slaps(c, s, t, sp, dt);
    let manual = manual_landing(c, s, t, sp, dt);
    wheel_landings(c, s, t, sp, dt, manual);
    takeoff(c, s, t, sp, col, dt);
    // 0x824BB540 is the foot drag object (`glue::board::foot_drag`).
    pushes(c, s, t, sp, dt);
    body_impacts::body(c, s, t, col, sp);
    body_impacts::board(c, s, t, col, sp);
    skeleton_edges(c, s, t, sp, dt);
    drag_a(c, s, t, sp, dt);
    drag_b(c, s, t, sp, dt);
    trick_38(c, s, t, sp, dt);
    // 0x824B82C8: the timer `+440` counts down in whole milliseconds
    // (literal `0x82256FE8`, 1000) and stops at 0.
    let left = c.c32(440) as i32 - fctiwz(dt * 1000.0);
    c.set32(440, left.max(0) as u32);
    replay_drag(c);
}

/// `0x824C0AB8`: mix input 9 from the replay drag `+464` over `+336`.
/// Outside a replay (the flag at `0x82083C38 -> +0x2FCB4 -> +16` is 0)
/// `+460..+472` are cleared first.
fn replay_drag(c: &mut Component) {
    for off in [460, 464, 468, 472] {
        c.set32(off, 0);
    }
    let x = c.rf(464) / c.rf(336) * 32767.0;
    let lower = if -x >= 0.0 { 0.0 } else { x };
    let clamped = if 32767.0 - lower >= 0.0 { lower } else { 32767.0 };
    c.inputs[9] = fctiwz(clamped).clamp(0, 32767);
}

/// `0x824C01E8`: the drag on unmounting (skater `+310`), on a surface
/// whose surface map row `+8` is 1 or not (`+620`, the first wheel's).
fn drag_a(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    if c.c32(456) != 0 || s.r8(310) == 0 {
        return;
    }
    let surface = s.r32(620) as i32;
    let field = if surface < 143 { surface_field(t, surface, 8) } else { 0 };
    let id = drag_id(s, t, field != 1);
    // The submix routing (root 140 `0x55DA01533B378EFA`) is the host's.
    let h = sp.create(0, id);
    c.set32(456, h);
    sp.play(h, 0, Params::at_play(dt));
}

/// `0x824C0350`: the drag sound by speed (`+208` in km/h, literal
/// `0x822F8628`) against the drag collection's two speeds. The hall of
/// meat override (global `-572 + 564`) is off outside that mode.
fn drag_id(s: &SkaterImage, t: &Tuning, flag: bool) -> i32 {
    let speed = s.rf(208) * f32::from_bits(0x4066_6666);
    let m = t.collection(0x923C_CB46_EF5B_F5BA, 0x109E_63A9_F198_B562);
    let attr = |key: u64| t.record(m, key).map_or(0, |r| t.g32(r.address));
    let key = if speed > f32::from_bits(attr(0xF60C_C341_DB01_452A)) {
        if flag { 0x47CC_07A8_59D6_A8C5 } else { 0x72B9_F4DE_2D0A_DC07 }
    } else if speed > f32::from_bits(attr(0x61AE_1666_FE7D_F49D)) {
        if flag { 0x0F52_513C_0394_2C24 } else { 0xF3C8_2D6F_8211_1158 }
    } else if flag {
        0xC8AD_6743_FE48_41F3
    } else {
        0x102F_3F03_B6B8_F544
    };
    attr(key) as i32
}

/// `0x824C07D8`: the kickout dismount drag (skater `+320`). Its handle
/// lives at `+368`, which the catch (`0x824B95A0`) also writes.
fn drag_b(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    if c.c32(368) != 0 || s.r8(320) == 0 {
        return;
    }
    // The submix routing (root 140 `0xF61F2797B24868D9`) is the host's.
    let h = sp.create(0, id(t, 108, 0xE518_088F_4E61_203A, 1));
    c.set32(368, h);
    sp.play(h, 0, Params::at_play(dt));
}

/// `0x824B90D8`: clears mix inputs 0, 1 and 6, starts the pop on the
/// takeoff edge and the landing on the touchdown edge off a grind.
pub fn takeoff(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, col: &mut Collisions, dt: f32) {
    c.set32(424, c.c32(424).wrapping_add(1));
    c.inputs[0] = 0;
    c.inputs[1] = 0;
    c.inputs[6] = 0;
    let air = s.r8(332) != 0;
    let grinding = s.r8(341) != 0;
    if c.image[120] == 0 {
        if air && s.r8(343) != 0 {
            let trick = s.r32(348) as i32;
            if !matches!(trick, -1 | 31 | 32 | 35 | 36) && c.image[122] == 0 {
                pop(c, s, t, sp, trick, dt);
            }
        }
    } else if !air && !grinding {
        landing(c, s, t, sp, col, dt);
    }
    if air {
        c.set32(340, s.r32(236));
    }
    c.set8(120, air as u8);
    c.set8(344, s.r8(676));
    if grinding && c.image[122] == 0 {
        body_impacts::grind_start(c, s, t, col, sp);
    }
    c.set8(122, grinding as u8);
}

/// `0x824BA310`'s bit 1: the surface's pop table.
fn hard_surface(s: &SkaterImage, t: &Tuning) -> bool {
    let surface = s.r32(620) as i32;
    surface < 143 && surface_field(t, surface, 8) != 0
}

/// `0x824BA310`: the surface class of the wheel landings (0..3). Bit 1 is
/// the surface map row's `+8`; bit 0 is `0x824B23C8`, the local skater's
/// `+684` (a remote skater reads the local controller's copy; the host has
/// no remote skaters).
fn surface_class(c: &Component, s: &SkaterImage, t: &Tuning) -> u32 {
    let hard = hard_surface(s, t);
    let low = if local(c, t) { s.r32(684) != 0 } else { true };
    (hard as u32) << 1 | low as u32
}

/// `0x824BA3F0`: the wheel-landing sound for `mode` (0 four wheels, 1
/// two, 2 the last two, 3 one, 4 the manual landing) and `class` (2 is the
/// heavy layer), with the surface class it used.
fn landing_id(c: &Component, s: &SkaterImage, t: &Tuning, mode: u32, class: u32) -> (i32, u32) {
    let out = surface_class(c, s, t);
    let table = if mode <= 2 && class >= 2 { 0 } else { out };
    let key = match table {
        0 => 0x5A93_802D_11B0_0173,
        1 => 0xA0F8_6FEE_A9C2_412F,
        2 => 0x797E_C345_0249_9EC3,
        _ => 0xAB0D_9205_8B42_93A7,
    };
    // The replay camera's override (global `-572 + 564`) is off.
    (id(t, 24, key, mode * 3 + class), out)
}

/// `0x824BEA80`: a wheel landing's level, mix output 3 scaled by its
/// surface class's table.
fn landing_level(c: &Component, t: &Tuning, mode: u32, class: u32, out: u32) -> f32 {
    let scale = match out {
        0 => float_at(t, 24, 0xCEA5_AFA8_BA17_0B07, mode * 3 + class),
        1 => float_at(t, 24, 0x951A_D537_1803_0327, mode * 3 + class),
        2 => float_at(t, 24, 0x068C_8C5E_BEC1_B45F, mode * 3 + class),
        3 => float_at(t, 24, 0x6823_F491_0A18_82AD, mode * 3 + class),
        _ => 1.0,
    };
    fctiwz(c.params.u15(3) as f32 * scale) as f32 * LEVEL
}

/// `0x824B9CC8`: the pop for `trick`.
fn pop(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, trick: i32, dt: f32) {
    let early = (c.c32(424) as i32) < 2;
    if c.c32(60) != 0 {
        sp.destroy(c.c32(60));
        for off in [60, 64, 68, 72, 76] {
            c.set32(off, 0);
        }
    }
    if c.c32(60) != 0 || early {
        return;
    }
    // Category from the landing level (`+468`).
    let level = s.rf(468);
    let mut cat = if level > float(t, 0xF2A1_E273_ABB8_E9AB) {
        2
    } else if level > float(t, 0xD775_8385_CDB8_DC26) {
        1
    } else {
        0
    };
    if trick == 33 || trick == 34 {
        cat = 0;
    }
    // 0x824B9AD8 (the replay-camera override, global `-572 + 564`, is off
    // in the free-skate host).
    let mode = hard_surface(s, t) as u32;
    let key = if mode == 0 { 0x3C1E_3B96_5A93_594A } else { 0x84DF_ABF7_6D82_1DEC };
    let pop = id(t, 24, key, cat);
    c.set32(64, cat);
    c.set32(68, mode);
    let h = start(sp, 0, pop, dt);
    c.set32(60, h);
    c.inputs[0] = 32767;
    // The speed sound (SFX Master).
    if c.c32(96) == 0 {
        let speed = s.rf(208);
        let sound = if speed > float(t, 0x5852_3180_E1AD_61B4) {
            Some(id(t, 24, 0x7A74_5D81_E4BC_ABC3, 0))
        } else if speed > float(t, 0xA730_73D3_A33E_35AE) || s.r8(344) != 0 {
            Some(id(t, 24, 0x537C_97E4_F64A_0EE2, 0))
        } else if speed > float(t, 0xFB71_DF2C_8592_8859) {
            Some(id(t, 24, 0x9C4C_DCF0_DD84_C281, 0))
        } else {
            None
        };
        if let Some(sound) = sound {
            let h = start(sp, 0, sound, dt);
            c.set32(96, h);
        }
    }
    if local(c, t) {
        release(c, sp, 52);
        let h = start(sp, 0, id(t, 24, 0xC3C2_5A37_D007_12A8, 0), dt);
        c.set32(52, h);
    }
}

/// `0x824BA630`: the landing. Sets mix input 1, restarts the local
/// player's landing layer, starts the positional landing while skater
/// `+769`/`+776` hold, and sets mix input 2 from the landed wheels' class.
fn landing(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, col: &mut Collisions, dt: f32) {
    c.set32(424, 0);
    c.inputs[1] = 32767;
    if local(c, t) {
        release(c, sp, 56);
        let h = start(sp, 0, id(t, 24, 0x633F_A94E_39C1_AE8F, 0), dt);
        c.set32(56, h);
    }
    body_impacts::landing(c, s, t, col, sp);
    if s.r8(769) != 0 || s.r8(776) != 0 {
        release(c, sp, 496);
        let surface = if s.r8(614) != 0 { s.r32(660) as i32 } else { 0 };
        let soft = surface_field(t, surface, 8) == 0;
        let long = c.rf(340) >= float(t, 0x224B_0656_2D9D_5E0E);
        let key = match (soft, long) {
            (true, true) => 0x3A2F_1C78_8E21_D92D,
            (true, false) => 0xF262_042E_AA29_5711,
            (false, true) => 0xBF22_DD8B_C69D_DE53,
            (false, false) => 0x1E86_4695_56AC_D80A,
        };
        let h = start_positional(sp, id(t, 24, key, 0));
        c.set32(496, h);
    }
    let mut landed = 0;
    let mut class = 0;
    for i in 0..4 {
        if s.r8(464 + i) != 0 {
            landed += 1;
            class = class.max(s.r32(448 + 4 * i) as i32);
        }
    }
    // The replay-camera wheel landing (`0x824B8D48`) is off.
    c.inputs[2] = if landed == 0 {
        0
    } else {
        match class {
            1 => 16000,
            2 => 32767,
            _ => 0,
        }
    };
}

/// `0x824BB330`: the manual landing, when a manual ends on three or four
/// wheels. Returns whether it played.
fn manual_landing(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) -> bool {
    if s.r8(340) != 0 {
        c.set8(121, s.r8(340));
        return false;
    }
    if c.image[121] == 0 {
        return false;
    }
    match s.r32(200) as i32 {
        3 | 4 => {}
        0 => {
            c.set8(121, 0);
            return false;
        }
        _ => return false,
    }
    release(c, sp, 36);
    for off in [40, 44, 48] {
        c.set32(off, 0);
    }
    let (sound, out) = landing_id(c, s, t, 4, 0);
    c.set32(40, out);
    let h = start(sp, 0, sound, dt);
    c.set32(36, h);
    for i in 0..4 {
        c.set8(132 + i, 1);
    }
    c.set8(121, 0);
    true
}

/// `0x824B86E0`: the wheel landings, from the wheels that touched down
/// since they were last latched.
fn wheel_landings(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32, manual: bool) {
    c.inputs[3] = 0;
    c.inputs[4] = 0;
    // The replay camera's skip (global `-572 + 564`) is off.
    if c.image[500] != 0 && s.r8(716) == 0 && s.r8(332) != 0 && (s.r32(720) as i32) > 0 {
        c.set8(501, 1);
    }
    c.set8(500, s.r8(716));
    if c.image[503] == 0 && s.r8(814) != 0 && s.r8(372) == 0 {
        c.set8(502, 1);
    }
    c.set8(503, s.r8(814));
    let mut landed = [false; 4];
    let mut count = 0;
    let mut class = 0i32;
    for i in 0..4 {
        landed[i] = s.r8(464 + i) != 0;
        if landed[i] {
            if c.image[132 + i] == 0 {
                count += 1;
                class = class.max(s.r32(448 + 4 * i) as i32);
            } else {
                landed[i] = false;
            }
        } else {
            c.set8(132 + i, 0);
        }
    }
    if count != 0 && c.image[501] != 0 {
        class = 2;
        c.set8(501, 0);
    }
    let latched = (0..4).filter(|&i| c.image[132 + i] != 0).count();
    if !local(c, t) && count > 0 && class == 2 {
        class = 1;
    }
    let heavy = (class > 1) as u8;
    let latch_all = |c: &mut Component| (0..4).for_each(|i| c.set8(132 + i, 1));
    let latch_landed = |c: &mut Component| (0..4).filter(|&i| landed[i]).for_each(|i| c.set8(132 + i, 1));
    match count {
        4 => {
            if class == 1 || class == 2 {
                // 0x8208EDA4 (0.08), 0x821747FC (32767).
                let speed = (s.rf(208) * 0.08).clamp(0.0, 1.0);
                c.inputs[5] = fctiwz(speed * 32767.0).clamp(0, 32767);
                c.inputs[if class == 1 { 3 } else { 4 }] = 32767;
            }
            start_wheels(c, s, t, sp, dt, 0, class, false);
            latch_all(c);
            c.set8(136, 0);
        }
        3 => {
            start_wheels(c, s, t, sp, dt, 0, class, true);
            latch_all(c);
            c.set8(136, 0);
        }
        2 => match latched {
            0 => {
                c.set8(136, heavy);
                start_wheels(c, s, t, sp, dt, 1, class, false);
                latch_landed(c);
            }
            2 => {
                if !manual && c.image[121] == 0 {
                    start_wheels(c, s, t, sp, dt, 2, class, true);
                }
                latch_landed(c);
                c.set8(136, 0);
            }
            _ => {
                start_wheels(c, s, t, sp, dt, 0, class, true);
                latch_all(c);
                c.set8(136, 0);
            }
        },
        1 => match latched {
            3 => latch_all(c),
            2 => {
                if !manual && c.image[121] == 0 {
                    start_wheels(c, s, t, sp, dt, 2, class, true);
                }
                latch_all(c);
                c.set8(136, 0);
            }
            1 => {
                let second = c.image[136] != 0;
                start_wheels(c, s, t, sp, dt, 3, class, second);
                c.set8(136, heavy);
                latch_landed(c);
            }
            _ => {
                c.set8(136, heavy);
                start_wheels(c, s, t, sp, dt, 3, class, false);
                latch_landed(c);
            }
        },
        _ => {}
    }
}

/// `0x824B8D48`: a wheel landing in the first free entry; a heavy landing
/// (class 2) plays class 1 plus the heavy layer, unless `flag` and the
/// second-layer latch both hold.
#[allow(clippy::too_many_arguments)]
fn start_wheels(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32, mode: u32, class: i32, flag: bool) {
    let Some(k) = (0..4).find(|k| c.c32(140 + 24 * k) == 0) else { return };
    let mut class = class as u32;
    let mut second = false;
    if class as i32 > 1 {
        class = 1;
        second = !flag || c.image[136] == 0;
    }
    let heavy = second.then(|| landing_id(c, s, t, mode, 2));
    let (sound, out) = landing_id(c, s, t, mode, class);
    let e = 140 + 24 * k;
    c.set32(e + 4, mode);
    c.set32(e + 12, out);
    c.set32(e + 8, class);
    let h = start(sp, 0, sound, dt);
    c.set32(e, h);
    if let Some((sound, out)) = heavy
        && c.c32(236) == 0
    {
        c.set32(240, mode);
        c.set32(248, out);
        let h = start(sp, 0, sound, dt);
        c.set32(236, h);
    }
}

/// `0x824B9948`: the foot slaps, a foot coming down on the deck fast
/// enough (skater `+276`/`+280`), and the delayed slap after skater `+769`.
fn slaps(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    if (s.r32(200) as i32) > 1 {
        // 0x820D71E8 (0.01).
        let threshold = || float(t, 0x823D_59FB_4632_4175) * f32::from_bits(0x3C23_D70A);
        if s.r8(616) != 0 && s.rf(276) > threshold() && c.c32(112) == 0 {
            slap(c, t, sp, 0, dt);
        }
        if s.r8(615) != 0 && s.rf(280) > threshold() && c.c32(116) == 0 {
            slap(c, t, sp, 1, dt);
        }
    }
    if s.r8(769) != 0 {
        c.set8(484, 1);
    } else if c.image[484] != 0 {
        if c.c32(112) == 0 {
            slap(c, t, sp, 0, dt);
        }
        c.set8(484, 0);
    }
}

/// Starts the foot slap sound (`0x824B97A8`).
fn slap(c: &mut Component, t: &Tuning, sp: &mut Splices, foot: u32, dt: f32) {
    let key = if c.image[484] != 0 { 0xF286_6EE0_540C_DF05 } else { 0x733C_45DF_5B63_8ECB };
    let h = start(sp, SLOT_FOLEY, id(t, 24, key, foot), dt);
    c.set32(112 + 4 * foot as usize, h);
}

/// `0x824B95A0`: the catch, a foot landing back on the deck after time off
/// it; both feet together count as one.
fn catch(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    let left = s.r8(615) != 0;
    let right = s.r8(616) != 0;
    if !left {
        c.setf(360, c.rf(360) + dt);
    }
    if !right {
        c.setf(352, c.rf(352) + dt);
    }
    let right_down = right && c.image[348] == 0;
    let left_down = left && c.image[356] == 0;
    if right_down && left_down {
        c.set32(364, 0);
        let (r, l) = (c.rf(352), c.rf(360));
        c.setf(368, if r - l >= 0.0 { r } else { l });
        let (a, b) = (s.rf(272), s.rf(268));
        catch_count(c, s, t, sp, 2, if a - b >= 0.0 { a } else { b });
        c.setf(352, 0.0);
        c.setf(368, 0.0);
        c.setf(360, 0.0);
    } else if right_down {
        c.set32(364, c.c32(364).wrapping_add(1));
        catch_count(c, s, t, sp, 0, s.rf(268));
        c.setf(352, 0.0);
    } else if left_down {
        c.set32(364, c.c32(364).wrapping_add(1));
        catch_count(c, s, t, sp, 1, s.rf(272));
        c.setf(360, 0.0);
    }
    c.set8(348, right as u8);
    c.set8(356, left as u8);
    if s.r8(676) != 0 || s.r8(725) != 0 || s.r8(724) != 0 {
        c.set8(476, 0);
    }
    if !right && !left {
        if s.r8(372) != 0 {
            if c.image[476] == 0 {
                c.set32(480, s.r32(304));
            }
            c.set8(476, 1);
        }
    } else if c.image[476] != 0 && right && left {
        c.set8(476, 0);
    }
}

/// `0x824B9508`: the catch variant from the feet counted so far.
fn catch_count(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, foot: u32, speed: f32) {
    match c.c32(364) {
        0 => catch_start(c, s, t, sp, 2, foot, speed),
        1 => catch_start(c, s, t, sp, 0, foot, speed),
        2 => {
            catch_start(c, s, t, sp, 1, foot, speed);
            c.set32(364, 0);
        }
        _ => {}
    }
}

/// `0x824B9268`: starts the catch for `foot` (2 both) when that foot was
/// off the deck long enough and lands fast enough.
fn catch_start(c: &mut Component, _s: &SkaterImage, t: &Tuning, sp: &mut Splices, mut mode: u32, foot: u32, speed: f32) {
    // 0x82063A48 (0.001).
    let threshold = float_at(t, 108, 0x6BFB_6A3F_22AA_F797, 0) * f32::from_bits(0x3A83_126F);
    if !(c.rf(352 + 8 * foot as usize) > threshold) || speed < float_at(t, 108, 0xB63C_C740_D29A_D118, 0) {
        return;
    }
    let mut cat = 0;
    if c.image[502] != 0 {
        c.set8(502, 0);
        mode = 3;
        cat = 1;
    } else if c.image[476] != 0 {
        mode = 3;
        cat = match c.c32(480) {
            1 => 1,
            2 => 2,
            _ => 0,
        };
    } else if speed >= float_at(t, 108, 0xE1F8_7C49_37E3_CCA2, 0) {
        cat = 2;
    } else if speed >= float_at(t, 108, 0x7D1C_A200_987A_A109, 0) {
        cat = 1;
    }
    let sound = catch_id(t, mode, cat, c.image[484] != 0);
    let off = 100 + 4 * foot as usize;
    if c.c32(off) == 0 {
        let h = start_positional(sp, sound);
        c.set32(off, h);
    }
}

/// The catch sound id for a mode and category (`0x824C0B90`).
fn catch_id(t: &Tuning, mode: u32, cat: u32, slap: bool) -> i32 {
    let key = match (slap, mode) {
        (false, 0) => 0x6F13_3132_EF3F_8063,
        (false, 1) => 0xE518_088F_4E61_203A,
        (false, 3) => 0x3AC5_AA2D_2010_B49C,
        (false, _) => 0x4CFF_3685_5CA0_4B6F,
        (true, 0) => 0xDB56_9526_76D7_122E,
        (true, 1) => 0x0ACF_CB7E_532B_7A0B,
        (true, _) => 0x7EA3_7827_E422_8614,
    };
    id(t, 108, key, cat)
}

/// `0x824BBB28`: the push plant when a push starts and its release when
/// it ends, by the surface type (`0x82494EB8`, map row `+20`).
fn pushes(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    let pushing = s.r8(333) != 0 || s.r8(334) != 0;
    let latched = c.image[123] != 0;
    if pushing == latched {
        return;
    }
    let surface = s.r32(620) as i32;
    let kind = if surface < 143 { surface_field(t, surface, 20) } else { 0 };
    let keys: [u64; 5] = if pushing {
        [0x1A5D_3CDB_A6C1_60D0, 0x6C93_C9BA_D7B0_7C6B, 0x9CCA_BF46_584C_A16C, 0x5755_3DC3_A33C_9B38, 0x4F13_8972_C957_C8AF]
    } else {
        [0x2D97_D30A_EA78_BCE2, 0x73CB_6988_2A79_481B, 0x767F_6CAE_ACB7_48C8, 0xA17B_A199_4B76_6B55, 0x8D8E_F475_983B_33A8]
    };
    let sound = keys.get(kind as usize).map_or(0, |&k| id(t, 24, k, 0));
    // 0x824BB8D8 (plant, +80/+84), 0x824BBA00 (release, +88/+92).
    let (kind_at, handle_at) = if pushing { (80, 84) } else { (88, 92) };
    release(c, sp, handle_at);
    c.set32(kind_at, kind);
    let h = start(sp, SLOT_FOLEY, sound, dt);
    c.set32(handle_at, h);
    c.set8(123, pushing as u8);
}

/// `0x824B85B0`: the sounds on skater `+688`/`+689` (Skeleton `+602`/`+603`)
/// rising (`0x824B8310`) and falling (`0x824B8448`).
fn skeleton_edges(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    let flags = [s.r8(688) != 0, s.r8(689) != 0];
    for side in [1usize, 0] {
        if flags[side] && c.image[376 + 8 * side] == 0 {
            release(c, sp, 372 + 8 * side);
            let key = match (s.r8(332) == 0) as u32 {
                0 => 0xA7B3_2EC5_FF49_97F5,
                _ => 0xF17E_EB71_F18C_EAF3,
            };
            let h = start(sp, 0, id(t, 112, key, 0), dt);
            c.set32(372 + 8 * side, h);
        }
    }
    for side in [1usize, 0] {
        if !flags[side] && c.image[392 + 8 * side] != 0 {
            release(c, sp, 388 + 8 * side);
            let h = start(sp, 0, id(t, 112, 0xA7B7_AE2A_6C25_670F, 0), dt);
            c.set32(388 + 8 * side, h);
        }
    }
    for side in 0..2 {
        c.set8(376 + 8 * side, flags[side] as u8);
        c.set8(392 + 8 * side, flags[side] as u8);
    }
}

/// `0x824C0DA8`: after a landing, trick 38 with skater `+792`.
fn trick_38(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    if s.r8(332) == 0 {
        c.set8(485, 1);
    }
    if c.image[485] == 0 || s.r32(348) as i32 != 38 || s.r8(792) == 0 {
        return;
    }
    release(c, sp, 488);
    let h = start(sp, SLOT_FOLEY, id(t, 116, 0x9538_0CD0_7CD4_21B7, 0), dt);
    c.set32(488, h);
    c.set8(485, 0);
}

/// One splice instance's per-frame update: `None` once it has finished
/// (destroyed, slot cleared).
fn live(c: &mut Component, sp: &mut Splices, off: usize) -> Option<u32> {
    let h = c.c32(off);
    if h == 0 {
        return None;
    }
    if !sp.is_playing(h) {
        sp.destroy(h);
        c.set32(off, 0);
        return None;
    }
    Some(h)
}

/// `0x824BE130` (after the mix map): every splice instance's per-frame
/// parameters, in the game's order.
pub fn instances(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    let p = c.params;
    let pitch = p.pitch(1) as f32 * PITCH;
    let pan = p.u16(0) as f32 * PAN;
    let local = if local(c, t) { 1.0 } else { 0.0 };
    let params = |gain: f32, pan_scale: f32| Params { gain, pitch, pan, dt, pan_scale, stretch: 1.0 };
    // 0x824BEBD8: the catches (positional).
    for off in [100, 104, 108] {
        if let Some(h) = live(c, sp, off) {
            sp.update(h, params(p.u15(8) as f32 * LEVEL, 0.0));
        }
    }
    // 0x824BF4A8: the foot slaps.
    for off in [112, 116] {
        if let Some(h) = live(c, sp, off) {
            sp.update(h, params(p.u15(9) as f32 * LEVEL, 0.0));
        }
    }
    // 0x824BE1B8: the pop, the speed sound, the wheel landings, the local
    // layers and the positional landing.
    let h = c.c32(60);
    if h != 0 {
        if !sp.is_playing(h) {
            sp.destroy(h);
            for off in [60, 64, 68, 72, 76] {
                c.set32(off, 0);
            }
        } else {
            let key = if c.c32(68) == 0 { 0x8BF3_668C_F799_4CD4 } else { 0x212B_6F75_D11A_A39B };
            let scale = float_at(t, 24, key, c.c32(64));
            let gain = fctiwz(p.u15(2) as f32 * scale) as f32 * LEVEL;
            sp.update(h, params(gain, local));
        }
    }
    if let Some(h) = live(c, sp, 96) {
        sp.update(h, params(p.u15(7) as f32 * LEVEL, 0.0));
    }
    for k in 0..4 {
        let e = 140 + 24 * k;
        let h = c.c32(e);
        if h == 0 {
            continue;
        }
        if !sp.is_playing(h) {
            sp.destroy(h);
            c.set32(e, 0);
            c.set32(e + 8, 3);
            c.set32(e + 4, 5);
            for off in [e + 12, e + 16, e + 20] {
                c.set32(off, 0);
            }
        } else {
            let gain = landing_level(c, t, c.c32(e + 4), c.c32(e + 8), c.c32(e + 12));
            sp.update(h, params(gain, local));
        }
    }
    let h = c.c32(236);
    if h != 0 {
        if !sp.is_playing(h) {
            sp.destroy(h);
            c.set32(236, 0);
            c.set32(244, 3);
            c.set32(240, 5);
            for off in [248, 252, 256] {
                c.set32(off, 0);
            }
        } else {
            let gain = landing_level(c, t, c.c32(240), 2, c.c32(248));
            sp.update(h, params(gain, local));
        }
    }
    if let Some(h) = live(c, sp, 52) {
        sp.update(h, params(p.u15(12) as f32 * LEVEL, local));
    }
    if let Some(h) = live(c, sp, 56) {
        sp.update(h, params(p.u15(13) as f32 * LEVEL, local));
    }
    if let Some(h) = live(c, sp, 496) {
        sp.update(h, params(p.u15(3) as f32 * LEVEL, 0.0));
    }
    if c.rf(124) > 0.0 {
        c.setf(124, c.rf(124) - dt);
    }
    // 0x824BED90: the manual landing.
    if let Some(h) = live(c, sp, 36) {
        let gain = landing_level(c, t, 4, 0, c.c32(40));
        sp.update(h, params(gain, 0.0));
    }
    // 0x824BF268: the push plant and its release.
    for off in [84, 92] {
        if let Some(h) = live(c, sp, off) {
            sp.update(h, params(p.u15(6) as f32 * LEVEL, 0.0));
        }
    }
    // 0x824BF728: the skeleton edge sounds, at their own pitch.
    let edge_pitch = p.pitch(11) as f32 * PITCH;
    for (base, index) in [(372, 0), (388, 1)] {
        let scale = float_at(t, 112, 0x25DC_B888_413D_570D, index);
        for side in 0..2 {
            if let Some(h) = live(c, sp, base + 8 * side) {
                let gain = fctiwz(p.u15(10) as f32 * scale) as f32 * LEVEL;
                sp.update(h, Params { pitch: edge_pitch, ..params(gain, 0.0) });
            }
        }
    }
    // 0x824C0660 / 0x824C0940: the drags, kept while their skater flag
    // (`+310`, `+320`) holds even once they stop playing.
    for (off, field, output) in [(456, 310, 19), (368, 320, 20)] {
        let h = c.c32(off);
        if h == 0 {
            continue;
        }
        if !sp.is_playing(h) {
            if s.r8(field) == 0 {
                sp.destroy(h);
                c.set32(off, 0);
            }
        } else {
            sp.update(h, params(p.u15(output) as f32 * LEVEL, local));
        }
    }
    // 0x824C0F48: trick 38.
    if let Some(h) = live(c, sp, 488) {
        sp.update(h, params(p.u15(9) as f32 * LEVEL, 0.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn setup() -> Option<(Tuning, Splices, u32)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/audio-cache");
        let mut t = Tuning::load(&dir).ok()?;
        let mut sp = Splices::default();
        sp.load(&dir, super::super::splices::SLOT_COLLISIONS, "Skate_Collisions").unwrap();
        sp.load(&dir, SLOT_FOLEY, "sk8_foley").unwrap();
        let controller = t.alloc(80);
        t.w8(controller + 72, 1);
        Some((t, sp, controller))
    }

    fn component(controller: u32) -> Component {
        let mut c = Component::new("foot_drag", 512, vec![]);
        c.set32(16, controller);
        c.set32(28, controller);
        c
    }

    #[test]
    fn takeoff_edge_pops_and_excluded_tricks_do_not() {
        let Some((t, mut sp, controller)) = setup() else { return };
        let mut col = Collisions::default();
        let dt = 1.0 / 60.0;
        for (trick, pops) in [(0, true), (31, false), (-1, false)] {
            let mut c = component(controller);
            let mut s = SkaterImage::default();
            s.w8(343, 1);
            s.w32(348, trick as u32);
            s.wf(208, 5.0);
            for _ in 0..3 {
                takeoff(&mut c, &s, &t, &mut sp, &mut col, dt);
            }
            assert_eq!(c.c32(60), 0);
            s.w8(332, 1);
            takeoff(&mut c, &s, &t, &mut sp, &mut col, dt);
            assert_eq!(c.c32(60) != 0, pops, "trick {trick}");
            assert_eq!(c.inputs[0], if pops { 32767 } else { 0 });
            if pops {
                // Above 4 m/s: the speed sound plays too; the local layer too.
                assert_ne!(c.c32(96), 0);
                assert_ne!(c.c32(52), 0);
                assert_eq!(word(&t, 24, 0x3C1E_3B96_5A93_594A, c.c32(64)), 1097);
            }
            // Held in the air: no second pop.
            let h = c.c32(60);
            for _ in 0..30 {
                takeoff(&mut c, &s, &t, &mut sp, &mut col, dt);
                assert_eq!(c.c32(60), h);
                instances(&mut c, &s, &t, &mut sp, dt);
                sp.service();
            }
        }
        assert!(sp.problems.is_empty(), "{:?}", sp.problems);
        assert!(!sp.started.is_empty());
    }

    #[test]
    fn drags_follow_their_skater_flags() {
        let Some((t, mut sp, controller)) = setup() else { return };
        let dt = 1.0 / 60.0;
        let mut c = component(controller);
        // Every mix output 0: unit pitch, so the sounds' clocks run.
        c.params.words = Some([0; 16]);
        let mut s = SkaterImage::default();
        s.w32(620, 143);
        // The drag collection's sounds by speed band, for both flags.
        let ids: Vec<i32> = [0.0, 3.0, 30.0].iter()
            .flat_map(|&v| { s.wf(208, v); [drag_id(&s, &t, true), drag_id(&s, &t, false)] })
            .collect();
        assert!(ids.iter().all(|&id| id > 0), "{ids:?}");
        s.wf(208, 0.0);
        s.w8(310, 1);
        s.w8(320, 1);
        drag_a(&mut c, &s, &t, &mut sp, dt);
        drag_b(&mut c, &s, &t, &mut sp, dt);
        let (a, b) = (c.c32(456), c.c32(368));
        assert!(a != 0 && b != 0);
        // Held flags keep the handles, even once the sounds stop.
        for _ in 0..600 {
            drag_a(&mut c, &s, &t, &mut sp, dt);
            drag_b(&mut c, &s, &t, &mut sp, dt);
            instances(&mut c, &s, &t, &mut sp, dt);
            sp.service();
        }
        assert_eq!((c.c32(456), c.c32(368)), (a, b));
        s.w8(310, 0);
        s.w8(320, 0);
        for _ in 0..600 {
            instances(&mut c, &s, &t, &mut sp, dt);
            sp.service();
        }
        assert_eq!((c.c32(456), c.c32(368)), (0, 0));
        assert!(sp.problems.is_empty(), "{:?}", sp.problems);
    }

    #[test]
    fn replay_drag_input_outside_replays() {
        let mut c = component(0);
        c.setf(464, 5.0);
        c.set32(336, 0);
        replay_drag(&mut c);
        // 0/0 is NaN, which fsel saturates.
        assert_eq!((c.inputs[9], c.rf(464)), (32767, 0.0));
        c.setf(336, 0.5);
        replay_drag(&mut c);
        assert_eq!(c.inputs[9], 0);
    }

    #[test]
    fn retail_sound_ids() {
        let Some((t, _, controller)) = setup() else { return };
        let c = component(controller);
        let s = SkaterImage::default();
        // The retail trace: landing 1095, wheel landings 1051..1060 (1061
        // and 1127, single wheels, did not occur in it), manual
        // landing 1050, slaps 94/95, push plants 84/88 and 85/89, edges
        // 1125/1126, trick 38 94.
        assert_eq!(id(&t, 24, 0x633F_A94E_39C1_AE8F, 0), 1095);
        let wheels: Vec<i32> = (0..4).flat_map(|m| (0..3).map(move |k| (m, k)))
            .map(|(m, k)| landing_id(&c, &s, &t, m, k).0).collect();
        assert_eq!(wheels, [1051, 1052, 1053, 1054, 1055, 1056, 1057, 1058, 1059, 1060, 1061, 1127]);
        assert_eq!(landing_id(&c, &s, &t, 4, 0).0, 1050);
        assert_eq!([id(&t, 24, 0x733C_45DF_5B63_8ECB, 0), id(&t, 24, 0x733C_45DF_5B63_8ECB, 1)], [95, 94]);
        assert_eq!(id(&t, 116, 0x9538_0CD0_7CD4_21B7, 0), 94);
        assert_eq!(id(&t, 112, 0xA7B7_AE2A_6C25_670F, 0), 1126);
        for cat in 0..3 {
            assert_ne!(catch_id(&t, 0, cat, false), 0);
        }
    }

    #[test]
    fn four_wheel_landing_plays_and_latches() {
        let Some((t, mut sp, controller)) = setup() else { return };
        let mut col = Collisions::default();
        let dt = 1.0 / 60.0;
        let mut c = component(controller);
        let mut s = SkaterImage::default();
        s.w32(200, 4);
        s.wf(208, 5.0);
        for _ in 0..3 {
            update(&mut c, &s, &t, &mut sp, &mut col, dt);
        }
        s.w8(332, 1);
        s.wf(236, 0.8);
        update(&mut c, &s, &t, &mut sp, &mut col, dt);
        s.w8(332, 0);
        for i in 0..4 {
            s.w8(464 + i, 1);
            s.w32(448 + 4 * i, 2);
        }
        update(&mut c, &s, &t, &mut sp, &mut col, dt);
        // The landing: input 1, input 2 from class 2, the local layer.
        assert_eq!((c.inputs[1], c.inputs[2]), (32767, 32767));
        assert_ne!(c.c32(56), 0);
        // The wheels: class 1 plus the heavy layer, inputs 4 and 5.
        assert_ne!(c.c32(140), 0);
        assert_ne!(c.c32(236), 0);
        assert_eq!((c.inputs[3], c.inputs[4], c.inputs[5]), (0, 32767, 13106));
        assert_eq!(&c.image[132..136], &[1, 1, 1, 1]);
        // Still down: nothing new.
        let h = c.c32(140);
        update(&mut c, &s, &t, &mut sp, &mut col, dt);
        assert_eq!((c.c32(140), c.c32(164)), (h, 0));
        for _ in 0..120 {
            instances(&mut c, &s, &t, &mut sp, dt);
            sp.service();
        }
        assert!(sp.problems.is_empty(), "{:?}", sp.problems);
    }

    #[test]
    fn foot_slap_and_push_plant() {
        let Some((t, mut sp, controller)) = setup() else { return };
        let mut col = Collisions::default();
        let dt = 1.0 / 60.0;
        let mut c = component(controller);
        let mut s = SkaterImage::default();
        s.w32(200, 4);
        s.w8(616, 1);
        s.wf(276, 100.0);
        s.w8(333, 1);
        update(&mut c, &s, &t, &mut sp, &mut col, dt);
        assert_ne!(c.c32(112), 0);
        assert_ne!(c.c32(84), 0);
        assert_eq!(c.image[123], 1);
        s.w8(333, 0);
        update(&mut c, &s, &t, &mut sp, &mut col, dt);
        assert_ne!(c.c32(92), 0);
        assert!(sp.problems.is_empty(), "{:?}", sp.problems);
    }
}

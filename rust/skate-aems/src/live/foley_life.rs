//! Game-side flip, cloth and footstep component ownership. Offsets name the
//! independently inspected TU3 state image, not guest pointers to execute.
use super::{creation::Message, driver::Component, frame::SkaterImage, splices::Splices, tuning::Tuning};
use crate::{glue::env::Send, ops::fctiwz};

#[derive(Default)]
pub struct Prep {
    pub sends: Vec<Send>,
    pub events: Vec<DirectEvent>,
}
/// Non-AEMS requests retain their source selector inputs. A consumer must
/// resolve the owned game's event tables before choosing/starting a sound.
#[derive(Clone, Debug, PartialEq)]
pub enum DirectEvent {
    TrickReaction {
        event_id: u16,
        payload: [i32; 3],
    },
}
#[derive(Default, Clone, Copy)]
pub struct SessionConfig {
    /// Active owned Skate challenge collection's alternate-flip-audio flag.
    /// GTA's free-skate session has no active Skate challenge collection.
    pub alternate_flip_audio: bool,
    pub button_mask: u32,
    pub force_third_button: bool,
}
fn vi(t: &Tuning, root: u32, key: u64) -> i32 {
    t.g32(t.tuning_at(root, key, 0)) as i32
}
fn vf(t: &Tuning, root: u32, key: u64) -> f32 {
    f32::from_bits(vi(t, root, key) as u32)
}
fn cf(c: &Component, at: usize) -> f32 {
    f32::from_bits(c.c32(at))
}
fn local(c: &Component, t: &Tuning) -> bool {
    t.g8(c.c32(28).wrapping_add(72)) != 0
}

/// 824CBD98, omitting base-class ownership and guest vtable storage.
pub fn init_flips(c: &mut Component) {
    for at in [36, 40, 44, 64, 68, 72, 84, 92, 96] {
        c.set32(at, 0);
    }
    for at in [48, 52, 56, 60] {
        c.set32(at, u32::MAX);
    }
    for at in [76, 77, 78, 79, 80, 88, 100] {
        c.set8(at, 0);
    }
}
fn angular(s: &SkaterImage, t: &Tuning, at: usize, threshold: u64, denominator: u64) -> i32 {
    let v = fctiwz(s.rf(at).abs() / vf(t, 72, denominator) * 1000.).min(1000);
    if v < vi(t, 72, threshold) { 0 } else { v }
}
/// 824CBFB8: create the flip wrapper only at an actual source trigger.
pub fn flip_start(
    c: &mut Component,
    s: &SkaterImage,
    t: &Tuning,
    config: SessionConfig,
) -> Vec<Send> {
    let push = s.r8(332) == 0 && local(c, t) && s.r8(310) != 0;
    if !((s.r8(332) != 0 && s.r8(343) != 0) || push) || c.c32(36) != 0 {
        return vec![];
    }
    let trick = if push { 34 } else { s.r32(348) as i32 };
    c.set32(52, trick as u32);
    if matches!(trick, -1 | 35 | 36) {
        return vec![];
    }
    let x = angular(s, t, 480, 0x5C73_CF6A_0D50_C8D8, 0x1494_BB20_854C_155C);
    let y = angular(s, t, 484, 0xEE15_7886_DE5D_3C97, 0x02D3_9586_635F_B1A3);
    let z = angular(s, t, 488, 0x9A03_1662_5B63_CD99, 0x8EDB_ACCB_A6FE_46AD);
    let controller = c.c32(16);
    let is_local = t.g8(controller.wrapping_add(72)) != 0;
    vec![Send::create(
        36,
        &Message::Flips.pack(&[
            z,
            y,
            x,
            fctiwz(s.rf(220) * 500.),
            trick,
            (s.r8(224) == 0) as i32,
            vi(t, 72, 0x99E6_FF02_4834_E4C7),
            vi(t, 72, 0xD2D0_EBAC_4384_2F6D),
            vi(t, 72, 0x13C1_55A1_81A5_5BA4),
            vi(t, 72, 0xB565_D4D7_6312_8252),
            config.alternate_flip_audio as i32,
            (is_local && t.g32(controller.wrapping_add(64)) == 0) as i32,
            is_local as i32,
            if is_local { c.params.u15(6) } else { 0 },
            vi(t, 140, 0xD9BE_1F2F_1A72_FEE8),
        ]),
    )]
}
/// 824B71C0: cloth_trick creation layout (eleven message words).
fn cloth_message(trick: i32, gain: i32) -> [i32; 11] {
    [
        0,
        0,
        4096,
        0,
        25000,
        0,
        0,
        0,
        1,
        trick.clamp(0, 40),
        gain.clamp(0, 32767),
    ]
}
/// 824CC590: a changed trick releases the old object; replacement starts on
/// the following prep, exactly as in the native component.
pub fn cloth_active(c: &mut Component, trick: i32, t: &Tuning) -> Vec<Send> {
    if trick == -1 {
        return vec![];
    }
    if c.c32(40) == 0 {
        c.set32(56, trick as u32);
        vec![Send::create(
            40,
            &cloth_message(trick, vi(t, 140, 0x4B6E_2D79_A845_2D9B)),
        )]
    } else if c.c32(56) as i32 != trick {
        vec![Send::release(40)]
    } else {
        vec![]
    }
}
/// 824CC680: keep post-trick cloth alive for the preceding trick duration.
pub fn cloth_tail(c: &mut Component, trick: i32, t: &Tuning, dt: f32) -> Vec<Send> {
    let ended = trick == -1 && c.c32(48) as i32 != -1;
    if ended {
        c.setf(68, cf(c, 64));
        c.setf(64, 0.);
    } else if trick != -1 {
        c.setf(64, cf(c, 64) + dt);
    }
    if cf(c, 68) > 0. {
        if c.c32(60) as i32 == -1 || !ended {
            return vec![];
        }
        let mut out = Vec::new();
        if c.c32(44) != 0 {
            out.push(Send::release(44));
        }
        out.push(Send::create(
            44,
            &cloth_message(c.c32(60) as i32, vi(t, 140, 0x4B6E_2D79_A845_2D9B)),
        ));
        out
    } else if c.c32(44) != 0 {
        vec![Send::release(44)]
    } else {
        vec![]
    }
}
/// 824CD170's integer controller-feedback slew.
pub fn button_feedback(c: &mut Component, t: &Tuning, dt: f32, config: SessionConfig) {
    if !(dt > 0.) {
        c.set32(72, 0);
        return;
    }
    let collection = t.collection(0xC183_1BDB_6CB1_B1EA, 0x47EC_76B4_F9FC_79F6);
    let value = |key| t.g32(t.attrib(collection, key));
    let key = if config.force_third_button {
        Some(0x36F8_D124_86A9_29D1)
    } else if config.button_mask & 0x8000 != 0 {
        Some(0xB601_DFAB_3AF7_DE66)
    } else if config.button_mask & 0x4000 != 0 {
        Some(0x0844_1EA8_E801_9665)
    } else if config.button_mask & 0x2000 != 0 {
        Some(0x36F8_D124_86A9_29D1)
    } else {
        None
    };
    let mut target = key.map(|k| value(k) as i32).unwrap_or(0);
    let down = fctiwz(f32::from_bits(value(0x6B57_BD44_C0E0_B267)) * dt);
    let up = fctiwz(f32::from_bits(value(0x57AE_D5FB_C374_D8F1)) * dt);
    let held = c.c32(72) as i32;
    if target < held && held.wrapping_sub(target) > down {
        target = held.wrapping_sub(down);
    } else if target > held && target.wrapping_sub(held) > up {
        target = held.wrapping_add(up);
    }
    c.set32(72, target as u32);
}
/// 824CD390 emits source reaction IDs, retaining the source payload fields
/// +72/+76/+80. These are the event-manager path, separate from AEMS flips.
pub fn trick_reactions(
    c: &mut Component,
    s: &SkaterImage,
    t: &Tuning,
    dt: f32,
) -> Vec<DirectEvent> {
    let mut out = Vec::new();
    if !local(c, t) {
        return out;
    }
    let active = s.r8(332) != 0;
    if active {
        c.setf(84, 2.);
    } else if cf(c, 84) > 0. {
        c.setf(84, cf(c, 84) - dt);
    } else {
        c.setf(84, 0.);
        for at in [76, 77, 78, 79, 80, 88] {
            c.set8(at, 0);
        }
        c.set32(92, 0);
        c.set32(96, 0);
    }
    let mut emit = |event_id, payload| out.push(DirectEvent::TrickReaction { event_id, payload });
    if s.r8(375) == 0 && c.image[100] != 0 && s.r8(676) == 0 && s.r8(717) == 0 && s.r32(368) != 0 {
        emit(0x606d, [s.r32(368) as i32, 0, 0]);
    }
    c.set8(100, s.r8(375));
    if c.image[76] == 0 && s.r32(360) != 0 {
        let v = s.r32(360);
        c.set32(92, v);
        emit(0x606e, [0, v as i32, 0]);
        c.set8(76, 1);
    }
    if c.image[77] == 0 && s.r32(356) != 0 {
        let v = s.r32(356);
        c.set32(96, v);
        emit(0x6070, [0, 0, v as i32]);
        c.set8(77, 1);
    }
    if c.image[78] == 0 && s.r32(348) == 38 {
        emit(0x606f, [0; 3]);
        c.set8(78, 1);
    }
    if c.image[79] == 0 && s.r32(364) != 0 && s.r8(374) != 0 {
        c.set8(79, 1);
        c.set8(80, 1);
    }
    if c.image[80] == 0 && active && s.r8(373) != 0 {
        emit(0x6072, [0; 3]);
        c.set8(80, 1);
    }
    if s.r8(676) != 0 && c.image[88] == 0 {
        if c.image[76] != 0 {
            if c.c32(92) != 0 {
                emit(0x6078, [0, c.c32(92) as i32, 0]);
                c.set32(92, 0);
                c.set8(88, s.r8(676));
                return out;
            }
        } else if c.image[77] != 0 && c.c32(96) != 0 {
            emit(0x6079, [0, 0, c.c32(96) as i32]);
            c.set32(96, 0);
        }
    }
    c.set8(88, s.r8(676));
    out
}
/// AEMS stages of 824CBEB0 in native order. The direct event stages are
/// separately represented so a missing event consumer cannot be silent.
pub fn prep_flips(
    c: &mut Component,
    s: &SkaterImage,
    t: &Tuning,
    dt: f32,
    mut config: SessionConfig,
) -> Prep {
    // Host extension: actual selected controller XInput buttons, not a native offset.
    config.button_mask = s.r32(1000);
    let mut out = Prep::default();
    if !local(c, t) {
        return out;
    }
    let trick = s.r32(348) as i32;
    if trick != -1 {
        c.set32(60, s.r32(352));
    }
    out.sends.extend(flip_start(c, s, t, config));
    out.sends.extend(cloth_active(c, trick, t));
    out.sends.extend(cloth_tail(c, trick, t, dt));
    c.set32(48, trick as u32);
    c.setf(68, if cf(c, 68) > 0. { cf(c, 68) - dt } else { 0. });
    button_feedback(c, t, dt, config);
    out.events.extend(trick_reactions(c, s, t, dt));
    out
}

/// 824D7CE8 embedded foot-contact initializer.
fn init_foot(c: &mut Component, base: usize) {
    c.image[base..base + 184].fill(0);
    c.set32(base + 20, 2);
    for start in [24, 64, 104, 144] {
        c.setf(base + start + 8, 96000.);
        c.setf(base + start + 12, 96000.);
        c.setf(base + start + 16, 1.);
        c.setf(base + start + 20, 3.);
        c.setf(base + start + 24, 1.);
        c.set8(base + start + 28, 6);
    }
}
/// 824E8F98; base controller/skater bindings remain owned by the driver.
pub fn init_footsteps(c: &mut Component) {
    init_foot(c, 36);
    init_foot(c, 220);
    c.image[404..496].fill(0);
    c.set32(456, 2);
}
/// 82481E10: 16-knot source curve, including native endpoint/duplicate-knot rules.
pub fn curve16(t: &Tuning, address: u32, value: f32) -> f32 {
    knots(t, address + 16, address + 80, 16, value)
}
/// 82481E10(count, xs, ys, value): `ys[0]` below the first knot, the last y
/// from the last knot on, else linear between the first knot above `value`
/// and its predecessor (that knot's y when they coincide).
pub fn knots(t: &Tuning, xs: u32, ys: u32, count: u32, value: f32) -> f32 {
    let x = |i: u32| f32::from_bits(t.g32(xs + i * 4));
    let y = |i: u32| f32::from_bits(t.g32(ys + i * 4));
    if value < x(0) {
        return y(0);
    }
    if !(value < x(count - 1)) {
        return y(count - 1);
    }
    for i in 1..count {
        if value < x(i) {
            let span = x(i) - x(i - 1);
            if span > 0. {
                return ((y(i) - y(i - 1)) / span).mul_add(value - x(i - 1), y(i - 1));
            }
            return y(i);
        }
    }
    y(0)
}
/// 824E9FD8's persistent AEMS ownership; the local foot strikes follow in
/// `super::footsteps::strikes`.
pub fn footstep_objects(c: &mut Component, s: &SkaterImage, t: &Tuning) -> Prep {
    let mut out = Prep::default();
    let right = s.r32(732) as i32;
    let left = s.r32(728) as i32;
    c.set32(56, if right == 143 { 3 } else { right } as u32);
    c.set32(240, if left == 143 { 3 } else { left } as u32);
    let gain = vi(t, 140, 0xC014_A21D_0FF6_EDBA).wrapping_add(10);
    for slot in [220, 36] {
        if c.c32(slot) == 0 {
            out.sends.push(Send::create(
                slot as u32,
                &Message::Footsteps.pack(&[
                    0, 0, 4096, 0, 25000, 0, 0, 32767, 0, 0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0,
                    gain,
                ]),
            ));
        }
    }
    out
}
/// 824E9270: the prep fields, the AEMS objects and foot strikes (824E9FD8),
/// the scuffs off the board (824E9D10) and the whooshes (824E9678), before
/// the previous flags commit. Other skaters' sounds (824EBA08) and water
/// (824EBB58) are not ported.
pub fn prep_footsteps(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) -> Prep {
    let actor = c.c32(28);
    if actor == 0 || t.g8(actor.wrapping_add(52)) == 0 {
        return Prep::default();
    }
    let moving = s.r8(716) != 0;
    c.inputs[0] = if moving { 32767 } else { 0 };
    for (dest, source, key) in [
        (408, 212, 0xC3CD_069B_B1B1_6B58),
        (416, 284, 0xCF84_4597_AB96_EAF8),
        (412, 288, 0xCF84_4597_AB96_EAF8),
        (424, 292, 0x2363_1160_4A3C_1FB5),
        (420, 296, 0x2363_1160_4A3C_1FB5),
    ] {
        let v = curve16(t, t.tuning_at(92, key, 0), s.rf(source));
        c.set32(dest, fctiwz(v) as u32);
    }
    c.set8(52, s.r8(724));
    c.set8(53, 0);
    c.set8(236, s.r8(725));
    c.set8(237, 0);
    let armed = s.r8(768) == 0 && c.image[460] != 0;
    if armed {
        if c.image[54] != 0 {
            c.set32(464, 10);
        } else if c.image[238] != 0 {
            c.set32(468, 10);
        }
    }
    for at in [464, 468] {
        let n = c.c32(at) as i32;
        if n > 0 {
            c.set32(at, (n - 1) as u32);
        }
    }
    let out = footstep_objects(c, s, t);
    super::footsteps::strikes(c, s, t, sp, dt);
    if moving {
        super::footsteps::scuffs(c, s, t, sp, dt);
    }
    super::footsteps::whooshes(c, s, t, sp, moving, armed, dt);
    c.set8(404, moving as u8);
    c.set8(460, s.r8(768));
    for (to, from) in [(54, 52), (55, 53), (238, 236), (239, 237)] {
        c.set8(to, c.image[from]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bound(name: &'static str) -> (Component, SkaterImage, Tuning) {
        let mut c = Component::new(name, 512, vec![]);
        let mut t = Tuning::default();
        let controller = t.alloc(80);
        t.w8(controller + 72, 1);
        t.w8(controller + 52, 1);
        c.set32(16, controller);
        c.set32(28, controller);
        (c, SkaterImage::default(), t)
    }
    #[test]
    fn cloth_change_releases_before_replacement_and_tail_expires() {
        let (mut c, _s, t) = bound("flips");
        init_flips(&mut c);
        let first = cloth_active(&mut c, 12, &t);
        assert!(first[0].create);
        assert_eq!(first[0].words[9], 12);
        c.set32(40, 1);
        let changed = cloth_active(&mut c, 13, &t);
        assert!(changed[0].release);
        assert_eq!(c.c32(56), 12);
        c.set32(40, 0);
        assert!(cloth_active(&mut c, 13, &t)[0].create);
        c.set32(48, 13);
        c.set32(60, 13);
        c.setf(64, 0.5);
        assert!(cloth_tail(&mut c, -1, &t, 0.02)[0].create);
        assert_eq!(cf(&c, 68), 0.5);
        c.set32(44, 1);
        c.setf(68, 0.);
        assert!(cloth_tail(&mut c, -1, &t, 0.02)[0].release);
    }
    #[test]
    fn footstep_input_and_contact_edges_are_retained_separately() {
        let (mut c, mut s, t) = bound("steps");
        init_footsteps(&mut c);
        s.w8(716, 1);
        s.w8(724, 1);
        s.w32(732, 143);
        s.w32(728, 7);
        let mut sp = Splices::default();
        let out = prep_footsteps(&mut c, &s, &t, &mut sp, 1. / 60.);
        assert_eq!(out.sends.len(), 2);
        assert_eq!(c.inputs[0], 32767);
        assert_eq!(c.c32(56), 3);
        assert_eq!(c.c32(240), 7);
        assert!(out.events.is_empty());
        assert_eq!(c.image[54], 1);
        // The local strike built the eight slot submixes.
        assert_ne!(c.c32(36 + 24 + 32), 0);
    }
    #[test]
    fn reaction_latches_reset_after_trick_window() {
        let (mut c, mut s, t) = bound("flips");
        init_flips(&mut c);
        s.w8(332, 1);
        s.w32(360, 2);
        assert_eq!(
            trick_reactions(&mut c, &s, &t, 0.1),
            vec![DirectEvent::TrickReaction {
                event_id: 0x606e,
                payload: [0, 2, 0]
            }]
        );
        assert!(trick_reactions(&mut c, &s, &t, 0.1).is_empty());
        s.w8(676, 1);
        assert_eq!(
            trick_reactions(&mut c, &s, &t, 0.1),
            vec![DirectEvent::TrickReaction {
                event_id: 0x6078,
                payload: [0, 2, 0]
            }]
        );
        assert!(trick_reactions(&mut c, &s, &t, 0.1).is_empty());
    }
}

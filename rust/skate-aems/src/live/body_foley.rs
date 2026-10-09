//! The body component's splice sounds (vtable `0x822FCD10`, mix-map bank 6,
//! the component `body` lives in): the push plant (`+48`, on skater `+337`
//! rising) and the push start (`+52`, on skater `+335`), both cut by a
//! wipeout (`+676`). `+56` is the previous `+337`. Slot 9 (`0x824DBB68`)
//! runs the cloth falls (`0x824DBF10`), these sounds (`0x824DBBB8`) and the
//! body slide (`0x824DC0E8`); the AEMS lifetimes are `component_life::body`.
//! Both sounds play into a submix (root 140 `0xAEA5BA7E64515945`), which is
//! routing, the host's.

use super::driver::Component;
use super::frame::SkaterImage;
use super::splices::{Splices, SLOT_FOLEY};
use super::tuning::Tuning;
use crate::splice::Params;

/// `0x822F8898`, `0x822F890C`, `0x822F8C64`.
const LEVEL: f32 = 1.0 / 32767.0;
const PITCH: f32 = 1.0 / 4096.0;
const PAN: f32 = 360.0 / 65536.0;

/// Root 136 sound ids.
const PLANT: u64 = 0x6FD2_7315_7742_AC4B;
const START: u64 = 0x616C_E02A_A10D_596F;

fn local(c: &Component, t: &Tuning) -> f32 {
    if t.g8(c.c32(28).wrapping_add(72)) != 0 { 1.0 } else { 0.0 }
}

fn start(c: &Component, t: &Tuning, sp: &mut Splices, key: u64, dt: f32) -> u32 {
    let id = t.g32(t.tuning_at(136, key, 0)) as i32;
    let h = sp.create(SLOT_FOLEY, id);
    sp.play(h, 0, Params { pan_scale: local(c, t), ..Params::at_play(dt) });
    h
}

/// One tick of the body foley component (`0x824DBBB8`).
pub fn update(c: &mut Component, s: &SkaterImage, t: &Tuning, sp: &mut Splices, dt: f32) {
    let contact = s.r8(337) != 0;
    let pushing = s.r8(335) != 0;
    let bailed = s.r8(676) != 0;
    let h = c.c32(48);
    if h == 0 {
        if c.image[56] == 0 && contact {
            let h = start(c, t, sp, PLANT, dt);
            c.set32(48, h);
        }
    } else if pushing || bailed {
        sp.destroy(h);
        c.set32(48, 0);
    }
    c.set8(56, contact as u8);
    let h = c.c32(52);
    if pushing {
        if h != 0 {
            sp.destroy(h);
            c.set32(52, 0);
        }
        let h = start(c, t, sp, START, dt);
        c.set32(52, h);
    } else if h != 0 && bailed {
        sp.destroy(h);
        c.set32(52, 0);
    }
}

/// `0x824DC7D8` (slot 10, after the body slide and the cloth falls): both
/// sounds at mix output 4, pitch 3.
pub fn instances(c: &mut Component, t: &Tuning, sp: &mut Splices, dt: f32) {
    let p = c.params;
    let params = Params {
        gain: p.u15(4) as f32 * LEVEL,
        pitch: p.pitch(3) as f32 * PITCH,
        pan: p.u16(0) as f32 * PAN,
        dt,
        pan_scale: local(c, t),
        stretch: 1.0,
    };
    for off in [48, 52] {
        let h = c.c32(off);
        if h == 0 {
            continue;
        }
        if !sp.is_playing(h) {
            sp.destroy(h);
            c.set32(off, 0);
        } else {
            sp.update(h, params);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn push_sounds_follow_contact_and_push_edges() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/audio-cache");
        let Ok(mut t) = Tuning::load(&dir) else { return eprintln!("SKIP: no audio cache") };
        let mut sp = Splices::default();
        sp.load(&dir, SLOT_FOLEY, "sk8_foley").unwrap();
        let controller = t.alloc(80);
        t.w8(controller + 72, 1);
        let mut c = Component::new("body", 192, vec![]);
        c.set32(28, controller);
        c.params.words = Some([0; 16]);
        // The retail splice tally: slot 7 ids 73/74.
        let ids = [PLANT, START].map(|k| t.g32(t.tuning_at(136, k, 0)));
        assert!(ids.iter().all(|&id| id == 73 || id == 74), "{ids:?}");
        let dt = 1.0 / 60.0;
        let mut s = SkaterImage::default();
        s.w8(337, 1);
        update(&mut c, &s, &t, &mut sp, dt);
        let plant = c.c32(48);
        assert_ne!(plant, 0);
        // Held contact: no restart.
        update(&mut c, &s, &t, &mut sp, dt);
        assert_eq!(c.c32(48), plant);
        // A push start cuts the plant and starts its own sound.
        s.w8(335, 1);
        update(&mut c, &s, &t, &mut sp, dt);
        assert_eq!(c.c32(48), 0);
        assert_ne!(c.c32(52), 0);
        s.w8(335, 0);
        s.w8(676, 1);
        update(&mut c, &s, &t, &mut sp, dt);
        assert_eq!((c.c32(48), c.c32(52)), (0, 0));
        assert!(sp.problems.is_empty(), "{:?}", sp.problems);
    }
}

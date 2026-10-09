//! Skate 3's world collision sounds: the impact events (`0x82486EF0`) the
//! skater's producers raise (`super::body_impacts`), the pool of ten
//! requests that plays them (`0x824F1818`) and the collision component
//! each request owns (vtable slots 7 to 10: `0x824D1DB8` activate,
//! `0x824D1CE0` finish, `0x824D1E00` mix inputs, `0x824D2318` update), one
//! mix-map instance of type 3 each (`0x4003_0000 | i << 11`).
//!
//! An event has two sides, a surface each (`+0`/`+4`, 143 none), a
//! strength type (`+8`/`+12`, 3 none) and a level (`+32`/`+36`). Each side
//! with a sound plays a splice (`super::impacts::choose`) into a submix at
//! the surface class's mix output; its gain is the event level times the
//! class's other output times the material's gain, its pitch the
//! material's. The request ends when both sides have finished.
//!
//! Host differences (documented, not approximated): the event position
//! (`+16`) is where GTA places the output: the producer's position source
//! becomes the emitter its splices play on (`Event::emitter`: the skater's
//! `+48` the body emitter, the board's `+144` the board emitter); the submix's
//! routing (`0x82497BF8` / `0x82491108`, reverb and environment buses) is
//! the host's. The requests are kept in creation order.

use super::evaluator::Evaluator;
use super::impacts::{self, NONE};
use super::splices::Splices;
use super::tuning::Tuning;
use crate::ops::fctiwz;
use crate::splice::Params;

pub const REQUESTS: usize = 10;
/// Mix-map type of a collision component.
pub const MIX_TYPE: usize = 3;
const LEVEL: f32 = 1.0 / 32767.0;
const PITCH: f32 = 1.0 / 4096.0;
const PAN: f32 = 360.0 / 65536.0;

/// A world collision event (the 48-byte `0x82486EF0` record).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    pub surfaces: [i32; 2],
    pub types: [i32; 2],
    pub levels: [i32; 2],
    /// `+40`: the local player's, outside the cutscene camera; selects a
    /// material's alternate pitch.
    pub alternate: bool,
    /// Where the event is placed (`+16`): the producer's position source as
    /// SkateV's emitter (`super::EMITTER_BOARD` / `super::EMITTER_BODY`).
    pub emitter: usize,
}

#[derive(Clone, Copy, Debug)]
struct Side {
    /// `+40 + 32k`.
    handle: u32,
    /// `+44`/`+48`: the material's gain and pitch (constructor 0 / 4096).
    gain: i32,
    pitch: i32,
}

impl Default for Side {
    fn default() -> Self {
        Side { handle: 0, gain: 0, pitch: 4096 }
    }
}

#[derive(Clone, Debug, Default)]
struct Request {
    /// `+52`.
    active: bool,
    /// The activation stamp the pool reuses by (`+64`).
    stamp: u32,
    /// Component `+36`.
    event: Option<Event>,
    sides: [Side; 2],
    /// `+72 + 4k`: each side's splice slot (−1 none).
    slots: [i32; 2],
}

#[derive(Clone, Debug, Default)]
pub struct Collisions {
    requests: [Request; REQUESTS],
    clock: u32,
}

pub fn mix_id(index: usize) -> u32 {
    0x4003_0000 | (index as u32) << 11
}

impl Collisions {
    #[cfg(test)]
    pub fn active(&self) -> usize {
        self.requests.iter().filter(|r| r.active).count()
    }

    #[cfg(test)]
    pub fn events(&self) -> impl Iterator<Item = &Event> {
        self.requests.iter().filter_map(|r| r.event.as_ref())
    }

    /// `0x82486EF0` / `0x824F1818`: the first idle request, else the
    /// oldest, finished first; then activated with `ev` (`0x828DF6F0`,
    /// component slot 7: the mix instance becomes active).
    pub fn post(&mut self, ev: Event, sp: &mut Splices) {
        let index = match self.requests.iter().position(|r| !r.active) {
            Some(i) => i,
            None => {
                let mut best = u32::MAX;
                let mut index = 0;
                for (i, r) in self.requests.iter().enumerate() {
                    if r.stamp < best {
                        best = r.stamp;
                        index = i;
                    }
                }
                self.finish(index, sp);
                index
            }
        };
        self.clock = self.clock.wrapping_add(1);
        let r = &mut self.requests[index];
        r.stamp = self.clock;
        r.active = true;
        r.event = Some(ev);
    }

    /// `0x824F8B58` → component slot 8 (`0x824D1CE0`): both sides released,
    /// the mix instance's inputs cleared and inactive, the event freed.
    fn finish(&mut self, index: usize, sp: &mut Splices) {
        let r = &mut self.requests[index];
        for side in &mut r.sides {
            if side.handle != 0 {
                sp.destroy(side.handle);
                side.handle = 0;
            }
        }
        r.event = None;
        r.active = false;
        r.stamp = 0;
    }

    /// Component slot 9 (`0x824D1E00`), before the mix map runs: input 0
    /// while a side plays, input 1 from the stronger side's type.
    pub fn inputs(&self, e: &mut Evaluator) {
        for (i, r) in self.requests.iter().enumerate() {
            let mut values = [0; 16];
            if let (true, Some(ev)) = (r.active, r.event.as_ref())
                && r.sides.iter().any(|s| s.handle != 0)
            {
                values[0] = 32767;
                let [a, b] = ev.types;
                let ty = if a == 3 { b } else if b == 3 || a > b { a } else { b };
                values[1] = match ty {
                    1 => 20000,
                    2 => 32767,
                    _ => 10000,
                };
            }
            e.set_active(mix_id(i), r.active);
            e.set_inputs(mix_id(i), values);
        }
    }

    /// Component slot 10 (`0x824D2318`), after the mix map: starts an
    /// activated request's sides, else updates them and finishes the
    /// request once neither plays.
    pub fn update(&mut self, e: &Evaluator, t: &Tuning, sp: &mut Splices, dt: f32) {
        for i in 0..REQUESTS {
            let r = &self.requests[i];
            let (true, Some(ev)) = (r.active, r.event) else { continue };
            if r.sides.iter().all(|s| s.handle == 0) {
                self.start(i, &ev, t, sp, dt);
                continue;
            }
            let p = e.params(mix_id(i));
            let classes = ev.surfaces.map(|s| class(t, s));
            let levels = classes.map(|c| p.u15(gain_output(c)));
            let pitches = classes.map(|c| p.pitch(if c == 9 { 22 } else { 1 }));
            let pan = p.u16(0) as f32 * PAN;
            let r = &mut self.requests[i];
            for k in 0..2 {
                let side = &mut r.sides[k];
                if side.handle == 0 {
                    continue;
                }
                if !sp.is_playing(side.handle) {
                    sp.destroy(side.handle);
                    side.handle = 0;
                    continue;
                }
                let base = fctiwz(ev.levels[k] as f32 * LEVEL * levels[k] as f32);
                let gain = fctiwz(base as f32 * (side.gain as f32 * LEVEL)) as f32 * LEVEL;
                let pitch = fctiwz(pitches[k] as f32 * (side.pitch as f32 * PITCH)) as f32 * PITCH;
                sp.update(side.handle, Params { gain, pitch, pan, dt, pan_scale: 0.0, stretch: 1.0 });
            }
            if r.sides.iter().all(|s| s.handle == 0) {
                self.finish(i, sp);
            }
        }
    }

    /// `0x824D1F68`: each side with a sound starts into its submix
    /// (`0x824D25E0`: aux send at the surface class's mix output, dry path
    /// to the bus at unity).
    fn start(&mut self, i: usize, ev: &Event, t: &Tuning, sp: &mut Splices, dt: f32) {
        let r = &mut self.requests[i];
        for k in 0..2 {
            r.slots[k] = -1;
            let surface = ev.surfaces[k];
            if surface == NONE || ev.types[k] == 3 {
                continue;
            }
            let choice = impacts::choose(t, surface, ev.surfaces[1 - k], ev.types[k], ev.alternate);
            let side = &mut r.sides[k];
            if let Some(gain) = choice.gain {
                side.gain = gain;
            }
            if let Some(pitch) = choice.pitch {
                side.pitch = pitch;
            }
            if choice.id == -1 {
                continue;
            }
            let h = sp.create(choice.slot as usize, choice.id);
            sp.set_emitter(h, ev.emitter);
            sp.play(h, 0, Params::at_play(dt));
            side.handle = h;
            r.slots[k] = choice.slot;
        }
    }
}

/// `0x824D20E8` / `0x824D22B8`: a surface's class (143 and beyond: 8).
fn class(t: &Tuning, surface: i32) -> i32 {
    if surface >= NONE { 8 } else { impacts::class(t, surface) }
}

/// `0x824D20E8`: the mix output scaling a side's gain, by class.
fn gain_output(class: i32) -> i32 {
    match class {
        0 => 13,
        1 => 14,
        2 => 15,
        3 => 16,
        4 => 17,
        5 => 18,
        6 => 12,
        7 => 19,
        9 => 21,
        _ => 20,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(level: i32) -> Event {
        Event { surfaces: [5, NONE], types: [1, 0], levels: [level, 0], alternate: false, emitter: super::super::EMITTER_BOARD }
    }

    #[test]
    fn pool_reuses_idle_then_oldest_request() {
        let mut c = Collisions::default();
        let mut sp = Splices::default();
        for n in 0..REQUESTS as i32 {
            c.post(event(n), &mut sp);
        }
        assert_eq!(c.active(), REQUESTS);
        c.post(event(100), &mut sp);
        // The oldest (level 0) was finished and reused.
        let levels: Vec<i32> = c.events().map(|e| e.levels[0]).collect();
        assert_eq!(levels[0], 100);
        assert!(!levels.contains(&0));
        c.finish(3, &mut sp);
        c.post(event(200), &mut sp);
        assert_eq!(c.requests[3].event.unwrap().levels[0], 200);
    }

    #[test]
    fn body_impact_plays_through_mix_and_banks() {
        use std::path::Path;
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/audio-cache");
        let t = match Tuning::load(&dir) { Ok(t) => t, Err(e) => { eprintln!("SKIP {e}"); return } };
        let mut e = Evaluator::load(&dir).unwrap();
        let mut sp = Splices::default();
        sp.load(&dir, super::super::splices::SLOT_COLLISIONS, "Skate_Collisions").unwrap();
        sp.load(&dir, super::super::splices::SLOT_METAL, "Skate_Metal").unwrap();
        // Every material row names a loaded bank or none (slot 2, hall of
        // meat, is not loaded natively outside that mode).
        let mut sounds = 0;
        for surface in 0..NONE {
            for ty in 0..3 {
                let c = impacts::choose(&t, surface, 97, ty, false);
                assert!(matches!(c.slot, -1..=2), "surface {surface}: slot {}", c.slot);
                if c.slot >= 0 && c.id > 0 {
                    sounds += 1;
                }
            }
        }
        assert!(sounds > 100, "{sounds}");
        let dt = 1.0 / 60.0;
        let mut c = Collisions::default();
        // The torso (97) hits concrete-like surface 0 hard.
        let (mut low, mut high) = (0.0, 0.0);
        let ty = impacts::strength(&t, 0, 0.9, &mut low, &mut high);
        let level = impacts::level(&t, 0, 97, ty, low, high, 0.9);
        c.post(Event { surfaces: [97, 0], types: [3, ty], levels: [0, level], alternate: false, emitter: super::super::EMITTER_BOARD }, &mut sp);
        c.inputs(&mut e);
        e.tick(dt).unwrap();
        c.update(&e, &t, &mut sp, dt);
        assert!(c.requests[0].sides[1].handle != 0, "no sound for level {level}, type {ty}");
        let mut frames = 0;
        while c.active() > 0 && frames < 600 {
            c.inputs(&mut e);
            assert_eq!(e.inputs(mix_id(0))[0], 32767);
            e.tick(dt).unwrap();
            c.update(&e, &t, &mut sp, dt);
            sp.service();
            frames += 1;
        }
        assert_eq!(c.active(), 0, "the request finishes when its sound ends");
        assert!(sp.problems.is_empty(), "{:?}", sp.problems);
        assert!(!sp.started.is_empty());
    }

    #[test]
    fn class_output_tables() {
        assert_eq!((0..11).map(gain_output).collect::<Vec<_>>(), [13, 14, 15, 16, 17, 18, 12, 19, 20, 21, 20]);
        assert_eq!(gain_output(-1), 20);
    }
}

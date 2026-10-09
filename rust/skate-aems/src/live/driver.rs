//! The game side of Skate 3's audio, live: sound components that own AEMS
//! objects and run the ported class handlers (`crate::glue`) on the
//! latest skater frame, as Skate's sound components do on the game thread
//! between AEMS control ticks.
//!
//! Each component keeps a byte image of the fields its handlers read and
//! write (`c8`/`c32` at the game's offsets) and the skater state image they
//! read through `*(this + 36)` / `*(this + 32)` (`super::frame::SkaterImage`),
//! so the handlers run unchanged.

use std::collections::HashMap;

use super::frame::{Frame, SkaterImage};
use super::tuning::Tuning;
use crate::glue::env::{Env, Send};
use crate::glue::{Params, Wrappers};
use crate::mem::Kind;
use crate::live::engine::Voices;
use crate::world::World;

pub type Handler = fn(&mut Env, &mut Wrappers) -> Vec<Send>;

/// One live sound component.
pub struct Component {
    pub name: &'static str,
    /// Player type-1 mix-map output bank; its inputs persist across updates.
    pub bank: u8,
    pub inputs: [i32; 16],
    /// Component bytes (big-endian, the game's offsets).
    pub image: Vec<u8>,
    pub params: Params,
    pub wrappers: Wrappers,
    /// AEMS object per wrapper slot.
    pub objects: HashMap<u32, u32>,
    /// Class reference cell per wrapper slot (for creates).
    pub classes: HashMap<u32, u32>,
    pub handlers: Vec<Handler>,
    /// Seam click words (word 7) not yet seen by a program tick:
    /// slot -> (value, `World::program_ticks` when sent).
    pub held: HashMap<u32, (i32, u64)>,
}

impl Component {
    pub fn new(name: &'static str, size: usize, handlers: Vec<Handler>) -> Component {
        Component {
            name,
            bank: 0,
            inputs: [0; 16],
            image: vec![0; size],
            params: Params::default(),
            wrappers: Wrappers::default(),
            objects: HashMap::new(),
            classes: HashMap::new(),
            handlers,
            held: HashMap::new(),
        }
    }
    pub fn c32(&self, off: usize) -> u32 {
        u32::from_be_bytes(self.image[off..off + 4].try_into().unwrap())
    }
    pub fn set32(&mut self, off: usize, v: u32) {
        self.image[off..off + 4].copy_from_slice(&v.to_be_bytes());
    }
    pub fn rf(&self, off: usize) -> f32 {
        f32::from_bits(self.c32(off))
    }
    pub fn setf(&mut self, off: usize, v: f32) {
        self.set32(off, v.to_bits());
    }
    pub fn set8(&mut self, off: usize, v: u8) {
        self.image[off] = v;
    }
}

/// Applies a handler's sends to the AEMS world.
pub fn apply(world: &mut World, vs: &mut Voices, comp: &mut Component, sends: Vec<Send>, problems: &mut Vec<String>) {
    for s in sends {
        if s.create {
            let Some(&class) = comp.classes.get(&s.slot) else {
                problems.push(format!("{}: no class for slot +{}", comp.name, s.slot));
                continue;
            };
            let words = write_words(world, &s.words);
            let out = world.mem.alloc_zeroed(Kind::Object, 4);
            world.create_object(class, words, out, vs);
            let obj = world.mem.r32(out);
            world.mem.free(out);
            world.mem.free(words);
            if obj != 0 {
                comp.objects.insert(s.slot, obj);
                comp.wrappers.map.insert(s.slot, s.words);
                comp.set32(s.slot as usize, 1);
            } else {
                comp.set32(s.slot as usize, 0);
                comp.wrappers.map.remove(&s.slot);
                problems.push(format!("{}: AEMS create failed at slot +{}", comp.name, s.slot));
            }
        } else if s.release {
            if let Some(obj) = comp.objects.remove(&s.slot) {
                world.release_object(obj, vs);
            }
            comp.wrappers.map.remove(&s.slot);
            comp.set32(s.slot as usize, 0);
        } else if let Some(&obj) = comp.objects.get(&s.slot) {
            let words = write_words(world, &s.words);
            world.update_object(obj, words, vs);
            world.mem.free(words);
        }
    }
}

/// Retail Skate 3 ran its game at 30 fps, the AEMS programs' rate, so a
/// seam click (word 7 set for one game frame, `glue/seams.rs::click`) was
/// always seen. SkateV publishes 60 Hz frames: a click is kept on its object
/// until a program tick has run after it, or about half the clicks are
/// cleared unseen and the rest play up to a program period late.
pub fn hold_clicks(comp: &mut Component, sends: &mut [Send], program_ticks: u64) {
    for s in sends.iter_mut().filter(|s| !s.create && !s.release && s.words.len() > 7) {
        if s.words[7] != 0 {
            let fresh = comp.held.get(&s.slot).is_none_or(|h| h.0 != s.words[7]);
            if fresh {
                comp.held.insert(s.slot, (s.words[7], program_ticks));
            }
        } else if let Some(&(value, at)) = comp.held.get(&s.slot) {
            if program_ticks == at {
                s.words[7] = value;
            } else {
                comp.held.remove(&s.slot);
            }
        }
    }
}

/// Creates the object of `slot` with `words` (a create wrapper).
pub fn create(world: &mut World, vs: &mut Voices, comp: &mut Component, slot: u32, words: &[i32], problems: &mut Vec<String>) {
    if comp.objects.contains_key(&slot) {
        return;
    }
    *comp.wrappers.words(slot, words.len()) = words.to_vec();
    comp.set32(slot as usize, 1);
    apply(world, vs, comp, vec![Send::create(slot, words)], problems);
}

/// Releases the object of `slot` (a release wrapper).
pub fn release(world: &mut World, vs: &mut Voices, comp: &mut Component, slot: u32) {
    let mut p = Vec::new();
    apply(world, vs, comp, vec![Send::release(slot)], &mut p);
}

/// Words in AEMS memory (at least 64, as the game's vectors are read in
/// place by listeners that copy their own count).
fn write_words(world: &mut World, words: &[i32]) -> u32 {
    let mut bytes = vec![0u8; 4 * words.len().max(64)];
    for (b, w) in bytes.chunks_exact_mut(4).zip(words) {
        b.copy_from_slice(&w.to_be_bytes());
    }
    world.mem.alloc(Kind::Object, bytes)
}

/// The live game side: components and their creation rules
/// (`super::components`).
pub struct Driver {
    pub skater: SkaterImage,
    pub tuning: Tuning,
    pub components: Vec<Component>,
    pub problems: Vec<String>,
    pub state: super::components::State,
    /// Set only after production configuration/evaluator initialization.
    pub configured: bool,
    pub evaluator: Option<super::evaluator::Evaluator>,
    pub collisions: super::collisions::Collisions,
    /// Frontend sound events (the combo multiplier).
    pub ui: super::ui_sounds::UiSounds,
}

impl Driver {
    /// Do not advertise a loaded bank collection as a working live producer.
    /// The offline voice harness can still use the engine directly.
    pub fn ensure_live_ready(&self) -> Result<(), String> {
        if !self.configured || self.components.is_empty() || !self.problems.is_empty() {
            return Err("live gameplay audio unavailable: component/evaluator integration is incomplete".into());
        }
        Ok(())
    }

    pub fn new(world: &mut World) -> Driver {
        let tuning = Tuning::default();
        let mut problems = Vec::new();
        let components = super::components::build(world, &mut problems);
        Driver {
            skater: SkaterImage::default(),
            tuning,
            components,
            problems,
            state: Default::default(),
            configured: false,
            evaluator: None,
            collisions: Default::default(),
            ui: Default::default(),
        }
    }

    pub fn set_frame(&mut self, frame: &Frame) {
        let previous_com_speed = self.skater.r32(212);
        self.skater = SkaterImage::from_frame(frame);
        self.skater.w32(216, previous_com_speed);
        super::components::impulse_speed_scale(&mut self.skater, &self.tuning);
        super::components::landing_class(&mut self.state, &mut self.skater, &self.tuning);
    }

    pub fn configure(&mut self,cache:&std::path::Path)->Result<(),String> {
        self.tuning=Tuning::load(cache)?;
        super::components::initialize(&mut self.components,&mut self.tuning)?;
        self.evaluator=Some(super::evaluator::Evaluator::load(cache)?);
        self.configured = self.problems.is_empty() && !self.components.is_empty();
        Ok(())
    }

    /// One game frame of the sound components.
    pub fn update(&mut self, world: &mut World, vs: &mut Voices, grains:&mut super::grains::Grains,
        splices:&mut super::splices::Splices,cache:&std::path::Path) {
        if let Err(e)=super::components::update(self, world, vs,grains,splices,cache)
            && !self.problems.contains(&e) {self.problems.push(e);}
        self.ui.update(&self.skater, &self.tuning, splices, 1. / 60.);
    }

    pub fn release_all(&mut self, world: &mut World, vs: &mut Voices) {
        for c in &mut self.components {
            let slots: Vec<u32> = c.objects.keys().copied().collect();
            for s in slots {
                release(world, vs, c, s);
            }
        }
        self.state = Default::default();
        // The engine has stopped every splice instance.
        self.collisions = Default::default();
        self.ui.reset();
        if let Some(e)=&mut self.evaluator {e.reset();}
        if let Err(e)=super::components::initialize(&mut self.components,&mut self.tuning)
            && !self.problems.contains(&e) {self.problems.push(e);}
    }

    /// Runs a component's handlers and applies their sends.
    pub fn run(&mut self, index: usize, world: &mut World, vs: &mut Voices) {
        let comp = &mut self.components[index];
        for i in 0..comp.handlers.len() {
            let h = comp.handlers[i];
            let params = comp.params;
            let mut env = Env { comp, skater: &self.skater, tuning: &self.tuning, params };
            let mut wrappers = std::mem::take(&mut env.comp.wrappers);
            let mut sends = h(&mut env, &mut wrappers);
            comp.wrappers = wrappers;
            if comp.name == "seams" {
                hold_clicks(comp, &mut sends, world.program_ticks);
            }
            apply(world, vs, comp, sends, &mut self.problems);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::frame::Field;

    #[test]
    fn publication_preserves_previous_com_speed() {
        let mut d = Driver::new(&mut World::default());
        d.set_frame(&Frame { fields: vec![(212, Field::F32(4.25))], ..Default::default() });
        d.set_frame(&Frame { fields: vec![(212, Field::F32(7.5))], ..Default::default() });
        assert_eq!(d.skater.rf(216), 4.25);
        assert_eq!(d.skater.rf(212), 7.5);
    }

    #[test]
    fn seam_clicks_stay_until_a_program_tick_sees_them() {
        let mut c = Component::new("seams", 192, vec![]);
        let send = |w7: i32| {
            let mut w = vec![0; 20];
            w[7] = w7;
            vec![Send::update(52, &w)]
        };
        let mut s = send(1);
        hold_clicks(&mut c, &mut s, 10);
        assert_eq!(s[0].words[7], 1);
        // Next 60 Hz frame clears word 7 before the 30 Hz programs ran: held.
        let mut s = send(0);
        hold_clicks(&mut c, &mut s, 10);
        assert_eq!(s[0].words[7], 1);
        // A program tick ran: released.
        let mut s = send(0);
        hold_clicks(&mut c, &mut s, 11);
        assert_eq!(s[0].words[7], 0);
        // A new click replaces a held one.
        let (mut a, mut b) = (send(2), send(1));
        hold_clicks(&mut c, &mut a, 12);
        hold_clicks(&mut c, &mut b, 12);
        assert_eq!(b[0].words[7], 1);
    }

    #[test]
    fn missing_live_producer_is_an_error_not_ready_silence() {
        let d = Driver::new(&mut World::default());
        assert!(d.ensure_live_ready().unwrap_err().contains("component/evaluator"));
    }
}

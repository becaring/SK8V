//! What the Hall of Meat and TRAX banner players share: the VM with the
//! natives both movies call, their `do_init_action` setup, the pending-action
//! drain and the frame step; and the edge-offset container the trick display
//! and the Hall of Meat move.
use crate::apt_movie::Movie;
use crate::apt_vm::{ObjectKind, Value, Vm};
use crate::hom::{Bindings, HomData};
use crate::layout::Layout;
use crate::player::{Assets, Frame};

/// Frame-action budget per drain, a guard against runaway scripts.
const MAX_ACTIONS: u32 = 4096;

pub(crate) struct MoviePlayer {
    pub(crate) vm: Vm,
    pub(crate) bindings: Bindings,
    /// Names the movie in errors.
    label: &'static str,
}

impl MoviePlayer {
    /// Loads the movie, runs its init actions and its first frame's actions.
    pub(crate) fn new(assets: &Assets, label: &'static str) -> Result<Self, String> {
        let mut vm = Vm::new();
        for name in ["MovieClip", "FELanguage", "HUDComponents", "Math", "Audio", "ScreenManager"] {
            let object = vm.object(ObjectKind::Native(name.into()));
            let prototype = vm.object(ObjectKind::Plain);
            vm.set(object, "prototype", Value::Object(prototype))?;
            vm.set(vm.global, name, Value::Object(object))?;
        }
        vm.set(vm.global, "Screen_EdgeOffset", Value::Number(0.0))?;
        let mut bindings = Bindings { movie: Movie::load(assets.source())?, data: HomData::default() };
        let initial: Vec<_> = bindings
            .movie
            .characters
            .values()
            .flat_map(|c| &c.frames)
            .flat_map(|f| &f.controls)
            .filter(|c| c.type_name == "do_init_action")
            .map(|c| c.actions_offset)
            .collect();
        for offset in initial {
            let code = bindings.movie.actions[&offset.to_string()].clone();
            vm.run(&code, &mut bindings)?;
        }
        vm.begin_update();
        bindings.movie.initialize(&mut vm)?;
        let mut player = Self { vm, bindings, label };
        player.drain()?;
        Ok(player)
    }

    /// Runs the actions the movie queued. The script never reads
    /// `movie.actions`, so the block is moved out while it runs.
    pub(crate) fn drain(&mut self) -> Result<(), String> {
        let mut calls = 0;
        while let Some((object, offset)) = self.bindings.movie.pending.pop_front() {
            calls += 1;
            if calls > MAX_ACTIONS {
                return Err(format!("{} frame action limit", self.label));
            }
            if !self.bindings.movie.instances.contains_key(&object) {
                continue;
            }
            let key = offset.to_string();
            let block = self
                .bindings
                .movie
                .actions
                .get_mut(&key)
                .ok_or_else(|| format!("Missing {} action block", self.label))?;
            let code = std::mem::take(block);
            let result = self.vm.run_on(object, &code, &mut self.bindings);
            self.bindings.movie.actions.insert(key, code);
            result?;
        }
        Ok(())
    }

    /// One timeline frame.
    pub(crate) fn advance(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        self.bindings.movie.advance(&mut self.vm)?;
        self.drain()?;
        self.vm.collect(self.bindings.movie.instances.keys().copied())?;
        Ok(())
    }

    /// The movie's own scene in `layout`'s pixels.
    pub(crate) fn native_frame(&self, assets: &Assets, layout: &Layout) -> Result<Frame, String> {
        let draws = crate::apt_scene::draw(&self.bindings.movie, &self.vm, assets.shapes())?;
        Ok(crate::player::flatten(draws, assets, layout))
    }

    pub(crate) fn visible_text(&self) -> Vec<String> {
        visible_text(&self.bindings.movie, &self.vm)
    }
}

pub(crate) fn visible_text(movie: &Movie, vm: &Vm) -> Vec<String> {
    let mut out = Vec::new();
    crate::player::collect_text(movie, vm, movie.root, &mut out);
    out
}

/// A movie's `mScreen` container and the `Screen_EdgeOffset` applied to it.
/// The movie's constructor added the runtime's 0 offset to the container's
/// x; later offsets move it from that constructed position.
pub(crate) struct EdgeScreen {
    screen: Option<usize>,
    x: f64,
    offset: f32,
}

impl EdgeScreen {
    pub(crate) fn new(vm: &Vm, screen: Option<usize>) -> Self {
        Self { screen, x: screen.map_or(0.0, |id| vm.get(id, "_x").number()), offset: 0.0 }
    }

    pub(crate) fn set(&mut self, vm: &mut Vm, offset: f32) -> Result<(), String> {
        let offset = if offset.is_finite() { offset } else { 0.0 };
        self.offset = offset;
        let global = vm.global;
        vm.set(global, "Screen_EdgeOffset", Value::Number(offset as f64))?;
        if let Some(id) = self.screen {
            vm.set(id, "_x", Value::Number(self.x + offset as f64))?;
        }
        Ok(())
    }

    pub(crate) fn offset(&self) -> f32 {
        self.offset
    }
}

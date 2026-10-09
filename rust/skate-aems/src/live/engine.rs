//! The live engine: AEMS world + sound components + dry voices.
//!
//! `set_frame` runs the game-side components once per submitted Skate frame,
//! so a frame's edge-triggered lifecycle work is never repeated by rendering.
//! Each 256-sample block (`render`) then runs `World::tick` (`0x82B1E290`,
//! the AEMS control cadence), and every voice, grain player and splice layer
//! renders one block into its emitter.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::cache::{self, BankFile, Cache};
use super::driver::Driver;
use super::frame::Frame;
use super::seam_voices::SeamVoices;
use super::voices::DryVoice;
use super::{EMITTERS, EMITTER_BODY, EMITTER_BOARD, EMITTER_SPEED};
use crate::eac::BLOCK;
use crate::mem::Memory;
use crate::world::World;

/// A hasher with fixed keys: the voices sum in the same order every run, so
/// the output is reproducible bit for bit.
pub type Det = std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher>;

/// What one block produced per emitter.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockInfo {
    pub voices: u32,
}

/// Classes whose objects the live sound components create (their banks
/// load at start, plus every bank their patches' child classes need).
pub const DRIVEN_CLASSES: &[&str] = &[
    "Class_rolling",
    "Rolling_Rattle_Class",
    "Class_wheels_skid",
    "Class_Squeaks",
    "c_board_slide",
    "SenseOfSpeed_wind",
    "SenseOfSpeed_rattle",
    "Class_foot_drag",
    "Class_Seams",
    "Class_Flips",
    "c_cloth_falls",
    "cloth_trick",
    "c_body_slide",
    "playercharacter_footstep",
    "Class_grind",
];

/// The emitter a class's voices play on.
pub fn emitter_of(class: &str) -> usize {
    let c = class.to_ascii_lowercase();
    if c.starts_with("senseofspeed") {
        EMITTER_SPEED
    } else if c.contains("foot") || c.contains("cloth") || c.contains("body") || c.contains("fstep") {
        EMITTER_BODY
    } else {
        EMITTER_BOARD
    }
}

/// EA Audio Core for AEMS: dry voices keyed by handle.
pub struct Voices {
    pub cache: Cache,
    /// Bank base address → loaded bank file.
    pub banks: HashMap<u32, Arc<BankFile>>,
    pub live: HashMap<u32, DryVoice, Det>,
    next: u32,
    /// Record address by patch code address (loaded banks).
    pub records: HashMap<u32, u32>,
    /// Voice starts (bank stem, table index, emitter); kept only when a
    /// diagnostic sets this to `Some`.
    pub trace: Option<Vec<(String, usize, usize)>>,
    /// The AEMS class names a voice's `class` indexes (diagnostics:
    /// per-class levels).
    classes: Vec<String>,
}

/// Output of one sound class since the meters were last taken.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClassMeter {
    pub energy: f64,
    pub frames: u64,
    pub voices: u32,
}

impl Voices {
    /// The index of class `name` in the class table (added on first use).
    pub fn class_index(&mut self, name: &str) -> usize {
        if let Some(i) = self.classes.iter().position(|c| c == name) {
            return i;
        }
        self.classes.push(name.to_string());
        self.classes.len() - 1
    }

    /// The class of a live voice (`"?"` for a voice without one).
    pub fn class_of(&self, v: &DryVoice) -> &str {
        self.classes.get(v.class).map_or("?", String::as_str)
    }

    /// Records a problem once; `Engine::problems` reports it.
    pub fn problem(&mut self, p: String) {
        if !self.cache.problems.contains(&p) {
            self.cache.problems.push(p);
        }
    }

    pub fn note_start(&mut self, stem: &str, index: usize, emitter: usize) {
        if let Some(t) = &mut self.trace {
            t.push((stem.to_string(), index, emitter));
        }
    }

    /// Adds a voice started outside the programs (direct paths).
    pub fn insert(&mut self, mut v: DryVoice, class: &str) -> u32 {
        v.class = self.class_index(class);
        self.next = self.next.wrapping_add(1).max(1);
        self.live.insert(self.next, v);
        self.next
    }

    /// The emitter and class of the object whose patch instance holds
    /// `player`: instance (its allocation) ? record (by code pointer `+16`)
    /// ? header `inst + rec[+52]` ? object `+8` ? class ? name.
    fn route(&self, mem: &Memory, player: u32) -> (usize, String) {
        let Some(inst) = mem.region_base(player) else { return (EMITTER_BOARD, String::new()) };
        let found = (|| {
            let rec = *self.records.get(&mem.r32(inst + 16))?;
            let hdr = inst.wrapping_add(mem.r32(rec + 52));
            let obj = mem.r32(hdr + 8);
            let class = mem.r32(obj);
            let mut name = Vec::new();
            let mut at = mem.r32(class + 4);
            while mem.valid(at, 1) && mem.r8(at) != 0 && name.len() < 64 {
                name.push(mem.r8(at));
                at += 1;
            }
            let class = String::from_utf8_lossy(&name).into_owned();
            Some((emitter_of(&class), class))
        })();
        found.unwrap_or((EMITTER_BOARD, String::new()))
    }

    /// The factory (slot 0 of `*0x82FD35F8`) for the sound player at `player`
    /// playing its table entry `entry`; 0 when no voice was made.
    pub fn create(&mut self, mem: &Memory, player: u32, entry: u32) -> u32 {
        let bank = mem.r32(player);
        let Some(file) = self.banks.get(&bank).cloned() else {
            return 0;
        };
        let index = mem.r16(entry) as i16;
        if index < 0 {
            return 0;
        }
        let Some(sound) = self.cache.sound(&file, index as usize) else {
            return 0;
        };
        let (emitter, class) = self.route(mem, player);
        let channels = sound.snr.channels as usize;
        let mut v = DryVoice::new(sound, channels, emitter, 0.0);
        v.class = if class.is_empty() { self.class_index(&format!("({})", file.stem)) } else { self.class_index(&class) };
        self.next = self.next.wrapping_add(1).max(1);
        self.live.insert(self.next, v);
        self.note_start(&file.stem, index as usize, emitter);
        self.next
    }

    /// Voice slot 0 (release).
    pub fn release(&mut self, voice: u32) {
        if let Some(v) = self.live.get_mut(&voice) {
            v.release();
        }
    }

    /// Voice slot 3: attribute set.
    pub fn set(&mut self, voice: u32, id: u32, value: u32) {
        if let Some(v) = self.live.get_mut(&voice) {
            v.set(id, value);
        }
    }

    /// Voice slot 5's "alive".
    pub fn alive(&self, voice: u32) -> bool {
        self.live.get(&voice).is_some_and(DryVoice::alive)
    }
}

pub struct Engine {
    pub grains: super::grains::Grains,
    pub splices: super::splices::Splices,
    pub world: World,
    pub voices: Voices,
    pub driver: Driver,
    /// System time in seconds (blocks × 256 / 48000).
    pub time: f64,
    pub seam: SeamVoices,
    active: bool,
    loaded: Vec<String>,
    cache_dir: std::path::PathBuf,
    /// Per-class output since `take_meters` (diagnostics), by class index.
    meters: Vec<ClassMeter>,
    /// This block's sum and voice count per class index.
    block_sums: Vec<[f32; BLOCK]>,
    block_voices: Vec<u32>,
    /// Class indexes of the grain and splice meters.
    grain_class: usize,
    splice_classes: [usize; EMITTERS],
}

impl Engine {
    /// Opens the prepared cache at `dir`. The second argument is ignored; it
    /// is kept so existing callers compile.
    pub fn open(dir: &Path, _unused: Option<&Path>) -> Result<Engine, String> {
        let mut voices = Voices {
            cache: Cache::open(dir)?,
            banks: HashMap::new(),
            live: HashMap::default(),
            next: 0,
            records: HashMap::new(),
            trace: None,
            classes: Vec::new(),
        };
        let mut world = World::default();
        for (name, bytes) in &voices.cache.csi {
            world.load_csi(bytes).map_err(|e| format!("{name}: {e}"))?;
        }
        // Banks: those subscribing to a driven class, closed over the
        // classes their patches import (child patches).
        let mut infos: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
        let mut keys: Vec<&String> = voices.cache.bank_paths.keys().collect();
        keys.sort();
        for key in keys {
            if !key.starts_with("audiofiles/") {
                continue;
            }
            let bytes = std::fs::read(&voices.cache.bank_paths[key]).map_err(|e| e.to_string())?;
            let (records, imported) = cache::bank_classes(&bytes);
            infos.push((key.clone(), records, imported));
        }
        let mut want: Vec<String> = DRIVEN_CLASSES.iter().map(|s| s.to_string()).collect();
        let mut chosen: Vec<String> = Vec::new();
        loop {
            let mut grew = false;
            for (key, records, imported) in &infos {
                if chosen.contains(key) || !records.iter().any(|r| want.contains(r)) {
                    continue;
                }
                chosen.push(key.clone());
                grew = true;
                for c in imported {
                    if !want.contains(c) {
                        want.push(c.clone());
                    }
                }
            }
            if !grew {
                break;
            }
        }
        chosen.sort();
        for key in &chosen {
            let file = Arc::new(voices.cache.bank(key)?);
            let base = world.load_bank(&file.bytes).map_err(|e| format!("{key}: {e}"))?;
            let m = &world.mem;
            let mut rec = base + m.r32(base + 28);
            for _ in 0..m.r16(base + 10) {
                voices.records.insert(m.r32(rec + 40), rec);
                rec += 60 + 4 * (m.r8(rec + 36) as u32 + m.r8(rec + 39) as u32);
            }
            voices.cache.preload(&file);
            voices.banks.insert(base, file);
        }
        let mut driver = Driver::new(&mut world);
        driver.configure(dir)?;
        // The board foley component's splice bank (slot 0).
        let mut splices = super::splices::Splices::default();
        splices.load(dir, super::splices::SLOT_COLLISIONS, "Skate_Collisions")?;
        splices.load(dir, super::splices::SLOT_FOLEY, "sk8_foley")?;
        splices.load(dir, super::splices::SLOT_METAL, "Skate_Metal")?;
        splices.load(dir, super::ui_sounds::SLOT_MENU, "sk8_menu")?;
        let mut seam = SeamVoices::default();
        seam.load(&mut voices);
        let grain_class = voices.class_index("rolling grains");
        let splice_classes = [
            voices.class_index("board splices"),
            voices.class_index("body splices"),
            voices.class_index("frontend splices"),
        ];
        Ok(Engine {
            grains: Default::default(),
            splices,
            world,
            voices,
            driver,
            time: 0.0,
            seam,
            active: false,
            loaded: chosen,
            cache_dir: dir.to_owned(),
            meters: Vec::new(),
            block_sums: Vec::new(),
            block_voices: Vec::new(),
            grain_class,
            splice_classes,
        })
    }

    pub fn summary(&self) -> String {
        format!("{} banks for {} classes ({})", self.loaded.len(), DRIVEN_CLASSES.len(), self.loaded.join(", "))
    }

    /// Problems so far: the cache's and voices', the driver's, then the
    /// splices'. The lists only grow, so a larger `problem_count` since the
    /// last look means there is something new to log.
    pub fn problems(&self) -> Vec<String> {
        let mut p = self.voices.cache.problems.clone();
        p.extend(self.driver.problems.iter().cloned());
        p.extend(self.splices.problems.iter().cloned());
        p
    }

    /// `problems().len()` without copying the strings.
    pub fn problem_count(&self) -> usize {
        self.voices.cache.problems.len() + self.driver.problems.len() + self.splices.problems.len()
    }

    /// Latest game state (call once per Skate tick).
    pub fn set_frame(&mut self, frame: &Frame) {
        self.driver.set_frame(frame);
        if self.active {
            self.driver.update(&mut self.world, &mut self.voices, &mut self.grains, &mut self.splices, &self.cache_dir);
            self.grains.set_ground(frame.roughness, frame.loose_ground);
            for e in &frame.seam_events {
                self.seam.play(&mut self.voices, self.time + e.at.max(0.0) as f64, e);
            }
        }
    }

    /// Skate mode on/off; going inactive releases every component's objects.
    pub fn set_active(&mut self, active: bool) {
        if self.active && !active {
            self.grains.stop_all();
            self.splices.stop_all();
            self.driver.release_all(&mut self.world, &mut self.voices);
        }
        self.active = active;
    }

    /// Per-class output since the last call: (class, RMS dBFS while it
    /// sounded, share of the window it sounded, most voices at once).
    pub fn take_meters(&mut self, window_frames: u64) -> Vec<(String, f32, f32, u32)> {
        let mut out: Vec<(String, f32, f32, u32)> = std::mem::take(&mut self.meters)
            .into_iter()
            .enumerate()
            .filter(|(_, m)| m.frames > 0)
            .map(|(class, m)| {
                let rms = if m.energy > 0.0 { (10.0 * (m.energy / m.frames as f64).log10()) as f32 } else { -120.0 };
                (self.voices.classes[class].clone(), rms, m.frames as f32 / window_frames.max(1) as f32, m.voices)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// No voice alive and nothing driven.
    pub fn idle(&self) -> bool {
        !self.active && self.voices.live.is_empty()
    }

    /// One 256-sample block per emitter.
    pub fn render(&mut self, out: &mut [[f32; BLOCK]; EMITTERS]) -> [BlockInfo; EMITTERS] {
        let dt = BLOCK as f32 / 48_000.0;
        self.world.tick(dt, &mut self.voices);
        let mut info = [BlockInfo::default(); EMITTERS];
        for o in out.iter_mut() {
            o.fill(0.0);
        }
        let now = self.time;
        // Released graphs were destroyed at the command drain (no render).
        self.voices.live.retain(|_, v| !v.released());
        let classes = self.voices.classes.len();
        self.block_sums.resize(classes, [0.0; BLOCK]);
        self.block_voices.clear();
        self.block_voices.resize(classes, 0);
        let (sums, counts) = (&mut self.block_sums, &mut self.block_voices);
        // This block's output per class (summed over its voices), metered once.
        let mut meter = |class: usize, block: &[f32; BLOCK], voices: u32| {
            if counts[class] == 0 {
                sums[class] = [0.0; BLOCK];
            }
            for (a, b) in sums[class].iter_mut().zip(block.iter()) {
                *a += *b;
            }
            counts[class] += voices;
        };
        for v in self.voices.live.values_mut() {
            if v.alive() {
                let mut block = [0.0f32; BLOCK];
                v.render(now, &mut block);
                for (o, s) in out[v.emitter].iter_mut().zip(block.iter()) {
                    *o += *s;
                }
                info[v.emitter].voices += 1;
                meter(v.class, &block, 1);
            }
        }
        let mut grains = [0.0f32; BLOCK];
        let grain_voices = self.grains.render(&mut grains);
        for (o, s) in out[EMITTER_BOARD].iter_mut().zip(grains.iter()) {
            *o += *s;
        }
        if grain_voices > 0 {
            meter(self.grain_class, &grains, grain_voices);
        }
        info[EMITTER_BOARD].voices += grain_voices;
        let mut splice = [[0.0f32; BLOCK]; EMITTERS];
        let splice_voices = self.splices.render_emitters(now, &mut splice);
        for e in [EMITTER_BOARD, EMITTER_BODY, EMITTER_SPEED] {
            for (o, s) in out[e].iter_mut().zip(splice[e].iter()) {
                *o += *s;
            }
            if splice_voices[e] > 0 {
                meter(self.splice_classes[e], &splice[e], splice_voices[e]);
            }
            info[e].voices += splice_voices[e];
        }
        self.meters.resize(classes, ClassMeter::default());
        for (class, &voices) in self.block_voices.iter().enumerate() {
            if voices > 0 {
                let m = &mut self.meters[class];
                m.energy += self.block_sums[class].iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>();
                m.frames += BLOCK as u64;
                m.voices = m.voices.max(voices);
            }
        }
        self.time += dt as f64;
        info
    }
}

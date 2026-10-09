//! Skate 3's splice sound path, live: the banks the board foley component
//! plays through (`crate::splice`), each started layer rendered through the
//! dry voice chain (`DryVoice`) like an AEMS voice.
//!
//! A layer's graph is SndPlayer1 → Resample → [TimeStretch] → Gain →
//! [Pan2D1] → Send (`0x82976360`). The dry hand-off keeps Resample and Gain;
//! Pan2D1 and the Send (always 1.0) are placement and routing, which the host
//! does per emitter. An instance that plays into its own submix
//! (`0x82488DD0`/`0x82498248`/`0x824D25E0`) goes Sub0 → Sen0 (the aux bus
//! `*(mixer + 52)`, at the level the caller passes and `0x82498140`
//! updates) and Sen0 (its bus at the default level): the dry path is unity
//! and the level is the aux send, which the host's own reverb replaces and
//! this layer does not keep. TimeStretch is not ported: a layer that needs a
//! ratio other than 1 is reported once.
//!
//! The footsteps component's slots each own a filter submix (`chain`,
//! `0x82494188`): Sub0 → HighPassIir2 → LowPassIir2 → PeakingIir2 → Sen0
//! (aux tap, routing) → Pn21 (placement) → Sen0 (SFX master at its default
//! level). Instances created into a chain are summed and filtered there.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::voices::DryVoice;
use super::{EMITTER_BOARD, EMITTERS};
use crate::eac::grain::MIX_RATE;
use crate::eac::iir2::{Iir2, Peaking};
use crate::eac::xma::Sound;
use crate::eac::BLOCK;
use crate::splice::{self, Bank, BankState, Cmd, CrtRand, Decision, Entry, Instance, Manager, Params};

/// The game's splice bank slots (`0x828DC660` load order, retail trace:
/// 0 Skate_Collisions, 1 Skate_Metal, 3 Sk82_Whsh_Bys, 5 sk8_menu,
/// 6 CellPhone_Rings, 7 sk8_foley).
pub const SLOT_COLLISIONS: usize = 0;
pub const SLOT_METAL: usize = 1;
pub const SLOT_FOLEY: usize = 7;
pub const SLOTS: usize = 9;

struct Loaded {
    bank: Bank,
    stem: String,
    sounds: HashMap<usize, Option<Arc<Sound>>>,
}

struct Playing {
    inst: Instance,
    voices: Vec<Option<DryVoice>>,
    /// The filter submix it plays into (0: none).
    chain: u32,
    /// The emitter its voices play on (GTA's placement).
    emitter: usize,
}

/// One footstep slot submix (mono, like the dry hand-off).
struct Chain {
    highpass: Iir2,
    lowpass: Iir2,
    peak: Peaking,
    mix: [f32; BLOCK],
}

/// Every splice instance the game side owns, by handle (0 = none).
pub struct Splices {
    banks: Vec<Option<Loaded>>,
    states: Vec<Option<BankState>>,
    pcm: std::path::PathBuf,
    playing: HashMap<u32, Playing, super::engine::Det>,
    chains: Vec<Chain>,
    manager: Manager<(u32, usize)>,
    /// Independent host session seed: retail shares the C runtime's
    /// `rand()` with the rest of the game thread, so its draw sequence is
    /// not portable.
    rand: CrtRand,
    next: u32,
    pub problems: Vec<String>,
    /// Layer starts (bank, sample), for tests.
    #[cfg(test)]
    pub started: Vec<(String, u16)>,
}

impl Default for Splices {
    fn default() -> Self {
        Splices {
            banks: (0..SLOTS).map(|_| None).collect(),
            states: (0..SLOTS).map(|_| None).collect(),
            pcm: Default::default(),
            playing: Default::default(),
            chains: Vec::new(),
            manager: Manager::default(),
            rand: CrtRand { seed: 1 },
            next: 0,
            problems: Vec::new(),
            #[cfg(test)]
            started: Vec::new(),
        }
    }
}

impl Splices {
    /// Loads `raw/audiofiles/<stem>.bnk` into `slot`; samples decode from
    /// `pcm/audiofiles/<stem>_<index>.xma16` on first use.
    pub fn load(&mut self, cache: &Path, slot: usize, stem: &str) -> Result<(), String> {
        let path = cache.join("raw/audiofiles").join(format!("{stem}.bnk"));
        let raw = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let bank = Bank::parse(raw).map_err(|e| format!("{}: {e}", path.display()))?;
        self.states[slot] = Some(bank.states());
        self.banks[slot] = Some(Loaded { bank, stem: stem.into(), sounds: HashMap::new() });
        self.pcm = cache.join("pcm/audiofiles");
        Ok(())
    }

    fn problem(&mut self, p: String) {
        if !self.problems.contains(&p) {
            self.problems.push(p);
        }
    }

    /// `0x82975700`: an instance of `id` in `slot` (0 when the bank is not
    /// loaded or has no sounds).
    pub fn create(&mut self, slot: usize, id: i32) -> u32 {
        self.create_into(slot, id, 0)
    }

    /// `0x82975700` with a filter submix (`chain`) as the route.
    pub fn create_into(&mut self, slot: usize, id: i32, chain: u32) -> u32 {
        let (Some(Some(l)), Some(Some(st))) = (self.banks.get(slot), self.states.get_mut(slot)) else { return 0 };
        let rand = &mut self.rand;
        let Some(inst) = splice::create(&l.bank, st, slot, id, &mut || rand.next()) else { return 0 };
        self.next = self.next.wrapping_add(1).max(1);
        let n = inst.layers.len();
        self.playing.insert(self.next, Playing { inst, voices: (0..n).map(|_| None).collect(), chain, emitter: EMITTER_BOARD });
        self.next
    }

    /// `0x82494188`: a new filter submix, at the footstep slot defaults
    /// (`0x824D7CE8`: high-pass 0, low-pass 96000, peak 96000 Hz, gain 1,
    /// Q 3), all passing through.
    pub fn chain(&mut self) -> u32 {
        self.chains.push(Chain {
            highpass: Iir2::new(true, 1),
            lowpass: Iir2::new(false, 1),
            peak: Peaking::new(1, 96000.0, 1.0, 3.0),
            mix: [0.0; BLOCK],
        });
        self.chains.len() as u32
    }

    /// `0x82494550`'s filter attributes: high-pass and low-pass cutoffs,
    /// then the peak's frequency, gain and Q.
    pub fn set_chain(&mut self, chain: u32, p: [f32; 5]) {
        let Some(c) = (chain as usize).checked_sub(1).and_then(|i| self.chains.get_mut(i)) else { return };
        c.highpass.cutoff = p[0];
        c.lowpass.cutoff = p[1];
        c.peak.frequency = p[2];
        c.peak.gain = p[3];
        c.peak.q = p[4];
    }

    /// Plays an instance (`0x82975A60`).
    pub fn play(&mut self, h: u32, flag: u8, p: Params) {
        let Some(pl) = self.playing.get_mut(&h) else { return };
        let bank = &self.banks[pl.inst.slot].as_ref().unwrap().bank;
        let rand = &mut self.rand;
        let cmds = pl.inst.play(bank, flag, p, &mut || rand.next());
        self.apply(h, cmds);
    }

    /// Updates an instance with new parameters (`0x82975B08`).
    pub fn update(&mut self, h: u32, p: Params) {
        let Some(pl) = self.playing.get_mut(&h) else { return };
        let bank = &self.banks[pl.inst.slot].as_ref().unwrap().bank;
        let voices = &pl.voices;
        let cmds = pl.inst.update(bank, p, &mut |i| voices[i].as_ref().is_some_and(DryVoice::finished));
        self.apply(h, cmds);
        if let Some(pl) = self.playing.get_mut(&h) {
            for (i, l) in pl.inst.layers.iter().enumerate() {
                if l.is_none() {
                    pl.voices[i] = None;
                }
            }
        }
    }

    /// Places an instance's voices on `emitter` (an event's position).
    pub fn set_emitter(&mut self, h: u32, emitter: usize) {
        if let Some(pl) = self.playing.get_mut(&h)
            && emitter < EMITTERS
        {
            pl.emitter = emitter;
        }
    }

    /// Whether any layer still plays (`0x82975BF0`).
    pub fn is_playing(&self, h: u32) -> bool {
        self.playing.get(&h).is_some_and(|p| p.inst.is_playing())
    }

    /// `0x824836B8`: every layer's graph is released and leaves the
    /// manager; the instance is freed.
    pub fn destroy(&mut self, h: u32) {
        if let Some(pl) = self.playing.remove(&h) {
            for i in 0..pl.voices.len() {
                self.manager.remove((h, i));
            }
        }
    }

    /// Every instance and filter submix goes (the components that own
    /// them are reinitialised).
    pub fn stop_all(&mut self) {
        self.playing.clear();
        self.chains.clear();
        self.manager = Manager::default();
    }

    /// `0x82975290`: the once-per-frame voice manager service.
    pub fn service(&mut self) {
        for d in self.manager.service() {
            match d {
                Decision::Start((h, i), p) => {
                    let Some(pl) = self.playing.get_mut(&h) else { continue };
                    let bank = &self.banks[pl.inst.slot].as_ref().unwrap().bank;
                    let cmds = pl.inst.start(bank, i, p);
                    self.apply(h, cmds);
                }
                Decision::Steal((h, i)) | Decision::Drop((h, i)) => {
                    if let Some(pl) = self.playing.get_mut(&h) {
                        pl.inst.drop_layer(i);
                        pl.voices[i] = None;
                    }
                }
            }
        }
    }

    fn sound(&mut self, slot: usize, sample: u16) -> Option<Arc<Sound>> {
        let l = self.banks[slot].as_mut()?;
        let i = sample as usize;
        if let Some(s) = l.sounds.get(&i) {
            return s.clone();
        }
        let pcm = &self.pcm;
        let result = (|| -> Result<Sound, String> {
            let e = l.bank.samples.get(i).ok_or("no such sample")?;
            let end = l.bank.samples.get(i + 1).map_or(l.bank.raw.len() - l.bank.data_at, |n| n.snr as usize);
            let bytes = l.bank.raw[l.bank.data_at + e.snr as usize..l.bank.data_at + end].to_vec();
            let p = pcm.join(format!("{}_{i}.xma16", l.stem));
            let data = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            Sound::new(bytes, &data)
        })();
        let (s, err) = match result {
            Ok(s) => (Some(Arc::new(s)), None),
            Err(e) => (None, Some(format!("splice {} sample {i}: {e}", l.stem))),
        };
        l.sounds.insert(i, s.clone());
        if let Some(e) = err {
            self.problem(e);
        }
        s
    }

    fn apply(&mut self, h: u32, cmds: Vec<Cmd>) {
        for c in cmds {
            let Some(slot) = self.playing.get(&h).map(|p| p.inst.slot) else { return };
            match c {
                Cmd::Queue { layer, priority, params } => {
                    if !self.manager.enqueue(Entry { handle: (h, layer), priority, params }) {
                        self.playing.get_mut(&h).unwrap().inst.drop_layer(layer);
                    }
                }
                Cmd::Play { layer, sample, offset } => {
                    if offset.is_some_and(|o| o != 0.0) {
                        self.problem(format!("splice slot {slot} sample {sample}: start offsets are not ported"));
                    }
                    let sound = self.sound(slot, sample);
                    #[cfg(test)]
                    self.started.push((self.banks[slot].as_ref().unwrap().stem.clone(), sample));
                    let pl = self.playing.get_mut(&h).unwrap();
                    let emitter = pl.emitter;
                    pl.voices[layer] = sound.map(|s| {
                        let ch = s.snr.channels as usize;
                        DryVoice::new(s, ch, emitter, 0.0)
                    });
                }
                Cmd::Pitch { layer, value } => {
                    if let Some(v) = self.voice(h, layer) {
                        v.resample.pitch = value;
                    }
                }
                Cmd::Gain { layer, value } => {
                    if let Some(v) = self.voice(h, layer) {
                        v.volume = value;
                        v.gain.target = v.volume * v.gain2;
                    }
                }
                Cmd::Stretch { value, .. } => {
                    if value != 1.0 {
                        self.problem(format!("splice slot {slot}: TimeStretch {value} is not ported"));
                    }
                }
                Cmd::Pan { .. } | Cmd::Send { .. } => {}
                Cmd::Stop { layer } => {
                    self.manager.remove((h, layer));
                    if let Some(pl) = self.playing.get_mut(&h) {
                        pl.voices[layer] = None;
                    }
                }
            }
        }
    }

    fn voice(&mut self, h: u32, layer: usize) -> Option<&mut DryVoice> {
        self.playing.get_mut(&h)?.voices.get_mut(layer)?.as_mut()
    }

    /// One block of every started layer into `out` (the board emitter).
    #[cfg(test)]
    pub fn render(&mut self, now: f64, out: &mut [f32; BLOCK]) -> u32 {
        let mut all = [[0.0; BLOCK]; EMITTERS];
        let n = self.render_emitters(now, &mut all).iter().sum();
        for e in &all {
            for (o, s) in out.iter_mut().zip(e) {
                *o += *s;
            }
        }
        n
    }

    /// One block of every started layer into its emitter's block (filter
    /// submixes on the board emitter); the voices per emitter.
    pub fn render_emitters(&mut self, now: f64, out: &mut [[f32; BLOCK]; EMITTERS]) -> [u32; EMITTERS] {
        let mut n = [0; EMITTERS];
        for pl in self.playing.values_mut() {
            let mix = match (pl.chain as usize).checked_sub(1).and_then(|i| self.chains.get_mut(i)) {
                Some(c) => &mut c.mix,
                None => &mut out[pl.emitter],
            };
            for v in pl.voices.iter_mut().flatten() {
                if v.alive() {
                    v.render(now, mix);
                    n[pl.emitter] += 1;
                }
            }
        }
        let out = &mut out[EMITTER_BOARD];
        for c in &mut self.chains {
            c.highpass.process(MIX_RATE, &mut [&mut c.mix[..]]);
            c.lowpass.process(MIX_RATE, &mut [&mut c.mix[..]]);
            c.peak.process(MIX_RATE, &mut [&mut c.mix[..]]);
            for (o, m) in out.iter_mut().zip(&mut c.mix) {
                *o += *m;
                *m = 0.0;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> Option<std::path::PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/audio-cache");
        dir.join("raw/audiofiles/Skate_Collisions.bnk").is_file().then_some(dir)
    }

    /// Plays the pop; (splices, energy, frames).
    fn pop(dir: &Path) -> (Splices, f64, u32) {
        let mut s = Splices::default();
        s.load(dir, SLOT_COLLISIONS, "Skate_Collisions").unwrap();
        let h = s.create(SLOT_COLLISIONS, 1097);
        assert_ne!(h, 0);
        let dt = 1.0 / 60.0;
        s.play(h, 0, Params::at_play(dt));
        let mut energy = 0.0f64;
        let mut frames = 0;
        let mut now = 0.0;
        while s.is_playing(h) && frames < 600 {
            s.service();
            s.update(h, Params { gain: 1.0, pitch: 1.0, pan: 0.0, dt, pan_scale: 1.0, stretch: 1.0 });
            // 800 samples a frame: about three blocks.
            for _ in 0..3 {
                let mut b = [0.0; BLOCK];
                s.render(now, &mut b);
                energy += b.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
                now += BLOCK as f64 / 48_000.0;
            }
            frames += 1;
        }
        s.destroy(h);
        (s, energy, frames)
    }

    #[test]
    fn a_pop_plays_and_ends() {
        let Some(dir) = cache() else { return };
        let (s, energy, frames) = pop(&dir);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert!(!s.started.is_empty());
        assert!(energy > 0.0);
        assert!(frames < 600);
        assert!(s.playing.is_empty());
    }
}

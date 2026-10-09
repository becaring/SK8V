//! The four board grain players, with caller-supplied retail parameter blocks.
//! No speed/volume constants are invented here: the board component supplies
//! the blocks after the retail surface curves and packed parameters run.

use std::{collections::HashMap, path::Path, sync::Arc};
use crate::eac::{grain::{Block, DryGrains, GameRng, GrainParams, Recording}, snr::Snr, BLOCK};
use crate::eac::{frequency_shift::FrequencyShift, gain::Gain, iir2::Iir2};

/// Scalar attributes of the board's 824C8878 dry submix path. The graph
/// is Sub0 → HI20 → LI20 → FSS0 → Sen0 (to the fast path) → Gai0 (speed
/// modulation) → Sen0, into Sub0 → Sen0 (+116 bus, local parameters 21/22)
/// → Sen0 (+52 aux bus, parameter 13) → Pn21 → Sen0 (SFX Master). The two
/// middle sends are taps: neither scales the dry path.
#[derive(Clone,Copy,Debug)]
pub struct Chain {
    pub highpass:f32,
    pub lowpass:f32,
    pub shift:f32,
}
impl Default for Chain {
    fn default()->Self {Self{highpass:0.,lowpass:25000.,shift:0.}}
}

pub struct Player {
    recording: Arc<Recording>,
    dry: DryGrains,
    highpass:Iir2,
    lowpass:Iir2,
    shift:FrequencyShift,
    modulation:Gain,
    first:bool,
}

pub struct Grains {
    recordings: HashMap<String, Arc<Recording>>,
    players: [Option<Player>; 4],
    rng: GameRng,
    /// SkateV roughness layer over the summed rolling grains: level and
    /// top end from the measured grain of the ground.
    rough_gain: Gain,
    rough_lp: Iir2,
}

/// Level and lowpass cutoff for ground roughness `r` (0..=1, 0.5 neutral):
/// +-4.5 dB around the median texture; smoother than the median also loses
/// top end (25 kHz down to ~5 kHz on the smoothest ground).
pub fn roughness_shape(r: Option<f32>) -> (f32, f32) {
    let Some(r) = r.map(|r| r.clamp(0.0, 1.0)) else { return (1.0, 25_000.0) };
    let gain = 2f32.powf((r - 0.5) * 1.5);
    let cutoff = if r < 0.5 { 25_000.0 * 2f32.powf(-(0.5 - r) * 4.6) } else { 25_000.0 };
    (gain, cutoff)
}

/// Loose ground and vegetation keep Skate's own rolling sound, which the
/// roughness layer never lowers; next to typical pavement (2-4 dB under it)
/// it was much too loud. Full trim with all wheels on it.
const LOOSE_TRIM_DB: f32 = -6.0;

pub fn loose_trim(loose: f32) -> f32 {
    10f32.powf(LOOSE_TRIM_DB * loose.clamp(0.0, 1.0) / 20.0)
}

impl Default for Grains {
    fn default() -> Self {
        // Independent host session seed; retail shares this generator with
        // unrelated game systems, so its exact draw sequence is not portable.
        let mut rough_lp = Iir2::new(false, 1);
        rough_lp.cutoff = 25_000.0;
        Self { recordings: HashMap::new(), players: std::array::from_fn(|_| None), rng: GameRng { s: [0; 6] },
            rough_gain: Gain { target: 1.0, current: 1.0 }, rough_lp }
    }
}

impl Grains {
    /// Shared host session generator, also used by the board area envelope.
    pub fn random(&mut self)->u32 {self.rng.next()}
    pub fn load(&mut self, cache: &Path, name: &str) -> Result<(), String> {
        // Only an archive member stem; never allow a config to escape the cache.
        if !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') || name.is_empty() {
            return Err("invalid grain recording name".into());
        }
        if self.recordings.contains_key(name) { return Ok(()); }
        let raw = std::fs::read(cache.join("raw/grains").join(format!("{name}.grain"))).map_err(|e| e.to_string())?;
        let offset = raw.get(..4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize).ok_or("short grain header")?;
        let header = raw.get(offset..).filter(|b| b.len() >= 24).ok_or("invalid grain SNR offset")?;
        let snr = Snr::parse(header);
        if snr.channels != 1 || snr.codec != 3 || snr.samples <= 0 || snr.rate <= 0.0 {
            return Err("unsupported grain SNR".into());
        }
        let bytes = std::fs::read(cache.join("pcm/grains").join(format!("{name}.xma16"))).map_err(|e| e.to_string())?;
        // Continuous XMA frames include the fresh decoder's 384-sample lead-in.
        let samples = snr.samples as usize;
        if bytes.len() % 2 != 0 || bytes.len() / 2 < samples + 384 { return Err("truncated grain decode".into()); }
        let pcm = bytes.chunks_exact(2).skip(384).take(samples)
            .map(|b| i16::from_be_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        self.recordings.insert(name.into(), Arc::new(Recording { rate: snr.rate, duration: samples as f32 / snr.rate, pcm }));
        Ok(())
    }

    pub fn start(&mut self, slot: usize, name: &str, params: GrainParams) -> Result<(), String> {
        let recording = self.recordings.get(name).cloned().ok_or_else(|| format!("grain {name} not loaded"))?;
        let target = self.players.get_mut(slot).ok_or("invalid grain slot")?;
        let mut dry = DryGrains::new();
        dry.set_recording(&recording, params, &mut self.rng);
        let mut lowpass=Iir2::new(false,1);lowpass.cutoff=25000.;
        *target = Some(Player { recording, dry,highpass:Iir2::new(true,1),lowpass,
            shift:FrequencyShift::default(),modulation:Gain{target:1.,current:1.},first:true });
        Ok(())
    }

    pub fn set_block(&mut self, slot: usize, block: &Block) {
        if let Some(Some(p)) = self.players.get_mut(slot) { p.dry.player.set_block(block); }
    }

    pub fn set_chain(&mut self,slot:usize,chain:Chain) {
        if let Some(Some(p))=self.players.get_mut(slot) {
            p.highpass.cutoff=chain.highpass;p.lowpass.cutoff=chain.lowpass;
            p.shift.hz=chain.shift;
        }
    }

    pub fn set_modulation(&mut self,slot:usize,value:f32) {
        if let Some(Some(p))=self.players.get_mut(slot) {p.modulation.target=value;}
    }

    /// The ground under the wheels: roughness (`Frame::roughness`) and the
    /// share on loose ground (`Frame::loose_ground`).
    pub fn set_ground(&mut self, r: Option<f32>, loose: f32) {
        let (gain, cutoff) = roughness_shape(r);
        self.rough_gain.target = gain * loose_trim(loose);
        self.rough_lp.cutoff = cutoff;
    }

    pub fn stop(&mut self, slot: usize) {
        if let Some(p) = self.players.get_mut(slot) { *p = None; }
    }

    pub fn stop_all(&mut self) { self.players = std::array::from_fn(|_| None); }

    pub fn render(&mut self, out: &mut [f32; BLOCK]) -> u32 {
        let mut voices = 0;
        let mut sum = [0.; BLOCK];
        for p in self.players.iter_mut().flatten() {
            let mut mono=[0.;BLOCK];
            p.dry.block(&p.recording, &mut self.rng, &mut mono);
            p.highpass.process(48000.,&mut [&mut mono]);
            p.lowpass.process(48000.,&mut [&mut mono]);
            p.shift.process(48000.,&mut mono);
            p.modulation.process(p.first,&mut [&mut mono]);p.first=false;
            for (dst,src) in sum.iter_mut().zip(mono) {*dst+=src;}
            voices += p.dry.voices();
        }
        self.rough_lp.process(48000., &mut [&mut sum]);
        self.rough_gain.process(false, &mut [&mut sum]);
        for (dst, src) in out.iter_mut().zip(sum) { *dst += src; }
        voices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roughness_is_neutral_at_the_median_and_off_ground() {
        assert_eq!(roughness_shape(None), (1.0, 25_000.0));
        assert_eq!(roughness_shape(Some(0.5)), (1.0, 25_000.0));
        let (g, c) = roughness_shape(Some(0.0));
        assert!((g - 0.59).abs() < 0.01 && (c - 5_100.0).abs() < 100.0, "smooth: quieter, duller");
        let (g, c) = roughness_shape(Some(1.0));
        assert!((g - 1.68).abs() < 0.01 && c == 25_000.0, "grainy: louder, full top end");
        assert_eq!(loose_trim(0.0), 1.0);
        assert!((loose_trim(1.0) - 0.501).abs() < 0.001 && (loose_trim(0.5) - 0.708).abs() < 0.001);
    }

    #[test]
    fn grain_stop_clears_sounding_subvoices() {
        for rate in [24000.0, 48000.0] { check_stop(rate); }
    }
    fn check_stop(rate: f32) {
        let mut g = Grains::default();
        g.recordings.insert("fixture".into(), Arc::new(Recording { rate, duration: 5.0, pcm: vec![0.5; (rate * 5.0) as usize] }));
        g.start(0, "fixture", GrainParams::VOICE_A).unwrap();
        g.set_block(0, &Block { level: 0.5, pitch: 1.0, flags: 0, position: 0.0 });
        let mut out = [0.0; BLOCK];
        for _ in 0..10 { g.render(&mut out); }
        assert!(out.iter().any(|s| *s > 0.0));
        g.stop_all();
        out.fill(0.0);
        assert_eq!(g.render(&mut out), 0);
        assert!(out.iter().all(|s| *s == 0.0));
    }
}

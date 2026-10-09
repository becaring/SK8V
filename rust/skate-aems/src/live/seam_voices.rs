//! Direct seam voices (SkateV): every relief
//! crossing a wheel makes on the measured ground (`Frame::seam_events`)
//! plays at the moment it happened, sample-accurately, instead of going
//! through Class_Seams' 30 Hz program (one toggle word per wheel, the same
//! two samples, an echo pair). The sound material stays Skate's own: the
//! Seams_Bank click (table 32) and the surface layer the program picks per
//! surface (48/56/64 by speed on surface 2, 80, 104, 126), with the
//! program's level, filters and speed pitch (seam_pulse probe). What the
//! map adds: the variant within each sample's group of eight (table
//! entries come in runs of eight of one length class), and depth and kind
//! shaping gain and pitch.

use std::sync::Arc;

use super::cache::BankFile;
use super::engine::Voices;
use super::frame::SeamEvent;
use super::voices::DryVoice;
use super::EMITTER_BOARD;

const BANK: &str = "audiofiles/Seams_Bank";
/// The program's click and surface-layer volumes, filters (seam_pulse).
const CLICK_VOLUME: f32 = 0.70;
const LAYER_VOLUME: f32 = 0.83;
const LOWPASS: f32 = 24_971.0;
const HIGHPASS: f32 = 77.0;
/// A skate wheel (54 mm) only drops into a gap as far as its curve lets it.
const WHEEL_RADIUS: f32 = 0.027;
/// Drops below this are inaudible (a gap under ~4.5 mm); at FULL_DROP (a
/// 3.5 cm gap) the click is at full level. In between the level follows the
/// drop in decibels, not linearly, so small sidewalk gaps stay audible (a
/// 1 cm gap sits 8 dB under a big groove).
const MIN_DROP: f32 = 0.000_1;
const FULL_DROP: f32 = 0.006;
/// Level range across MIN_DROP..FULL_DROP and MIN_LEVEL..FULL_LEVEL.
const DROP_RANGE_DB: f32 = 10.0;
const DEPTH_RANGE_DB: f32 = 6.0;
/// Measured relief below this depth level is texture noise, not a seam (road
/// specks measure 0.02; joints, brick gaps and grate bars 0.1 to 0.4).
const MIN_LEVEL: f32 = 0.06;
/// Depth level at which a seam reaches full level.
const FULL_LEVEL: f32 = 0.4;

/// How far a wheel drops into a gap `width` metres across.
pub fn wheel_drop(width: f32) -> f32 {
    let half = (width * 0.5).clamp(0.0, WHEEL_RADIUS);
    WHEEL_RADIUS - (WHEEL_RADIUS * WHEEL_RADIUS - half * half).sqrt()
}

/// Table index of the surface layer Class_Seams plays (seam_pulse).
pub fn layer_index(surface: u32, speed: f32) -> usize {
    match surface {
        2 if speed >= 10.0 => 64,
        2 if speed >= 5.0 => 56,
        2 => 48,
        1 | 3 | 4 => 104,
        5 | 6 => 126,
        _ => 80,
    }
}

/// 0 dB at a full drop and full depth, falling in decibels toward the gates.
fn size_gain(drop: f32, level: f32) -> f32 {
    let along = |v: f32, lo: f32, hi: f32| ((v.max(lo) / lo).ln() / (hi / lo).ln()).min(1.0);
    let db = -DROP_RANGE_DB * (1.0 - along(drop, MIN_DROP, FULL_DROP))
        - DEPTH_RANGE_DB * (1.0 - along(level.clamp(0.0, 1.0), MIN_LEVEL, FULL_LEVEL));
    10f32.powf(db / 20.0)
}

/// The program's gain2 by speed (0.099 at 1.5 m/s .. 0.125 at 12 m/s).
fn speed_gain(speed: f32) -> f32 {
    0.099 + 0.026 * ((speed - 1.5) / 10.5).clamp(0.0, 1.0)
}

/// The program's pitch by speed (1.0 at 1.5 m/s, 1.26 at 12 m/s).
fn speed_pitch(speed: f32) -> f32 {
    1.0 + 0.025 * (speed - 1.5).clamp(0.0, 14.0)
}

#[derive(Default)]
pub struct SeamVoices {
    bank: Option<Arc<BankFile>>,
    rng: u32,
    last: [usize; 2],
}

impl SeamVoices {
    fn next(&mut self) -> u32 {
        // xorshift32: variation only, never a sequence anyone depends on.
        self.rng = if self.rng == 0 { 0x9E37_79B9 } else { self.rng };
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A variant of `index`'s group of eight, never the one layer `slot` played last.
    fn variant(&mut self, index: usize, slot: usize, count: usize) -> usize {
        let base = index - index % 8;
        let n = 8.min(count.saturating_sub(base)).max(1);
        let mut pick = base + self.next() as usize % n;
        if pick == self.last[slot] && n > 1 {
            pick = base + (pick - base + 1) % n;
        }
        self.last[slot] = pick;
        pick
    }

    /// Loads the click bank and its samples; a bank the cache lacks is a
    /// problem, and the seams then stay silent.
    pub fn load(&mut self, voices: &mut Voices) {
        match voices.cache.bank(BANK) {
            Ok(b) => {
                voices.cache.preload(&b);
                self.bank = Some(Arc::new(b));
            }
            Err(err) => voices.problem(format!("seam voices: {err}")),
        }
    }

    /// Starts `e`'s click and surface layer at system time `start` (seconds);
    /// gaps too narrow for a wheel to drop into stay silent.
    pub fn play(&mut self, voices: &mut Voices, start: f64, e: &SeamEvent) {
        let drop = wheel_drop(e.width);
        if drop < MIN_DROP || e.level < MIN_LEVEL {
            return;
        }
        let Some(bank) = self.bank.clone() else { return };
        let n = (drop / FULL_DROP).min(1.0);
        // Deeper relief and bigger drops: louder; bigger drops a little lower.
        let kind_gain = if e.kind == 3 { 0.8 } else { 1.0 };
        let gain = speed_gain(e.speed) * size_gain(drop, e.level) * kind_gain;
        let pitch = speed_pitch(e.speed) * 2f32.powf(-0.15 * n);
        let count = bank.spans.len();
        let layers = [(32, CLICK_VOLUME), (layer_index(e.surface, e.speed), LAYER_VOLUME)];
        for (slot, (index, volume)) in layers.into_iter().enumerate() {
            let index = self.variant(index, slot, count);
            let Some(sound) = voices.cache.sound(&bank, index) else { continue };
            let channels = sound.snr.channels as usize;
            let mut v = DryVoice::new(sound, channels, EMITTER_BOARD, start);
            // The program's random pitch spread, about +-5 %.
            v.resample.pitch = pitch * (0.95 + 0.1 * self.unit());
            v.volume = volume;
            v.gain2 = gain;
            v.gain.target = volume * gain;
            v.gain.current = v.gain.target;
            v.lp.cutoff = LOWPASS;
            v.hp.cutoff = HIGHPASS;
            voices.insert(v, "Class_Seams (direct)");
            voices.note_start(&bank.stem, index, EMITTER_BOARD);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_stay_in_their_group_and_never_repeat_back_to_back() {
        let mut s = SeamVoices::default();
        let mut prev = usize::MAX;
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..200 {
            let v = s.variant(56, 1, 234);
            assert!((56..64).contains(&v));
            assert_ne!(v, prev);
            seen.insert(v);
            prev = v;
        }
        assert_eq!(seen.len(), 8);
        assert!((232..234).contains(&s.variant(233, 0, 234)), "short last group");
    }

    #[test]
    fn wheels_ride_over_narrow_cracks() {
        assert!(wheel_drop(0.004) < MIN_DROP, "4 mm crack: silent");
        assert!(wheel_drop(0.005) > MIN_DROP, "5 mm sidewalk gap: a click");
        assert!(wheel_drop(0.03) < FULL_DROP && wheel_drop(0.04) > FULL_DROP);
        assert_eq!(wheel_drop(1.0), WHEEL_RADIUS);
    }

    #[test]
    fn small_gaps_stay_audible() {
        let db = |g: f32| 20.0 * g.log10();
        assert!(db(size_gain(FULL_DROP, FULL_LEVEL)).abs() < 1e-4, "big groove unchanged");
        let sidewalk = db(size_gain(wheel_drop(0.01), 0.15));
        assert!((-10.0..-7.0).contains(&sidewalk), "1 cm gap at 0.15 depth: {sidewalk} dB");
        assert!(db(size_gain(MIN_DROP, MIN_LEVEL)) >= -16.01, "quietest click: -16 dB");
    }

    #[test]
    fn layers_follow_the_program() {
        assert_eq!([layer_index(2, 2.0), layer_index(2, 6.0), layer_index(2, 12.0)], [48, 56, 64]);
        assert_eq!([layer_index(0, 6.0), layer_index(3, 6.0), layer_index(6, 6.0)], [80, 104, 126]);
        assert!((speed_pitch(12.0) - 1.26).abs() < 0.01 && (speed_gain(1.5) - 0.099).abs() < 1e-6);
    }
}

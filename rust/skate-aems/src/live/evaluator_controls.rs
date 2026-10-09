//! Temporal MXB controls, from TU3 82951148
//! (attack/hold/release) and 82950D40 (attack/decay/sustain/release).
use super::evaluator::Transfer;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Envelope {
    pub phase: i32,
    pub elapsed: f32,
    pub start_level: i32,
    pub start_time: f32,
    pub raw: i32,
}

// Duration units: TU3 8294F228 reads 822F8F04. The transition threshold
// is a separate constant at 822F8ADC; it is intentionally slightly smaller.
const FRAME_MS: f32 = f32::from_bits(0x4185_5557);
const SHORT_MS: f32 = f32::from_bits(0x4185_53F8);
const Q15_RECIP: f32 = f32::from_bits(0x3800_0100);

fn shape(t: &Transfer, fraction: f32, mode: u8) -> f32 {
    if mode > 9 {
        return 0.;
    }
    t.shape((fraction * 32767.) as i32, mode) as f32 * Q15_RECIP
}
impl Envelope {
    fn enter(&mut self, phase: i32, level: i32) {
        self.phase = phase;
        self.elapsed = 0.;
        self.start_time = 0.;
        self.start_level = level;
    }
    fn reverse(&mut self, phase: i32, from: f32, to: f32, threshold: bool) {
        self.phase = phase;
        self.start_level = self.raw;
        let time = if threshold && from < SHORT_MS {
            to
        } else {
            ((from - self.elapsed) / from) * to
        };
        self.start_time = time;
        self.elapsed = time;
    }
    fn fraction(&self, duration: f32) -> f32 {
        let span = duration - self.start_time;
        let time = self.elapsed - self.start_time;
        if span > 0. { time / span } else { time }
    }
    fn attack(&mut self, duration: f32, curve: u8, t: &Transfer) {
        let amount = shape(t, self.fraction(duration), curve);
        self.raw = self
            .start_level
            .wrapping_add((amount * (32767i32.wrapping_sub(self.start_level)) as f32) as i32);
    }
    fn decay(&mut self, duration: f32, target: i32, curve: u8, t: &Transfer) {
        let amount = 1. - shape(t, 1. - self.fraction(duration), curve);
        self.raw = self
            .start_level
            .wrapping_add((amount * target.wrapping_sub(self.start_level) as f32) as i32);
    }
    fn release(&mut self, duration: f32, curve: u8, t: &Transfer) {
        let amount = 1. - shape(t, 1. - self.fraction(duration), curve);
        self.raw = self
            .start_level
            .wrapping_sub((amount * self.start_level as f32) as i32);
    }
    /// Owns common elapsed-time advancement and the inactive-input reset.
    /// Gain mapping, millibel output and graph links belong to the evaluator.
    pub fn update(&mut self, words: &[u32; 6], input: i32, dt_ms: f32, transfer: &Transfer) {
        if self.phase == 0 && input == 0 {
            *self = Self::default();
            return;
        }
        self.elapsed += dt_ms;
        let mode = (words[0] >> 24) & 15;
        if mode != 1 && mode != 3 {
            return;
        }
        let gated = words[0] & (1 << 8) != 0;
        let retrigger = words[0] & (1 << 10) != 0;
        let attack = (words[3] & 0xfff) as f32 * FRAME_MS;
        let decay = ((words[4] >> 16) & 0xfff) as f32 * FRAME_MS;
        let hold = (words[4] & 0xfff) as f32 * FRAME_MS;
        let release = (words[5] & 0xfff) as f32 * FRAME_MS;
        let sustain = if mode == 3 {
            (words[5] as i32) >> 16
        } else {
            32767
        };
        let attack_curve = ((words[3] >> 12) & 15) as u8;
        let decay_curve = ((words[4] >> 12) & 15) as u8;
        let release_curve = ((words[5] >> 12) & 15) as u8;
        loop {
            match self.phase {
                0 => {
                    self.phase = 1;
                }
                1 => {
                    if gated && input == 0 {
                        self.reverse(4, attack, release, true);
                        continue;
                    }
                    if !(self.elapsed < attack) {
                        self.enter(if mode == 3 { 2 } else { 3 }, 32767);
                        continue;
                    }
                    if (mode == 1 && attack == 0.) || (mode == 3 && !(attack > SHORT_MS)) {
                        self.raw = 32767;
                    } else {
                        self.attack(attack, attack_curve, transfer);
                    }
                    return;
                }
                2 if mode == 3 => {
                    if gated && input == 0 {
                        self.reverse(4, decay, release, false);
                        continue;
                    }
                    if self.elapsed > decay {
                        self.enter(3, sustain);
                        continue;
                    }
                    self.decay(decay, sustain, decay_curve, transfer);
                    return;
                }
                3 => {
                    if (gated && input == 0) || (!gated && self.elapsed > hold) {
                        self.enter(4, sustain);
                        continue;
                    }
                    self.raw = sustain;
                    if gated {
                        self.elapsed = 0.;
                    }
                    return;
                }
                _ => {
                    if !(self.elapsed < release) {
                        *self = Self::default();
                        return;
                    }
                    if input != 0 && retrigger {
                        self.reverse(1, release, attack, true);
                        continue;
                    }
                    self.release(release, release_curve, transfer);
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gate_release_retrigger_retains_current_level() {
        let t = Transfer::default();
        let w = [
            (1 << 24) | (1 << 8) | (1 << 10),
            0,
            0,
            (9 << 12) | 2,
            2,
            (9 << 12) | 2,
        ];
        let mut e = Envelope::default();
        e.update(&w, 32767, 10., &t);
        assert_eq!(e.phase, 1);
        assert_eq!(e.raw, 9830);
        e.update(&w, 0, 5., &t);
        assert_eq!(e.phase, 4);
        assert_eq!(e.raw, 9830);
        assert_eq!(e.elapsed, e.start_time);
        e.update(&w, 32767, 5., &t);
        assert_eq!(e.phase, 1);
        assert_eq!(e.raw, 9830);
        e.update(&w, 32767, 50., &t);
        assert_eq!(e.phase, 3);
        assert_eq!(e.raw, 32767);
        assert_eq!(e.elapsed, 0.);
        e.update(&w, 0, 5., &t);
        assert_eq!(e.phase, 4);
        assert_eq!(e.raw, 32767);
        e.update(&w, 0, 50., &t);
        assert_eq!(e, Envelope::default());
    }
    #[test]
    fn zero_duration_decay_and_sustain_preserve_boundary_order() {
        let t = Transfer::default();
        let w = [3 << 24, 0, 0, 9 << 12, 9 << 12, (16000 << 16) | (9 << 12)];
        let mut e = Envelope::default();
        e.update(&w, 1, 1., &t);
        // Attack enters decay at elapsed zero; zero decay is not skipped until
        // elapsed is strictly greater, so the full peak survives this frame.
        assert_eq!(e.phase, 2);
        assert_eq!(e.raw, 32767);
        e.update(&w, 1, 1., &t);
        assert_eq!(e.phase, 3);
        assert_eq!(e.raw, 16000);
        e.update(&w, 0, 1., &t);
        assert_eq!(e, Envelope::default());
    }
    #[test]
    fn inactive_and_unsupported_mode_follow_common_timing() {
        let t = Transfer::default();
        let w = [4 << 24, 0, 0, 0, 0, 0];
        let mut e = Envelope::default();
        e.update(&w, 1, 17., &t);
        assert_eq!(e.elapsed, 17.);
        e.update(&w, 0, 17., &t);
        assert_eq!(e, Envelope::default());
    }
}

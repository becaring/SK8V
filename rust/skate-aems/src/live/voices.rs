//! EA Audio Core voices for the live engine: `VoiceSystem` (what AEMS's
//! sound player modules call) over dry voice graphs.
//!
//! A voice is the chain Skate's voice builder `0x824A3140` creates through
//! `0x82B48C48`, cut before Pan2D1: SndPlayer1 → Rechannel → Resample →
//! HighPassIir2 → LowPassIir2 → Gain. Attribute sets (voice vtable slot 3,
//! `0x824A29A8`) are posted to the audio thread's queue and applied before
//! the next block (`0x82B463A8`), so they take effect from the next
//! `render` here too.

use std::sync::Arc;

use crate::eac::gain::Gain;
use crate::eac::iir2::Iir2;
use crate::eac::rechannel::{self, Placement, Rechannel};
use crate::eac::resample::{Outcome, Resample};
use crate::eac::sndplayer::{Play, SndPlayer1};
use crate::eac::xma::Sound;
use crate::eac::BLOCK;

/// 1/32767 as the voice's attribute code multiplies (`0x38000100`).
pub const LEVEL: f32 = f32::from_bits(0x3800_0100);
/// The mix rate.
pub const MIX_RATE: f32 = 48_000.0;
/// Most samples a block can pull from the player (pitch ≤ 4, + history).
const MAX_PULL: usize = 4 * BLOCK + 16;
/// The graph's tail (+40): HighPass 450 + LowPass 450 + Resample 6.
pub const TAIL: i32 = 906;

/// One dry voice.
pub struct DryVoice {
    pub player: SndPlayer1,
    pub rechannel: Rechannel,
    pub resample: Resample,
    pub hp: Iir2,
    pub lp: Iir2,
    pub gain: Gain,
    /// `+40` volume, `+44` gain2 (`+48` aux is a send: not dry).
    pub volume: f32,
    pub gain2: f32,
    /// Output channels of the graph (Rechannel's target).
    pub channels: usize,
    pub emitter: usize,
    /// Index of the AEMS class that started it (`Voices::class_of`).
    pub class: usize,
    first: bool,
    started: bool,
    released: bool,
    /// Graph tail left once the source ran dry (+48 - +44, 906 samples).
    tail: i32,
    src: [Vec<f32>; 2],
    mid: [Vec<f32>; 2],
    out: [Vec<f32>; 2],
}

impl DryVoice {
    /// A voice whose sound starts at system time `start` (seconds; 0 = at
    /// the next block), sample-accurately within a block. `channels` is the
    /// graph's (1 or 2), as is the sound's.
    pub fn new(sound: Arc<Sound>, channels: usize, emitter: usize, start: f64) -> DryVoice {
        let sch = sound.snr.channels as usize;
        assert!(sch <= 2 && channels <= 2, "dry voices are mono or stereo");
        let mut player = SndPlayer1::new(sch as u8, 1);
        player.play(&Play { sound, start, offset: 0.0, notify: true, user: 0.0 });
        let mut lp = Iir2::new(false, channels);
        // LowPassIir2's default cutoff (96 kHz): bypassed until set.
        lp.cutoff = 96_000.0;
        DryVoice {
            player,
            rechannel: Rechannel::new(channels as u8, channels as u8),
            resample: Resample::new(channels),
            hp: Iir2::new(true, channels),
            lp,
            gain: Gain { target: 1.0, current: 1.0 },
            volume: 1.0,
            gain2: 1.0,
            channels,
            emitter,
            class: 0,
            first: true,
            started: false,
            released: false,
            tail: TAIL,
            src: std::array::from_fn(|_| vec![0.0; MAX_PULL]),
            mid: std::array::from_fn(|_| vec![0.0; MAX_PULL]),
            out: std::array::from_fn(|_| vec![0.0; BLOCK]),
        }
    }

    /// Voice slot 3 (`0x824A29A8`) for the dry attributes.
    pub fn set(&mut self, id: u32, value: u32) {
        match id {
            0 => self.resample.pitch = value as f32 * (1.0 / 4096.0),
            2 => {
                self.volume = value as f32 * LEVEL;
                self.gain.target = self.volume * self.gain2;
            }
            6 => self.lp.cutoff = value as f32,
            7 => self.hp.cutoff = value as f32,
            8 => {
                self.gain2 = value as f32 * LEVEL;
                self.gain.target = self.volume * self.gain2;
            }
            _ => {}
        }
    }

    /// Voice slot 0 (0x82B1E458): the graph is destroyed at the next
    /// command drain, before the next render: the output just stops.
    pub fn release(&mut self) {
        self.released = true;
    }

    pub fn released(&self) -> bool {
        self.released
    }

    /// Voice slot 5's "alive": the graph is not yet expelled. A one-shot's
    /// entry is released by the service once read out; the graph then runs
    /// its tail over zeros (906 samples) and is expelled (82B485C8).
    pub fn alive(&self) -> bool {
        !self.released && self.tail > 0
    }

    /// The player has read its sound out (a splice layer's graph `+0x47 ==
    /// 2`, `0x82976860`).
    pub fn finished(&self) -> bool {
        self.started && self.player.entries.iter().all(|e| e.state == crate::eac::sndplayer::IDLE)
    }

    /// One 256-sample block at system time `now` (seconds); adds the
    /// voice's output, summed over its channels, to `mix`.
    pub fn render(&mut self, now: f64, mix: &mut [f32; BLOCK]) {
        self.player.service(now);
        let mut factor = 1.0f32;
        let need = self.resample.request(MIX_RATE, BLOCK as u32, &mut factor) as usize;
        let need = need.min(MAX_PULL);
        self.player.set_request(need as u16);
        for b in &mut self.src {
            b[..need].fill(0.0);
        }
        let o = self.player.process(now, MIX_RATE, factor, &mut self.src);
        let count = o.count as usize;
        if count > 0 {
            self.started = true;
        }
        let drained = self.player.entries.iter().all(|e| e.state == crate::eac::sndplayer::IDLE);
        if drained {
            self.tail -= BLOCK as i32;
        }
        // Rechannel (0x82B2C8F0): the block's channels to the graph's (the
        // voice is built with the sample's own count, so this passes
        // through unless the format changes mid-voice).
        let in_ch = (o.channels as usize).clamp(1, self.src.len());
        let swapped = {
            let ins = [&self.src[0][..], &self.src[1][..]];
            let [m0, m1] = &mut self.mid;
            let mut outs = [&mut m0[..], &mut m1[..]];
            self.rechannel.process(in_ch as u8, count as u32, &ins[..in_ch], &mut outs, &Placement::ALIGNED).swapped
        };
        let ch = self.channels.min(if swapped { self.mid.len() } else { in_ch });
        let source = if swapped { &self.mid } else { &self.src };
        for b in &mut self.out {
            b.fill(0.0);
        }
        let ins = [&source[0][..], &source[1][..]];
        let [o0, o1] = &mut self.out;
        let mut views = [&mut o0[..], &mut o1[..]];
        let produced = match self.resample.process(o.rate, count as u32, &ins[..ch], &mut views[..ch]) {
            Outcome::Resampled { count } => count as usize,
            Outcome::RateChanged => 0,
        };
        for b in &mut views[..ch] {
            b[produced.min(BLOCK)..].fill(0.0);
        }
        let views = &mut views[..ch];
        self.hp.process(MIX_RATE, views);
        self.lp.process(MIX_RATE, views);
        self.gain.process(self.first, views);
        self.first = false;
        // The dry hand-off is mono per emitter: a stereo voice folds down
        // with Rechannel's own 2 -> 1 route (L + R, gain 1; table 0x820ED780).
        if ch == 1 {
            for (m, s) in mix.iter_mut().zip(self.out[0].iter()) {
                *m += *s;
            }
        } else {
            let mut mono = [0.0f32; BLOCK];
            let ins = [&self.out[0][..], &self.out[1][..]];
            rechannel::rechannel(&mut [&mut mono[..]], &ins[..ch], 1, ch as u8, BLOCK as u32, &Placement::ALIGNED);
            for (m, s) in mix.iter_mut().zip(mono.iter()) {
                *m += *s;
            }
        }
    }
}

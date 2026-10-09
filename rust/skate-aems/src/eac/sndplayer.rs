//! `SndPlayer1` (descriptor `0x82FD2B74`, vtable `0x8231B9B4`): the voice
//! chain's source, playing a queue of bank samples through the sample
//! decoder ([`super::xma`]). RAM samples only (every AEMS bank sample is
//! one).
//!
//! It works in three places:
//! - **Commands** on the audio thread's queue: PLAY1 (event 5 → `0x82B32DC8`)
//!   fills the next queue entry from the SNR header; STOP (event 1 →
//!   `0x82B32328`) releases everything and arms a 16-sample fade; event 4
//!   (`0x82B33238`) reschedules a pending start.
//! - **Service** (`0x82B31EE0`, registered with the system's second service
//!   list and run after the command queue in the tick `0x82B48530`): creates
//!   decoders, submits blocks as decoder records (each takes one of 20
//!   slots; up to 8,192 samples per run), resubmits the loop, releases
//!   finished entries. An XMA entry with no start time gets one: the system
//!   time plus 256/48000 s.
//! - **Process** (`0x82B34278`, once per block): waits for the entry's start
//!   time, reads the requested samples from the current slot's record, wraps
//!   the play position at the loop, moves to the next entry at the end, and
//!   keeps each channel's last sample for the STOP fade (`0x82B34108`).
//!
//! Instance fields named after their offsets: `+42` channels, `+456` rate,
//! `+460` requested samples, `+462` last samples, `+464` entries (48 bytes:
//! `+0` start time, `+8` decoder, `+12` user value, `+16` rate, `+20`
//! samples, `+24` loop start, `+28` skip, `+32`/`+36` seek, `+46` state,
//! `+47` channels), `+96` per-entry source state (80 bytes), `+100` slots
//! (16 bytes: `+8` samples read, `+12` record, `+13` state, `+14` entry),
//! `+432` play position, `+467`/`+468`/`+469` fill, clean-up and play
//! entries, `+471` output since the last start, `+472` fade samples left,
//! `+473`/`+474`/`+475` slot fill, read and free indices.

use super::snr::{self, Snr};
use super::xma::{Decoder, Record, Sound};
use std::sync::Arc;

pub const SLOTS: usize = 20;
/// Samples submitted per service run before it stops (`0x82B32300`).
pub const SERVICE_BUDGET: i32 = 8192;
/// `0x822F9660`: the delay given to an unscheduled XMA start (one block).
pub const XMA_START_DELAY: f64 = 0.005_333_333_333_333_333;
/// `0x82B32328`: samples of the fade after STOP.
pub const STOP_FADE: u8 = 16;

/// Entry states (`+46`).
pub const IDLE: u8 = 0;
pub const QUEUED: u8 = 1;
pub const PLAYING: u8 = 2;
/// All blocks submitted.
pub const SUBMITTED: u8 = 3;
pub const DONE: u8 = 4;

fn active(state: u8) -> bool {
    state != IDLE && state != DONE
}

/// One queue entry with its source state (`+96` table, 80 bytes).
#[derive(Debug, Default)]
pub struct Entry {
    pub start: f64,
    pub decoder: Option<Decoder>,
    pub user: f32,
    pub rate: f32,
    pub samples: i32,
    pub loop_start: i32,
    pub skip: i32,
    pub seek: [i32; 2],
    pub state: u8,
    pub channels: u8,
    /// Source state: the sample, `+48` next block, `+52` loop block, `+20`
    /// samples submitted, `+74` slot being filled, `+75` notify the owner
    /// when released.
    pub sound: Option<Arc<Sound>>,
    pub next_block: usize,
    pub loop_block: usize,
    pub submitted: i32,
    pub slot: u8,
    pub notify: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub read: i32,
    pub record: u8,
    /// 0 free, 1 holds a record, 2 read out (freed by the next service).
    pub state: u8,
    pub entry: u8,
}

/// PLAY1's command data (the queued command at `+8`..).
#[derive(Clone, Debug)]
pub struct Play {
    pub sound: Arc<Sound>,
    /// `+8`: start time in seconds of system time; 0 for "now".
    pub start: f64,
    /// `+24`: start offset in seconds (only 0 is ported).
    pub offset: f64,
    /// `+46`: notify the owner when the sound is released.
    pub notify: bool,
    /// `+48`: a value the game identifies the sound by (event 4).
    pub user: f32,
}

/// What a process call delivered (context `+48`, `+60`, `+52`, and the
/// return value).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    pub ret: bool,
    pub count: u32,
    pub channels: u8,
    pub rate: f32,
}

#[derive(Debug)]
pub struct SndPlayer1 {
    pub channels: u8,
    pub rate: f32,
    pub max_channels: u8,
    pub request: u16,
    pub last: Vec<f32>,
    pub entries: Vec<Entry>,
    pub slots: [Slot; SLOTS],
    pub fill: u8,
    pub clean: u8,
    pub play: u8,
    pub slot_fill: u8,
    pub slot_read: u8,
    pub slot_free: u8,
    pub had_output: bool,
    pub fade: u8,
    /// `+424`, `+428`, `+432`, `+436`: the playing entry's user value, rate,
    /// position and length.
    pub user: f32,
    pub play_rate: f32,
    pub position: i32,
    pub length: i32,
    /// Entries released with `notify` since the caller last cleared this
    /// (the game marks the owning voice finished, `0x82B340A8`).
    pub notified: u32,
}

impl SndPlayer1 {
    /// As `0x82B325A8` leaves it: `channels` (`+42`) from the voice, `queue`
    /// entries (attribute 0, rounded; 1 without attributes).
    pub fn new(channels: u8, queue: usize) -> SndPlayer1 {
        SndPlayer1 {
            channels,
            rate: 48_000.0,
            max_channels: channels,
            request: 0,
            // The array holds `channels` floats, padded to 8 bytes before
            // the entries; the STOP fade walks the *current* channel count,
            // so a mono player that last played a stereo sample fades
            // channel 1 from (and into) the padding word.
            last: vec![0.0; (channels as usize + 1) & !1],
            entries: (0..queue).map(|_| Entry::default()).collect(),
            slots: [Slot::default(); SLOTS],
            fill: 0,
            clean: 0,
            play: 0,
            slot_fill: 0,
            slot_read: 0,
            slot_free: 0,
            had_output: false,
            fade: 0,
            user: 0.0,
            play_rate: 48_000.0,
            position: 0,
            length: 0,
            notified: 0,
        }
    }

    fn next_entry(&self, i: u8) -> u8 {
        if i as usize + 1 == self.entries.len() { 0 } else { i + 1 }
    }

    fn next_slot(i: u8) -> u8 {
        if i as usize + 1 == SLOTS { 0 } else { i + 1 }
    }

    /// `0x82B34268`: samples the next process should deliver.
    pub fn set_request(&mut self, samples: u16) {
        self.request = samples;
    }

    /// PLAY1 (`0x82B32DC8`) for a RAM sample. Dropped when the next entry
    /// is still in use.
    pub fn play(&mut self, cmd: &Play) {
        let i = self.fill as usize;
        if self.entries[i].state != IDLE {
            return;
        }
        let snr: &Snr = &cmd.sound.snr;
        assert!(snr.kind == snr::KIND_RAM && snr.version == 0, "only version-0 RAM samples are ported");
        let e = &mut self.entries[i];
        e.user = cmd.user;
        e.decoder = None;
        e.start = cmd.start;
        e.notify = cmd.notify;
        e.state = QUEUED;
        e.submitted = 0;
        e.channels = snr.channels;
        e.rate = snr.rate;
        e.samples = snr.samples;
        e.loop_start = snr.loop_start;
        let offset = crate::ppc::fctiwz(snr.rate as f64 * cmd.offset);
        assert!(offset <= 0, "start offsets are not ported");
        if e.samples <= 0 {
            e.state = IDLE;
            e.samples = 0;
            return;
        }
        // 0x82B33780 without seek tables.
        e.seek = [0, 0];
        e.skip = 0;
        e.sound = Some(cmd.sound.clone());
        e.state = QUEUED;
        self.fill = self.next_entry(self.fill);
    }

    /// STOP (`0x82B32328`).
    pub fn stop(&mut self) {
        while active_or_queued(self.entries[self.clean as usize].state) {
            self.release(self.clean);
            self.clean = self.next_entry(self.clean);
        }
        self.play = 0;
        self.fill = 0;
        self.clean = 0;
        self.position = 0;
        self.length = 0;
        self.slot_fill = 0;
        self.slot_read = 0;
        self.slot_free = 0;
        self.fade = STOP_FADE;
    }

    /// Event 4 (`0x82B33238`): an active entry identified by `user` whose
    /// start is still ahead of `now` starts at `start` instead.
    pub fn reschedule(&mut self, user: f32, start: f64, now: f64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.user == user && active(e.state))
            && e.start > now
        {
            e.start = start;
        }
    }

    /// Frees an entry and its decoder (`0x82B33F98`).
    fn release(&mut self, i: u8) {
        self.entries[i as usize].decoder = None;
        if self.slots[self.slot_read as usize].entry == i {
            loop {
                let s = &mut self.slots[self.slot_read as usize];
                if s.state == 2 || s.state == 0 {
                    break;
                }
                s.state = 2;
                self.slot_read = Self::next_slot(self.slot_read);
                if self.slots[self.slot_read as usize].entry != i {
                    break;
                }
            }
        }
        self.free_slots();
        let e = &mut self.entries[i as usize];
        e.state = IDLE;
        if e.notify {
            self.notified += 1;
        }
    }

    /// `0x82B32480`: slots read out become free.
    fn free_slots(&mut self) {
        while self.slots[self.slot_free as usize].state == 2 {
            self.slots[self.slot_free as usize].state = 0;
            self.slot_free = Self::next_slot(self.slot_free);
        }
    }

    /// The next free output slot, if any (`0x82B32550`).
    fn take_slot(&mut self) -> Option<u8> {
        let s = self.slot_fill;
        if self.slots[s as usize].state != 0 {
            return None;
        }
        self.slot_fill = Self::next_slot(s);
        Some(s)
    }

    /// `0x82B33418`: block `at` of entry `i` as a record in slot `+74`;
    /// returns the next block. `cont`: the block continues the previous one.
    fn submit(&mut self, i: u8, at: usize, cont: bool, budget: &mut i32) -> usize {
        let e = &mut self.entries[i as usize];
        let sound = e.sound.clone().unwrap();
        let b = snr::block(&sound.bytes, at, sound.snr.version);
        let slot = e.slot as usize;
        self.slots[slot] = Slot { read: 0, record: 0, state: 1, entry: i };
        let rec = Record { block: at, start: 0, end: b.samples as i32, bytes: b.size - 8, flag: cont as u8 };
        self.slots[slot].record = e.decoder.as_mut().unwrap().add_record(rec);
        *budget += b.samples as i32;
        e.submitted += b.samples as i32;
        at + b.size as usize
    }

    /// `0x82B33D40` for RAM: the decoder and the first block.
    fn start(&mut self, i: u8, budget: &mut i32) {
        let e = &mut self.entries[i as usize];
        let sound = e.sound.clone().unwrap();
        e.decoder = Some(Decoder::new(sound.clone()));
        // The service only gets here with a free slot.
        let slot = self.take_slot().expect("a free slot");
        self.entries[i as usize].slot = slot;
        // +77 is 1 (no seek tables): the first block is fresh.
        let next = self.submit(i, sound.snr.data, false, budget);
        self.entries[i as usize].next_block = next;
    }

    /// The service (`0x82B31EE0`) at system time `now`.
    pub fn service(&mut self, now: f64) {
        self.free_slots();
        while self.entries[self.clean as usize].state == DONE {
            self.release(self.clean);
            self.clean = self.next_entry(self.clean);
        }
        let mut i = self.play;
        if !active(self.entries[i as usize].state) {
            return;
        }
        if self.entries[i as usize].samples == 0 {
            loop {
                i = self.next_entry(i);
                if i == self.play || !active(self.entries[i as usize].state) {
                    return;
                }
                if self.entries[i as usize].samples != 0 {
                    break;
                }
            }
        }
        let mut budget = 0i32;
        loop {
            if !active(self.entries[i as usize].state) || self.slots[self.slot_fill as usize].state != 0 {
                return;
            }
            if self.entries[i as usize].state == QUEUED {
                self.start(i, &mut budget);
                let e = &mut self.entries[i as usize];
                e.state = PLAYING;
                if e.sound.as_ref().unwrap().snr.codec == snr::CODEC_XMA && e.start == 0.0 && i == self.play {
                    e.start = now + XMA_START_DELAY;
                }
            }
            let e = &self.entries[i as usize];
            if e.state == PLAYING && self.slots[self.slot_fill as usize].state == 0 {
                if e.submitted == e.loop_start {
                    // 0x82B33870: the loop block starts fresh.
                    let at = e.next_block;
                    self.entries[i as usize].loop_block = at;
                    let slot = self.take_slot().unwrap();
                    self.entries[i as usize].slot = slot;
                    let next = self.submit(i, at, false, &mut budget);
                    self.entries[i as usize].next_block = next;
                } else if e.submitted == e.samples {
                    // 0x82B33970.
                    if e.loop_start < 0 {
                        self.entries[i as usize].state = SUBMITTED;
                        i = self.next_entry(i);
                        if i == self.play {
                            return;
                        }
                    } else {
                        let e = &mut self.entries[i as usize];
                        if e.loop_start == 0 {
                            e.loop_block = e.sound.as_ref().unwrap().snr.data;
                        }
                        e.submitted = e.loop_start;
                        let at = e.loop_block;
                        let slot = self.take_slot().unwrap();
                        self.entries[i as usize].slot = slot;
                        let next = self.submit(i, at, false, &mut budget);
                        self.entries[i as usize].next_block = next;
                    }
                } else {
                    // 0x82B332D0.
                    let e = &self.entries[i as usize];
                    let sound = e.sound.clone().unwrap();
                    let (at, next) = snr::next_block(&sound.bytes, e.next_block, e.loop_block, e.channels);
                    self.entries[i as usize].next_block = next;
                    let Some(slot) = self.take_slot() else { return };
                    self.entries[i as usize].slot = slot;
                    // +48 stays where the walk left it (0x82B332D0 drops
                    // 0x82B33418's return value).
                    self.submit(i, at, true, &mut budget);
                }
            } else {
                i = self.next_entry(i);
                if i == self.play {
                    return;
                }
            }
            if budget > SERVICE_BUDGET {
                return;
            }
        }
    }

    /// Moves play on to the next entry (`0x82B349A8`).
    fn advance_entry(&mut self) {
        self.play = self.next_entry(self.play);
        self.position = 0;
        self.length = 0;
        let e = &self.entries[self.play as usize];
        if active(e.state) && e.state != QUEUED {
            self.position = 0;
            self.user = e.user;
            self.play_rate = e.rate;
            self.length = e.samples;
        }
        self.had_output = false;
    }

    /// `0x82B34108`: the STOP fade, from each channel's last sample to 0.
    fn fade_out(&mut self, out: &mut [Vec<f32>]) -> Output {
        let left = self.fade;
        let n = (left as u16).min(self.request) as usize;
        if self.channels != 0 {
            let scale = 1.0f32 / left as i32 as f32;
            for ch in 0..self.channels as usize {
                let mut v = self.last[ch];
                let step = v * scale;
                for s in &mut out[ch][..n] {
                    v -= step;
                    *s = v;
                }
                self.last[ch] = v;
            }
        }
        self.fade = left - n as u8;
        if self.fade == 0 {
            self.had_output = false;
        }
        Output { ret: true, count: n as u32, channels: self.channels, rate: self.rate }
    }

    /// One block (`0x82B34278`). `time`: the block's time (context `+16`);
    /// `mix_rate`: the system rate (`+40` → `+12`); `rate_factor`: context
    /// `+56`. Writes into `out` (one buffer per channel, at least
    /// `request` long).
    pub fn process(&mut self, time: f64, mix_rate: f32, rate_factor: f32, out: &mut [Vec<f32>]) -> Output {
        if self.fade != 0 && self.had_output {
            return self.fade_out(out);
        }
        self.fade = 0;
        let mut read = 0i32;
        let mut skipped = 0i32;
        'play: {
            let mut i = self.play as usize;
            if !active(self.entries[i].state) {
                break 'play;
            }
            while self.entries[i].samples == 0 {
                self.entries[i].state = DONE;
                self.advance_entry();
                i = self.play as usize;
                if !active(self.entries[i].state) {
                    break 'play;
                }
            }
            let e = &self.entries[i];
            if e.state != PLAYING && e.state != SUBMITTED {
                break 'play;
            }
            if e.rate != self.rate || e.channels != self.channels {
                self.rate = e.rate;
                self.channels = e.channels;
                return Output { ret: true, count: 0, channels: e.channels, rate: e.rate };
            }
            if self.slots[self.slot_read as usize].state == 0 {
                while self.slot_read != self.slot_fill {
                    self.slot_read = Self::next_slot(self.slot_read);
                    if self.slots[self.slot_read as usize].state != 0 {
                        break;
                    }
                }
            }
            if self.slots[self.slot_read as usize].state != 1 {
                break 'play;
            }
            if e.start != 0.0 {
                let d = e.start - time;
                let wait = if d > 0.0 {
                    let samples = (mix_rate as f64 * d) as f32;
                    if samples >= 256.0 {
                        self.position = 0;
                        break 'play;
                    }
                    crate::ppc::fctidz((rate_factor * samples) as f64) as u32
                } else {
                    0
                };
                if wait != 0 {
                    let n = wait.min(self.request as u32) as usize;
                    for buf in out.iter_mut().take(e.channels as usize) {
                        buf[..n].fill(0.0);
                    }
                    self.position = 0;
                    return Output { ret: true, count: n as u32, channels: e.channels, rate: e.rate };
                }
                self.entries[i].start = 0.0;
            }
            let e = &mut self.entries[i];
            let rec = self.slots[self.slot_read as usize].record;
            let dec = e.decoder.as_mut().unwrap();
            let avail = dec.available(rec);
            let mut skip = e.skip.min(avail);
            let want = (self.request as i32).min(avail - skip);
            while skip != 0 {
                let k = skip.min(256);
                skip -= k;
                skipped += dec.read(out, k);
            }
            read = dec.read(out, want);
            if read > 0 {
                self.had_output = true;
                let n = e.channels.min(self.max_channels) as usize;
                for ch in 0..n {
                    self.last[ch] = out[ch][read as usize - 1];
                }
            }
            self.user = e.user;
            if self.position == 0 {
                self.position = e.seek[1] + e.seek[0];
            }
            self.position += read + skipped;
            let mut rest = avail - read - skipped;
            self.play_rate = e.rate;
            self.length = e.samples;
            self.slots[self.slot_read as usize].read += read + skipped;
            let mut decoding = true;
            if self.position == e.samples {
                if e.loop_start >= 0 {
                    self.position = e.loop_start;
                } else {
                    e.state = DONE;
                    self.advance_entry();
                    let n = &self.entries[self.play as usize];
                    decoding = active(n.state) && n.decoder.is_some();
                }
            }
            while rest == 0 {
                let s = self.slot_read as usize;
                if self.slots[s].state != 1 {
                    break;
                }
                self.slots[s].state = 2;
                self.slot_read = Self::next_slot(self.slot_read);
                let next = self.slots[self.slot_read as usize];
                if decoding && next.state == 1 {
                    // The decoder of the entry now playing.
                    rest = self.entries[self.play as usize].decoder.as_ref().unwrap().available(next.record);
                }
            }
            let e = &self.entries[i];
            let out = Output { ret: true, count: read as u32, channels: e.channels, rate: e.rate };
            return self.tail(out, read, skipped);
        }
        let out = Output { ret: true, count: 0, channels: self.channels, rate: self.rate };
        self.tail(out, read, skipped)
    }

    /// `0x82B34918`: the context gets the player's format; nothing read with
    /// a request outstanding returns 0.
    fn tail(&self, mut out: Output, read: i32, skipped: i32) -> Output {
        out.channels = self.channels;
        out.rate = self.rate;
        out.ret = !(read == 0 && skipped == 0 && self.request != 0);
        out
    }
}

fn active_or_queued(state: u8) -> bool {
    state != IDLE
}

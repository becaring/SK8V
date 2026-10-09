//! EA Audio Core's sample decoder (`0x82B3CD38`) with the EA-XMA codec
//! (`EXm0`, descriptor `0x82FCD3A4`): the records SndPlayer1 hands it and
//! the samples it returns.
//!
//! **Records.** Every EA block SndPlayer1 submits becomes a record in a ring
//! of 20 (`+36`; 24 bytes: `+0` data, `+8` start, `+12` end = the block's
//! samples, `+16` bytes, `+20` flag). Flag 0 marks a block that starts a
//! fresh XMA stream (the first block, the loop block); flag 1 continues the
//! previous block. `+47` is the next record to fill, `+49` the record being
//! read, `+28` the position in it.
//!
//! **XMA.** One context per stream (`0x82B4F8C8`) is initialized once and
//! fed every record's packets in order (`0x82B4FE40`): all blocks decode as
//! one continuous stream into the context's output ring. Reading a record
//! (`0x82B50B80`) first drops `(start + k) mod 512` samples plus a carry,
//! with `k` = 384 for a fresh record (the encoder's lead-in) and 0 for a
//! continuation; the carry is the rest of the previous record's last
//! 512-sample frame (`512 - (end + k) mod 512`). Samples convert as
//! `s16 / 32768` (`0x82B50380`).
//!
//! The decoded stream comes from [`Sound::pcm`]: the sound's blocks decoded
//! once, continuously (`skate-xma --continuous`). A block holds
//! `ceil((samples + k) / 512)` frames, which matches all 5,165 bank samples.
//! A looped sound wraps from its last block back to its loop block; the
//! loop block's frames decode identically on every pass (checked over all
//! 399 looped bank samples: the 384-sample lead-in absorbs the decoder's
//! history), so the wrap reuses them.
//!
//! The model assumes the XMA context always has the frames ready when a
//! record is read. When it does not, the game gives up on the record: it
//! outputs silence, reinitializes the context and resumes at the next
//! record (`0x82B50F00`). That happens on the recompiled game for sounds where
//! its decoder drops a frame, not on hardware.

use super::snr::{self, Snr};
use std::collections::VecDeque;
use std::sync::Arc;

pub const RECORDS: usize = 20;
pub const FRAME: i32 = 512;
/// The lead-in skipped at the start of a fresh record.
pub const LEAD_IN: i32 = 384;

/// A bank sample: SNR header and blocks, and its decoded PCM.
#[derive(Debug)]
pub struct Sound {
    pub snr: Snr,
    /// The header and blocks, as the bank holds them.
    pub bytes: Vec<u8>,
    /// Big-endian PCM16 frames as the XMA context writes them, decoded to
    /// host `i16`, interleaved for stereo.
    pub pcm: Vec<i16>,
    /// Byte offset of each block and its first frame in `pcm`.
    pub blocks: Vec<(usize, usize)>,
}

impl Sound {
    /// `bytes`: the SNR header and blocks; `xma16`: the continuous decode.
    pub fn new(bytes: Vec<u8>, xma16: &[u8]) -> Result<Sound, String> {
        let snr = Snr::parse(&bytes);
        if snr.codec != snr::CODEC_XMA || snr.kind != snr::KIND_RAM || snr.channels > 2 {
            return Err(format!("unsupported sample: codec {} kind {} channels {}", snr.codec, snr.kind, snr.channels));
        }
        let pcm: Vec<i16> = xma16.chunks_exact(2).map(|b| i16::from_be_bytes([b[0], b[1]])).collect();
        let mut blocks = Vec::new();
        let (mut at, mut first, mut frame) = (snr.data, 0i64, 0usize);
        while first < snr.samples as i64 {
            let b = snr::block(&bytes, at, snr.version);
            let fresh = first == 0 || first == snr.loop_start as i64;
            let k = if fresh { LEAD_IN as i64 } else { 0 };
            blocks.push((at, frame));
            frame += ((b.samples as i64 + k + FRAME as i64 - 1) / FRAME as i64) as usize;
            first += b.samples as i64;
            if b.tag & 0x80 != 0 || b.size == 0 {
                break;
            }
            at += b.size as usize;
        }
        let want = frame * FRAME as usize * snr.channels as usize;
        if pcm.len() != want {
            return Err(format!("decoded {} samples, the blocks need {}", pcm.len(), want));
        }
        if snr.looped && !blocks.iter().any(|&(at, _)| at == loop_block(&snr, &bytes, &blocks)) {
            return Err("loop start is not at a block".into());
        }
        Ok(Sound { snr, bytes, pcm, blocks })
    }

    /// The frames of the block at byte offset `block`: first frame and
    /// count.
    pub fn frames(&self, block: usize) -> (usize, usize) {
        let k = self.blocks.iter().position(|&(at, _)| at == block).expect("a block of this sound");
        let end = match self.blocks.get(k + 1) {
            Some(&(_, f)) => f,
            None => self.pcm.len() / (FRAME as usize * self.snr.channels as usize),
        };
        (self.blocks[k].1, end - self.blocks[k].1)
    }
}

/// The block whose first sample is the loop start.
fn loop_block(snr: &Snr, bytes: &[u8], blocks: &[(usize, usize)]) -> usize {
    let mut first = 0i64;
    for &(at, _) in blocks {
        if first == snr.loop_start as i64 {
            return at;
        }
        first += snr::block(bytes, at, snr.version).samples as i64;
    }
    usize::MAX
}

/// A decoder record (`+0` data → the block's offset here).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub block: usize,
    pub start: i32,
    pub end: i32,
    pub bytes: u32,
    pub flag: u8,
}

/// One decoder: the generic record ring plus the XMA codec's read state.
#[derive(Debug)]
pub struct Decoder {
    pub sound: Arc<Sound>,
    pub records: [Record; RECORDS],
    /// `+47`, `+49`, `+28`.
    pub write: u8,
    pub read: u8,
    pub pos: i32,
    /// XMA `+72` samples left in the record being read, `+76` carry, `+56`
    /// that record; the stream's `+16` samples still to drop.
    remaining: i32,
    carry: i32,
    current: Record,
    drop: i32,
    /// The context's output in order: runs of samples (per channel) of
    /// `pcm`, one per submitted record, and the read position in the first.
    ring: VecDeque<(usize, usize)>,
}

impl Decoder {
    pub fn new(sound: Arc<Sound>) -> Decoder {
        Decoder {
            sound,
            records: [Record::default(); RECORDS],
            write: 0,
            read: 0,
            pos: 0,
            remaining: 0,
            carry: 0,
            current: Record::default(),
            drop: 0,
            ring: VecDeque::new(),
        }
    }

    pub fn channels(&self) -> usize {
        self.sound.snr.channels as usize
    }

    /// `0x82B3C870`: fills record `+47` and hands its packets to the context
    /// (vtable slot 0, `0x82B50B78` → `0x82B4FE40`); returns the record's
    /// index, or 0 when the ring is full.
    pub fn add_record(&mut self, r: Record) -> u8 {
        let w = self.write as usize;
        if self.records[w].end != 0 {
            return 0;
        }
        self.records[w] = r;
        assert!(r.start == 0, "records with a start offset (seeks) are not ported");
        let (first, frames) = self.sound.frames(r.block);
        self.ring.push_back((first * FRAME as usize, frames * FRAME as usize));
        if self.read == self.write {
            self.pos = r.start;
        }
        self.write = ((w + 1) % RECORDS) as u8;
        w as u8
    }

    /// `0x82B23C10`: samples left in record `rec`.
    pub fn available(&self, rec: u8) -> i32 {
        let r = &self.records[rec as usize];
        if r.end == 0 {
            0
        } else if rec == self.read {
            r.end - self.pos
        } else {
            r.end - r.start
        }
    }

    /// `0x82B3CA60` for a codec without an internal buffer (EXm0's `+24` is
    /// 0): up to `count` samples per channel. Each record's samples are
    /// written from `out[..][0]`, as the original passes the same buffers
    /// every time round its loop.
    pub fn read(&mut self, out: &mut [Vec<f32>], count: i32) -> i32 {
        let mut done = 0;
        while done < count {
            let r = self.records[self.read as usize];
            if r.end == 0 {
                break;
            }
            let n = (count - done).min(r.end - self.pos);
            self.decode(out, n);
            self.advance(n);
            done += n;
        }
        done
    }

    /// Advances the read position by `n` samples (`0x82B3C9D8`).
    fn advance(&mut self, n: i32) {
        self.pos += n;
        let r = &mut self.records[self.read as usize];
        if self.pos != r.end {
            return;
        }
        r.end = 0;
        self.read = ((self.read as usize + 1) % RECORDS) as u8;
        self.pos = self.records[self.read as usize].start;
    }

    /// `0x82B50B80` with the frames always ready.
    fn decode(&mut self, out: &mut [Vec<f32>], mut count: i32) -> i32 {
        let mut written = 0;
        while count != 0 {
            if self.remaining == 0 {
                let r = self.records[self.read as usize];
                assert!(r.end != 0, "XMA read past the submitted records");
                let k = if r.flag == 0 { LEAD_IN } else { 0 };
                // srawi/addze: the frame rounds toward zero.
                let frame = (r.start + k) / FRAME;
                self.drop += r.start - frame * FRAME + self.carry + k;
                self.carry = 0;
                self.remaining = r.end - r.start;
                self.current = r;
            }
            let n = count.min(self.remaining);
            let d = self.drop as usize;
            self.drop = 0;
            self.pull(d, None);
            self.pull(n as usize, Some((&mut *out, written as usize)));
            self.remaining -= n;
            written += n;
            count -= n;
            if self.remaining == 0 && n != 0 {
                let k = if self.current.flag == 0 { LEAD_IN } else { 0 };
                let e = (self.current.end + k) & (FRAME - 1);
                if e != 0 {
                    self.carry = FRAME - e;
                }
            }
        }
        written
    }

    /// Takes `n` samples per channel from the context's output, converting
    /// them into `out` from `at` (`0x82B50380`: `s16 / 32768`) or dropping
    /// them.
    fn pull(&mut self, mut n: usize, mut out: Option<(&mut [Vec<f32>], usize)>) {
        let ch = self.channels();
        let mut k = 0;
        while n > 0 {
            let (start, len) = self.ring.front_mut().expect("XMA context ran dry");
            let take = n.min(*len);
            if let Some((out, at)) = out.as_mut() {
                let src = &self.sound.pcm[*start * ch..(*start + take) * ch];
                for (c, buf) in out.iter_mut().enumerate().take(ch) {
                    let dst = &mut buf[*at + k..*at + k + take];
                    for (d, frame) in dst.iter_mut().zip(src.chunks_exact(ch)) {
                        *d = frame[c] as f32 / 32768.0;
                    }
                }
            }
            *start += take;
            *len -= take;
            if *len == 0 {
                self.ring.pop_front();
            }
            n -= take;
            k += take;
        }
    }
}

//! EA sample headers ("SNR", parsed by `0x82B335A8`) and the EA blocks that
//! follow them (`0x82B33C08`, `0x82B33418`).
//!
//! A bank sample is its SNR header followed by blocks: `[u32 flags|size]`
//! `[u32 samples]` and the codec data (for EA-XMA, items of XMA2 packets).
//! The top bit of the first word marks the last block.

use crate::be;

/// `0x82B26B70`: an MSB-first bit reader over a byte buffer.
pub struct BitReader<'a> {
    data: &'a [u8],
    pub pos: u32,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    pub fn read(&mut self, mut n: u32) -> u32 {
        let mut v = 0u32;
        while n > 0 {
            let bit = self.pos & 7;
            let take = (8 - bit).min(n);
            let byte = self.data[(self.pos >> 3) as usize] as u32;
            v = (v << take) | ((byte >> (8 - take - bit)) & ((1 << take) - 1));
            self.pos += take;
            n -= take;
        }
        v
    }
}

/// Where a sample's data lives (`+73` of the player's per-entry state).
pub const KIND_RAM: u8 = 0;
pub const KIND_STREAM: u8 = 1;
pub const KIND_GIGASAMPLE: u8 = 2;

/// EA-XMA (`+72` codec 3; the codec table at `0x82119870` maps it to `EXm0`).
pub const CODEC_XMA: u8 = 3;

/// A parsed SNR header.
#[derive(Clone, Debug, PartialEq)]
pub struct Snr {
    pub version: u8,
    pub codec: u8,
    pub channels: u8,
    pub rate: f32,
    pub kind: u8,
    pub looped: bool,
    pub samples: i32,
    /// `-1` when not looped.
    pub loop_start: i32,
    /// Gigasample: samples held in RAM (`+16`).
    pub prefetch: u32,
    /// Streams: byte offset of the loop start (`+12`).
    pub loop_offset: u32,
    /// Offset of the first block from the start of the header bytes given.
    pub data: usize,
}

impl Snr {
    /// `0x82B335A8` with a header pointer: an optional `'H'` word, then
    /// version (4), codec (4), channels - 1 (6), rate (18), kind (2), loop
    /// flag (1), samples (29), loop start (32, looped only), the gigasample
    /// prefetch (32) and the stream loop offset (32) where they apply.
    pub fn parse(header: &[u8]) -> Snr {
        let skip = if header[0] == b'H' { 4 } else { 0 };
        let mut r = BitReader::new(&header[skip..]);
        let version = r.read(4) as u8;
        let codec = r.read(4) as u8;
        let channels = r.read(6) as u8 + 1;
        let rate = r.read(18) as f32;
        let kind = r.read(2) as u8;
        let looped = r.read(1) != 0;
        let samples = r.read(29) as i32;
        let loop_start = if looped { r.read(32) as i32 } else { -1 };
        let prefetch = if kind == KIND_GIGASAMPLE { r.read(32) } else { 0 };
        let loop_offset = if looped && (kind == KIND_STREAM || (kind == KIND_GIGASAMPLE && loop_start >= prefetch as i32)) {
            r.read(32)
        } else {
            0
        };
        Snr {
            version,
            codec,
            channels,
            rate,
            kind,
            looped,
            samples,
            loop_start,
            prefetch,
            loop_offset,
            data: skip + (r.pos >> 3) as usize,
        }
    }
}

/// One block header as `0x82B33418` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    /// First byte: `0x80` on the last block of a sound; `'E'`, `'H'`, `'U'`
    /// mark stream end, header and unused blocks.
    pub tag: u8,
    /// Bytes including the 8-byte header (version 0: the first word without
    /// its top bit; otherwise its low 24 bits).
    pub size: u32,
    pub samples: u32,
}

pub fn block(data: &[u8], at: usize, version: u8) -> Block {
    let w = be::u32(data, at);
    let samples = be::u32(data, at + 4);
    let size = if version == 0 { w & 0x7FFF_FFFF } else { w & 0xFF_FFFF };
    Block { tag: data[at], size, samples }
}

/// `0x82B33C08` for RAM samples: the block at `at` and where the walk goes
/// next. An `'E'` block jumps to `loop_at`; `'E'`, `'U'` and (for samples
/// with channels) `'H'` blocks are passed over. `0x82B33C08` advances by
/// the 24-bit size whatever the version.
pub fn next_block(data: &[u8], mut at: usize, loop_at: usize, channels: u8) -> (usize, usize) {
    loop {
        let here = at;
        let size = (be::u32(data, at) & 0xFF_FFFF) as usize;
        at = if data[here] == b'E' { loop_at } else { here + size };
        match data[here] {
            b'E' | b'U' => continue,
            b'H' if channels != 0 => continue,
            _ => return (here, at),
        }
    }
}

//! Skate 3's second sound path: splice banks (`*.bnk`, magic `SPLC`) and the
//! game's splice sound runtime (`0x82974BD0`..`0x82977188`, TU3), which plays
//! one-shots (board landings, pops, foot plants, body impacts, foley,
//! whooshes, menu sounds) without AEMS.
//!
//! # File (loader `0x828DC660` type 2 → `0x82974F48`)
//!
//! All big-endian. `D` = `0x3C` (start of the sound data).
//!
//! | at | size | field |
//! |---|---|---|
//! | `+0x00` | 4 | `SPLC` |
//! | `+0x04` | 4 | version (3) |
//! | `+0x08` | 4 | `size`: bytes of sound data from `D` |
//! | `+0x0C` | 4 | `nsounds` |
//! | `+0x10` | 4 | `ngroups` |
//! | `+0x14` | 4 | count of 24-byte records after the groups (0 in every bank) |
//! | `+0x18` | 4 | `nsamples` |
//! | `+0x1C` | 32 | bank name (NUL-terminated) |
//! | `D` | 36 × `nsounds` | sounds |
//! | | 72 × `ngroups` | random groups |
//! | | 24 × `+0x14` | unused |
//! | | | layer blocks, one per layer of each sound in order: a 12-byte header then 72 bytes per variant |
//! | `T = align4(D + size)` | 12 × `nsamples` | sample table `{snr offset, seek offset, hash}` |
//! | `S = align4(T + 12 nsamples)` | | sample data: EA SNR header + EA-XMA blocks; the seek info (20 bytes) follows each sample |
//!
//! Sound (36 bytes): `+4` u16 id, `+6` bank slot (runtime), `+7` layer
//! count, `+8` gain, `+0xC` pitch minimum, `+0x10` pitch range, `+0x14` and
//! `+0x18` (not read by the runtime; `+0x18` is a length in ms), `+0x20`
//! layer pointer (runtime).
//!
//! Group (72 bytes): `+0` chooser state, `+4` 32 × u16 sound indices,
//! `+0x44` count, `+0x45` chooser mode.
//!
//! Layer header (12 bytes): `+0` variant pointer (runtime), `+4` chooser
//! state, `+8` variant count, `+9` chooser mode.
//!
//! Variant (72 bytes), with the routine that reads each field:
//!
//! | at | field | use |
//! |---|---|---|
//! | `+0x00` | u16 sample index | `0x82976360` |
//! | `+0x02` | bank slot (runtime) | `0x82976360` |
//! | `+0x03` | effect bus (255 = none) | manager vtable slot 1 (returns 0 in Skate 3: no effect) |
//! | `+0x04` | gain | Gain = gain × gain random × P0 × envelope |
//! | `+0x08` | pitch | Resample = (pitch + rnd × pitch range) × P1 |
//! | `+0x0C` | time-stretch ratio | TimeStretch = clamp(ratio × P5, 0.5, 2) |
//! | `+0x10` | pan offset (degrees; -127 = no Pan2D1) | Pan = P2 + P4 × offset |
//! | `+0x14` | start delay (s) | |
//! | `+0x18` | start time (s) | PLAY1 start offset, envelope clock start |
//! | `+0x1C` | end time (s) | the layer stops at `pitch / actual pitch × end + 0.16` |
//! | `+0x20` | fade-in end (s, 0 = none) | |
//! | `+0x24` | fade-out start (s, 0 = none) | |
//! | `+0x28` | u8 curve: low nibble fade-in, high nibble fade-out (the game uses the low nibble for both) | |
//! | `+0x2C` | gain ratio range | gain random in `[r, 1]` ∪ `[1, 1/r]` |
//! | `+0x30` | pitch random range (added) | |
//! | `+0x34` | delay random range (added) | |
//! | `+0x3C` | u8 priority (voice stealing) | |
//! | `+0x40` | probability | |
//! | `+0x44` | u8 stream flag (Route instead of Resample/Pan/Send) | |
//!
//! # Runtime
//!
//! The game addresses a sound by bank slot and id: ids below `nsounds` are
//! sounds, the next `ngroups` ids are groups (one sound picked by the
//! group's chooser), larger ids clamp to the last sound (`0x82975700`).
//! Creating an instance picks a variant per layer (chooser + probability,
//! `0x829757D0`); playing draws the sound pitch random and each layer's gain,
//! pitch and delay randoms (`0x82975A60`, `0x82975CC8`); layers without a
//! delay are queued for the voice manager, which builds an EA Audio Core
//! graph per layer once per frame (`0x82975290` → `0x82976360`):
//!
//! SndPlayer1 → Resample → [TimeStretch] → Gain → [Pan2D1] → Send (to the
//! caller's submix)
//!
//! Every update (`0x82975B08`, `0x82976860`) posts changed node values. The
//! random numbers come from the C runtime's `rand()` (`0x82F4EAF0`,
//! per-thread seed, shared with the rest of the game thread).
//!
//! Everything here is a translation of those routines; each function names
//! the address it reproduces.

use crate::be;
use crate::ops::{fctiwz, fmadds, fmsubs, fnmsubs};

/// `0x8231BB94`: `rand()` to `[0, 1)`.
const RAND_SCALE: f32 = 1.0 / 32768.0;
/// `0x82098E40`: margin added to a layer's end time.
const END_MARGIN: f32 = 0.16;
/// `0x8209975C` / `0x82060C50`: TimeStretch clamp.
const STRETCH_MIN: f32 = 0.5;
const STRETCH_MAX: f32 = 2.0;
/// `0x822F8EEC`: a pan offset of this value means "no Pan2D1 node".
pub const NO_PAN: f32 = -127.0;
/// `0x82163F98`: the curves' quarter turn (just under π/2).
const QUARTER: f32 = f32::from_bits(0x3FC9_0FD0);
/// Voice manager limits (`0x82975668`, `0x82975290`).
pub const QUEUE_MAX: usize = 40;
pub const ACTIVE_MAX: usize = 60;

pub const MAGIC: &[u8; 4] = b"SPLC";
const DATA: usize = 0x3C;
const SOUND: usize = 36;
const GROUP: usize = 72;
const VARIANT: usize = 72;
const LAYER_HEADER: usize = 12;

fn f32_at(d: &[u8], o: usize) -> f32 {
    f32::from_bits(be::u32(d, o))
}

fn align4(x: usize) -> usize {
    (x + 3) & !3
}

/// A variant chooser's data (`0x82976DD8` arguments).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chooser {
    pub count: u8,
    /// 0 random, 1 sequential, 2 shuffle by halves (no repeats).
    pub mode: u8,
    /// Initial state from the file (`0x00010000` everywhere).
    pub state: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub sample: u16,
    pub effect: u8,
    pub gain: f32,
    pub pitch: f32,
    pub stretch: f32,
    pub pan: f32,
    pub delay: f32,
    pub start: f32,
    pub end: f32,
    pub fade_in_end: f32,
    pub fade_out_start: f32,
    pub curve: u8,
    pub gain_ratio: f32,
    pub pitch_range: f32,
    pub delay_range: f32,
    pub priority: i8,
    pub probability: f32,
    pub stream: u8,
}

impl Variant {
    fn parse(d: &[u8], o: usize) -> Variant {
        Variant {
            sample: be::u16(d, o),
            effect: d[o + 3],
            gain: f32_at(d, o + 0x04),
            pitch: f32_at(d, o + 0x08),
            stretch: f32_at(d, o + 0x0C),
            pan: f32_at(d, o + 0x10),
            delay: f32_at(d, o + 0x14),
            start: f32_at(d, o + 0x18),
            end: f32_at(d, o + 0x1C),
            fade_in_end: f32_at(d, o + 0x20),
            fade_out_start: f32_at(d, o + 0x24),
            curve: d[o + 0x28],
            gain_ratio: f32_at(d, o + 0x2C),
            pitch_range: f32_at(d, o + 0x30),
            delay_range: f32_at(d, o + 0x34),
            priority: d[o + 0x3C] as i8,
            probability: f32_at(d, o + 0x40),
            stream: d[o + 0x44],
        }
    }

    /// The graph `0x82976020` builds: (TimeStretch, Pan2D1) present.
    pub fn nodes(&self, stretch_flag: u8) -> (bool, bool) {
        if self.stream != 0 {
            return (false, false);
        }
        (stretch_flag != 0 || self.stretch != 1.0, self.pan != NO_PAN)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub chooser: Chooser,
    pub variants: Vec<Variant>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub id: u16,
    pub gain: f32,
    pub pitch_min: f32,
    pub pitch_range: f32,
    pub unknown14: f32,
    pub length_ms: f32,
    pub layers: Vec<Layer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub chooser: Chooser,
    /// The first `chooser.count` entries of the 32-entry list.
    pub sounds: Vec<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleEntry {
    /// SNR header offset from the sample data base.
    pub snr: u32,
    /// Seek info offset (PLAY1 attribute 5, sent only with a start offset).
    pub seek: u32,
    pub hash: u32,
}

/// What an id resolves to (`0x82975700`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Sound(usize),
    Group(usize),
    /// Out of range: the game plays the last sound.
    Clamped(usize),
}

#[derive(Clone, Debug)]
pub struct Bank {
    pub version: u32,
    pub name: String,
    pub sounds: Vec<Sound>,
    pub groups: Vec<Group>,
    pub samples: Vec<SampleEntry>,
    /// File offsets of the sample table and the sample data.
    pub table_at: usize,
    pub data_at: usize,
    pub raw: Vec<u8>,
}

impl Bank {
    pub fn parse(raw: Vec<u8>) -> Result<Bank, String> {
        let d = &raw;
        if d.len() < DATA || &d[0..4] != MAGIC {
            return Err("not an SPLC bank".into());
        }
        let version = be::u32(d, 4);
        let size = be::u32(d, 8) as usize;
        let nsounds = be::u32(d, 12) as usize;
        let ngroups = be::u32(d, 16) as usize;
        let nextra = be::u32(d, 20) as usize;
        let nsamples = be::u32(d, 24) as usize;
        let name_end = d[0x1C..DATA].iter().position(|&b| b == 0).unwrap_or(32);
        let name = String::from_utf8_lossy(&d[0x1C..0x1C + name_end]).into_owned();
        let end = DATA + size;
        let table_at = align4(end);
        let data_at = align4(table_at + 12 * nsamples);
        if data_at > d.len() {
            return Err(format!("sample table past the end ({data_at:#x} > {:#x})", d.len()));
        }
        let groups_at = DATA + SOUND * nsounds;
        let mut layer_at = groups_at + GROUP * ngroups + 24 * nextra;
        let mut sounds = Vec::with_capacity(nsounds);
        for i in 0..nsounds {
            let o = DATA + SOUND * i;
            let nlayers = d[o + 7] as usize;
            let mut layers = Vec::with_capacity(nlayers);
            for _ in 0..nlayers {
                if layer_at + LAYER_HEADER > end {
                    return Err(format!("sound {i}: layer block past the data ({layer_at:#x})"));
                }
                let chooser = Chooser { count: d[layer_at + 8], mode: d[layer_at + 9], state: be::u32(d, layer_at + 4) };
                let first = layer_at + LAYER_HEADER;
                let n = chooser.count as usize;
                if first + VARIANT * n > end {
                    return Err(format!("sound {i}: variants past the data ({first:#x})"));
                }
                let variants = (0..n).map(|k| Variant::parse(d, first + VARIANT * k)).collect();
                layers.push(Layer { chooser, variants });
                layer_at = first + VARIANT * n;
            }
            sounds.push(Sound {
                id: be::u16(d, o + 4),
                gain: f32_at(d, o + 8),
                pitch_min: f32_at(d, o + 0xC),
                pitch_range: f32_at(d, o + 0x10),
                unknown14: f32_at(d, o + 0x14),
                length_ms: f32_at(d, o + 0x18),
                layers,
            });
        }
        if layer_at != end {
            return Err(format!("layer blocks end at {layer_at:#x}, data at {end:#x}"));
        }
        let groups = (0..ngroups)
            .map(|g| {
                let o = groups_at + GROUP * g;
                let chooser = Chooser { count: d[o + 0x44], mode: d[o + 0x45], state: be::u32(d, o) };
                let sounds = (0..(chooser.count as usize).min(32)).map(|k| be::u16(d, o + 4 + 2 * k)).collect();
                Group { chooser, sounds }
            })
            .collect();
        let samples = (0..nsamples)
            .map(|k| {
                let o = table_at + 12 * k;
                SampleEntry { snr: be::u32(d, o), seek: be::u32(d, o + 4), hash: be::u32(d, o + 8) }
            })
            .collect();
        Ok(Bank { version, name, sounds, groups, samples, table_at, data_at, raw })
    }

    /// `0x82975700`: what the game plays for `id`.
    pub fn resolve(&self, id: i32) -> Id {
        let n = self.sounds.len() as i32;
        if id < n {
            Id::Sound(id as usize)
        } else if id < n + self.groups.len() as i32 {
            Id::Group((id - n) as usize)
        } else {
            Id::Clamped((n - 1) as usize)
        }
    }

    #[cfg(test)]
    fn snr(&self, i: usize) -> crate::eac::snr::Snr {
        crate::eac::snr::Snr::parse(&self.raw[self.data_at + self.samples[i].snr as usize..])
    }

    /// Fresh chooser states (they live in the loaded bank and persist).
    pub fn states(&self) -> BankState {
        BankState {
            groups: self.groups.iter().map(|g| g.chooser.state).collect(),
            layers: self.sounds.iter().map(|s| s.layers.iter().map(|l| l.chooser.state).collect()).collect(),
        }
    }
}

/// The chooser states a loaded bank carries (`group +0`, `layer header +4`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BankState {
    pub groups: Vec<u32>,
    pub layers: Vec<Vec<u32>>,
}

/// `0x82F4EAF0`: the C runtime's `rand()` (per-thread seed at `ptd + 0x14`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrtRand {
    pub seed: u32,
}

impl CrtRand {
    pub fn next(&mut self) -> i32 {
        self.seed = self.seed.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.seed >> 16) & 0x7FFF) as i32
    }
}

/// `rand() * 2^-15` in single precision (`fcfid`, `frsp`, `fmuls`).
fn unit(r: i32) -> f32 {
    r as f32 * RAND_SCALE
}

/// `0x82976DD8`: pick an index in `[0, count)`.
pub fn choose(count: u8, mode: u8, state: &mut u32, rand: &mut impl FnMut() -> i32) -> u8 {
    let n = count as u32;
    if count == 1 {
        return 0;
    }
    match mode {
        0 => fctiwz(unit(rand()) * n as f32) as u8,
        2 => {
            // The items are two halves, [0, n/2) and [n/2, n) (the odd one
            // goes to the second); the high half-word of the state masks the
            // current half's unplayed items, bit 0 says which half.
            let mask = *state >> 16;
            let half = *state & 1;
            let size = n / 2 + (half & n & 1);
            let r = fctiwz(unit(rand()) * (size + 1) as f32) as u32;
            if size == 0 {
                return 0;
            }
            for i in 0..size {
                let k = i.wrapping_add(r) % size;
                let bit = 1u32 << k;
                if bit & mask != 0 {
                    let pick = if half != 0 { n / 2 } else { 0 } + k;
                    let mut left = mask & !bit;
                    let mut half = half;
                    if left == 0 {
                        // Switch halves and refill: pow(2, m) - 1.
                        half = (half == 0) as u32;
                        let m = n / 2 + (half & n & 1);
                        left = (2f64.powf(m as f32 as f64) as f32 as i64 - 1) as u32;
                    }
                    *state = (left << 16) | (half != 0) as u32;
                    return pick as u8;
                }
            }
            0
        }
        _ => {
            // divw: the state is a signed word.
            let next = (*state as i32).wrapping_add(1);
            // A zero divisor traps (logged only) and divw gives 0.
            let r = if n == 0 { next } else { next.wrapping_rem(n as i32) };
            *state = r as u32;
            r as u8
        }
    }
}

/// `0x82473930`: the game's vector cosine (range reduction by 2π, then the
/// Taylor series to x²²), one lane, with the recomp's VMX rounding (`vmaddfp`
/// is a product rounded, then a sum rounded) and flush-to-zero.
pub fn vcos(x: f32) -> f32 {
    fn ftz(v: f32) -> f32 {
        if v.is_subnormal() { 0.0f32.copysign(v) } else { v }
    }
    let mul = |a: f32, b: f32| ftz(ftz(a) * ftz(b));
    let madd = |a: f32, b: f32, c: f32| ftz(mul(a, b) + ftz(c));
    const C: [u32; 12] = [
        0x3F80_0000, 0xBF00_0000, 0x3D2A_AAAB, 0xBAB6_0B61, 0x37D0_0D01, 0xB493_F27E, 0x310F_76C8, 0xAD49_CBA5,
        0x2957_3F9F, 0xA534_13C3, 0x20F2_A15D, 0x9C86_71CB,
    ];
    let c = |i: usize| f32::from_bits(C[i]);
    let two_pi = f32::from_bits(0x40C9_0FDB);
    let inv_two_pi = f32::from_bits(0x3E22_F983);
    let n = mul(x, inv_two_pi).round_ties_even();
    let r = ftz(ftz(x) - mul(two_pi, n));
    let z = mul(r, r);
    let z2 = mul(z, z);
    let mut v8 = madd(c(1), z, c(0));
    let z3 = mul(z2, z);
    v8 = madd(c(2), z2, v8);
    let z4 = mul(z2, z2);
    let z5 = mul(z3, z2);
    let mut s = madd(c(3), z3, v8);
    let z6 = mul(z3, z3);
    let z7 = mul(z4, z3);
    let z8 = mul(z4, z4);
    let z9 = mul(z5, z4);
    s = madd(c(4), z4, s);
    let z11 = mul(z6, z5);
    let z10 = mul(z5, z5);
    s = madd(c(5), z5, s);
    s = madd(c(6), z6, s);
    s = madd(c(7), z7, s);
    s = madd(c(8), z8, s);
    s = madd(c(9), z9, s);
    s = madd(c(10), z10, s);
    madd(c(11), z11, s)
}

/// `0x82976FF0`: fade curve `kind` at `x` (clamped to `[0, 1]`).
pub fn curve(kind: u8, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    match kind {
        0 => 1.0 - vcos(x * QUARTER),
        1 => {
            let s = 1.0 - vcos(x * QUARTER);
            s * s
        }
        3 => vcos((x - 1.0) * QUARTER),
        4 => {
            let s = vcos((x - 1.0) * QUARTER);
            s * s
        }
        _ => x,
    }
}

/// `0x82976CF0`: the variant's fade at clock `t`, applied to `gain`.
pub fn envelope(v: &Variant, t: f32, gain: f32) -> f32 {
    let kind = v.curve & 0x0F;
    if v.fade_in_end != 0.0 && t < v.fade_in_end {
        let x = (t - v.start) / (v.fade_in_end - v.start);
        return curve(kind, x) * gain;
    }
    if v.fade_out_start != 0.0 && t > v.fade_out_start {
        let x = 1.0 - (t - v.fade_out_start) / (v.end - v.fade_out_start);
        return curve(kind, x) * gain;
    }
    gain
}

/// The six floats every play and update passes (`P0..P5`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Linear gain (the update multiplies it by the sound's gain).
    pub gain: f32,
    /// Pitch ratio (the update multiplies it by the sound's pitch random).
    pub pitch: f32,
    /// Pan angle (degrees) for Pan2D1.
    pub pan: f32,
    /// Seconds since the last update: delay countdown and the layer clock.
    pub dt: f32,
    /// Scale of the variant's pan offset.
    pub pan_scale: f32,
    /// TimeStretch scale.
    pub stretch: f32,
}

impl Params {
    /// What most callers pass to play: gain 0, pitch 1, pan 0, dt, 1, 1.
    pub fn at_play(dt: f32) -> Params {
        Params { gain: 0.0, pitch: 1.0, pan: 0.0, dt, pan_scale: 1.0, stretch: 1.0 }
    }
}

/// Layer states (`+0x4C`).
pub mod state {
    pub const NEW: u32 = 0;
    pub const DELAYED: u32 = 1;
    pub const QUEUED: u32 = 2;
    pub const PLAYING: u32 = 3;
    pub const STOPPED: u32 = 4;
    pub const DROPPED: u32 = 5;
}

/// One layer of a playing sound (the 0x64-byte object, vtable `0x82317184`).
#[derive(Clone, Debug, PartialEq)]
pub struct LayerVoice {
    pub layer: usize,
    pub variant: usize,
    pub state: u32,
    /// `+0x40` pitch (variant pitch + random), `+0x44` gain random,
    /// `+0x48` the pitch the clock advances by, `+0x50` delay left,
    /// `+0x54` clock (s).
    pub pitch: f32,
    pub gain_rand: f32,
    pub rate: f32,
    pub delay: f32,
    pub clock: f32,
    /// Last values posted: Resample `+0x28`, TimeStretch `+0x2C`, Gain
    /// `+0x30`, Pan `+0x34`, Send `+0x3C` (`-1` before the first post).
    pub posted_pitch: f32,
    pub posted_stretch: f32,
    pub posted_gain: f32,
    pub posted_pan: f32,
    pub posted_send: f32,
    /// Graph nodes present (`0x82976020`).
    pub has_stretch: bool,
    pub has_pan: bool,
}

/// A sound instance (the 0x5C-byte object `0x82975700` fills).
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub slot: usize,
    pub sound: usize,
    /// `+0x00`: the play flag (forces a TimeStretch node).
    pub flag: u8,
    /// `+0x58`: the sound's pitch random, `pitch_min + rnd × pitch_range`.
    pub pitch_rand: f32,
    /// `+0x04..`: one entry per sound layer (`None` = skipped or failed).
    pub layers: Vec<Option<LayerVoice>>,
}

/// What a layer does to its EA Audio Core graph.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    /// SndPlayer1 PLAY1 of the variant's sample, from `offset` seconds
    /// (`(pitch / stretch) × start`, with the seek info) when the start time
    /// is set.
    Play { layer: usize, sample: u16, offset: Option<f64> },
    Pitch { layer: usize, value: f32 },
    Stretch { layer: usize, value: f32 },
    Gain { layer: usize, value: f32 },
    Pan { layer: usize, value: f32 },
    Send { layer: usize, value: f32 },
    /// SndPlayer1 STOP (event 1) and the graph's release (`0x82975F40`:
    /// the layer also leaves the manager, `Manager::remove`).
    Stop { layer: usize },
    /// The layer is handed to the voice manager (`0x82975668`).
    Queue { layer: usize, priority: i8, params: Params },
}

/// `0x82975700` + `0x829757D0`: resolve `id` and prepare an instance.
/// Returns `None` when the bank has no sounds.
pub fn create(bank: &Bank, st: &mut BankState, slot: usize, id: i32, rand: &mut impl FnMut() -> i32) -> Option<Instance> {
    if bank.sounds.is_empty() {
        return None;
    }
    let sound = match bank.resolve(id) {
        Id::Sound(s) | Id::Clamped(s) => s,
        Id::Group(g) => {
            let grp = &bank.groups[g];
            let c = choose(grp.chooser.count, grp.chooser.mode, &mut st.groups[g], rand);
            // `lhzx` of list entry `c` (no range check in the game).
            let o = DATA + SOUND * bank.sounds.len() + GROUP * g + 4 + 2 * c as usize;
            be::u16(&bank.raw, o) as usize
        }
    };
    let snd = &bank.sounds[sound];
    let mut layers = vec![None; snd.layers.len()];
    // The block pointer only advances when a layer passes its probability
    // test: a skipped layer makes the next slot draw from the same block.
    let mut block = 0usize;
    for slot_i in 0..snd.layers.len() {
        let l = &snd.layers[block];
        let c = choose(l.chooser.count, l.chooser.mode, &mut st.layers[sound][block], rand) as usize;
        let v = &l.variants[c.min(l.variants.len().saturating_sub(1))];
        if unit(rand()) > v.probability {
            continue;
        }
        layers[slot_i] = Some(LayerVoice {
            layer: block,
            variant: c,
            state: state::NEW,
            pitch: 0.0,
            gain_rand: 0.0,
            rate: 0.0,
            delay: -1.0,
            clock: 0.0,
            posted_pitch: -1.0,
            posted_stretch: -1.0,
            posted_gain: -1.0,
            posted_pan: -1.0,
            posted_send: -1.0,
            has_stretch: false,
            has_pan: false,
        });
        block += 1;
    }
    Some(Instance { slot, sound, flag: 0, pitch_rand: 0.0, layers })
}

impl Instance {
    fn variant<'a>(&self, bank: &'a Bank, lv: &LayerVoice) -> &'a Variant {
        &bank.sounds[self.sound].layers[lv.layer].variants[lv.variant]
    }

    /// `0x82975A60`: play. Layers with no delay are queued with `p`.
    pub fn play(&mut self, bank: &Bank, flag: u8, p: Params, rand: &mut impl FnMut() -> i32) -> Vec<Cmd> {
        let snd = &bank.sounds[self.sound];
        self.pitch_rand = fmadds(unit(rand()), snd.pitch_range, snd.pitch_min);
        self.flag = flag;
        let mut out = Vec::new();
        for i in 0..self.layers.len() {
            let Some(lv) = self.layers[i].as_ref() else { continue };
            let v = self.variant(bank, lv).clone();
            let lv = self.layers[i].as_mut().unwrap();
            // 0x82975CC8
            lv.state = state::DELAYED;
            let u = fmsubs(unit(rand()), 2.0, 1.0);
            let f12 = 1.0 - v.gain_ratio;
            lv.gain_rand = if u > 0.0 {
                if v.gain_ratio == 0.0 {
                    4.0
                } else {
                    let r = 1.0 - f12;
                    fmadds(1.0 / r - 1.0, u, 1.0)
                }
            } else {
                fnmsubs(-u, f12, 1.0)
            };
            lv.pitch = fmadds(unit(rand()), v.pitch_range, v.pitch);
            lv.rate = lv.pitch;
            lv.delay = if v.delay == 0.0 { 0.0 } else { v.delay };
            if v.delay_range != 0.0 {
                lv.delay = fmadds(unit(rand()), v.delay_range, lv.delay);
            }
            if lv.delay == 0.0 {
                lv.state = state::QUEUED;
                out.push(Cmd::Queue { layer: i, priority: v.priority, params: p });
            }
        }
        out
    }

    /// `0x82976360` (run by the voice manager): build the layer's graph and
    /// start it with the queued parameters.
    pub fn start(&mut self, bank: &Bank, i: usize, p: Params) -> Vec<Cmd> {
        let flag = self.flag;
        let Some(lv) = self.layers[i].as_ref() else { return Vec::new() };
        let v = self.variant(bank, lv).clone();
        let lv = self.layers[i].as_mut().unwrap();
        let mut out = Vec::new();
        lv.clock = v.start;
        lv.state = state::PLAYING;
        let (stretch, pan) = v.nodes(flag);
        lv.has_stretch = stretch;
        lv.has_pan = pan;
        let offset = if v.start != 0.0 { Some((v.pitch / v.stretch * v.start) as f64) } else { None };
        out.push(Cmd::Play { layer: i, sample: v.sample, offset });
        if v.stream == 0 {
            let pitch = lv.pitch * p.pitch;
            out.push(Cmd::Pitch { layer: i, value: pitch });
            lv.posted_pitch = pitch;
            out.push(Cmd::Send { layer: i, value: 1.0 });
            lv.posted_send = 1.0;
        }
        let gain = envelope(&v, lv.clock, lv.gain_rand * v.gain * p.gain);
        out.push(Cmd::Gain { layer: i, value: gain });
        lv.posted_gain = gain;
        if pan {
            let a = fmadds(p.pan_scale, v.pan, p.pan);
            out.push(Cmd::Pan { layer: i, value: a });
            lv.posted_pan = a;
        }
        if stretch {
            let s = (p.stretch * v.stretch).clamp(STRETCH_MIN, STRETCH_MAX);
            out.push(Cmd::Stretch { layer: i, value: s });
            lv.posted_stretch = s;
        }
        out
    }

    /// `0x82975B08`: per-frame update. `finished(i)` answers whether layer
    /// `i`'s graph reports done (graph `+0x47 == 2`). Layers that reach
    /// STOPPED or DROPPED are destroyed (`None`).
    pub fn update(&mut self, bank: &Bank, mut p: Params, finished: &mut impl FnMut(usize) -> bool) -> Vec<Cmd> {
        p.gain *= bank.sounds[self.sound].gain;
        p.pitch *= self.pitch_rand;
        let mut out = Vec::new();
        for i in 0..self.layers.len() {
            if self.layers[i].is_none() {
                continue;
            }
            self.update_layer(bank, i, p, finished, &mut out);
            if self.layers[i].as_ref().unwrap().state >= state::STOPPED {
                self.layers[i] = None;
            }
        }
        out
    }

    /// One layer's per-update step (`0x82976860`).
    fn update_layer(&mut self, bank: &Bank, i: usize, p: Params, finished: &mut impl FnMut(usize) -> bool, out: &mut Vec<Cmd>) {
        let v = self.variant(bank, self.layers[i].as_ref().unwrap()).clone();
        let lv = self.layers[i].as_mut().unwrap();
        if lv.delay > 0.0 {
            lv.delay -= p.dt;
            if lv.delay > 0.0 {
                return;
            }
            lv.state = state::QUEUED;
            out.push(Cmd::Queue { layer: i, priority: v.priority, params: p });
            return;
        }
        if lv.state != state::PLAYING {
            return;
        }
        let clock = fmadds(lv.rate, p.dt, lv.clock);
        lv.rate = p.pitch;
        let gain = v.gain * lv.gain_rand * p.gain;
        let pitch = lv.pitch * p.pitch;
        let pan = fmadds(v.pan, p.pan_scale, p.pan);
        let stretch = v.stretch * p.stretch;
        lv.clock = clock;
        let gain = envelope(&v, lv.clock, gain);
        let end = fmadds(v.pitch / lv.pitch, v.end, END_MARGIN);
        if end < lv.clock || finished(i) {
            if v.stream == 0 {
                out.push(Cmd::Pitch { layer: i, value: 0.0 });
            }
            out.push(Cmd::Gain { layer: i, value: 0.0 });
            out.push(Cmd::Stop { layer: i });
            lv.state = state::STOPPED;
            return;
        }
        if lv.has_pan && lv.posted_pan != pan {
            lv.posted_pan = pan;
            out.push(Cmd::Pan { layer: i, value: pan });
        }
        if lv.has_stretch {
            let s = stretch.clamp(STRETCH_MIN, STRETCH_MAX);
            lv.posted_stretch = s;
            out.push(Cmd::Stretch { layer: i, value: s });
        }
        if v.stream == 0 && lv.posted_pitch != pitch {
            lv.posted_pitch = pitch;
            out.push(Cmd::Pitch { layer: i, value: pitch });
        }
        if lv.posted_gain != gain {
            lv.posted_gain = gain;
            out.push(Cmd::Gain { layer: i, value: gain });
        }
    }

    /// The manager refused (queue full) or stole layer `i`: `0x82975F40`
    /// with state DROPPED; the next update destroys it.
    pub fn drop_layer(&mut self, i: usize) {
        if let Some(lv) = self.layers[i].as_mut() {
            lv.state = state::DROPPED;
        }
    }

    /// `0x82975BF0`: any layer below STOPPED.
    pub fn is_playing(&self) -> bool {
        self.layers.iter().flatten().any(|l| l.state < state::STOPPED)
    }
}

/// One entry of the manager's queue or active table (36 bytes:
/// `{instance, layer, priority, params}`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry<H: Copy> {
    pub handle: H,
    pub priority: i8,
    pub params: Params,
}

/// The voice manager (`0x83084348`, 0x1098 bytes, built by `0x82974BD0`):
/// a queue of 40 layer starts (`+0x8`, count `+0xE18`) and a table of 60
/// active layers (`+0x5A8`, count `+0xE1C`, a free entry has priority -1).
#[derive(Clone, Debug)]
pub struct Manager<H: Copy + PartialEq> {
    pub queue: Vec<Entry<H>>,
    pub active: Vec<Option<Entry<H>>>,
}

/// What the manager decided in one service.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision<H> {
    /// Start the layer (`0x82976360`) with its queued parameters.
    Start(H, Params),
    /// Stop a playing layer to make room (state DROPPED, graph released).
    Steal(H),
    /// The layer never starts (state DROPPED).
    Drop(H),
}

impl<H: Copy + PartialEq> Default for Manager<H> {
    fn default() -> Self {
        Manager { queue: Vec::new(), active: vec![None; ACTIVE_MAX] }
    }
}

impl<H: Copy + PartialEq> Manager<H> {
    /// `0x82975668`: false (and the layer is dropped) when the queue is full.
    pub fn enqueue(&mut self, e: Entry<H>) -> bool {
        if self.queue.len() >= QUEUE_MAX {
            return false;
        }
        self.queue.push(e);
        true
    }

    /// `0x82975498`: forget a layer (active entry or queued start).
    pub fn remove(&mut self, h: H) {
        if let Some(a) = self.active.iter_mut().find(|a| a.map(|e| e.handle) == Some(h)) {
            *a = None;
            return;
        }
        if let Some(k) = self.queue.iter().position(|e| e.handle == h) {
            self.queue.remove(k);
        }
    }

    fn active_count(&self) -> usize {
        self.active.iter().flatten().count()
    }

    /// `0x829755B8`: start and take the first free active entry at or after
    /// `from` (none free: the voice plays untracked).
    fn start(&mut self, e: Entry<H>, from: usize, out: &mut Vec<Decision<H>>) {
        out.push(Decision::Start(e.handle, e.params));
        if let Some(a) = self.active[from.min(ACTIVE_MAX)..].iter_mut().find(|a| a.is_none()) {
            *a = Some(e);
        }
    }

    /// `0x82975290`: the once-per-frame service (`0x82485190`).
    pub fn service(&mut self) -> Vec<Decision<H>> {
        let mut out = Vec::new();
        let count = self.queue.len();
        if count == 0 {
            return out;
        }
        let mut queue = std::mem::take(&mut self.queue);
        if count + self.active_count() <= ACTIVE_MAX {
            for e in queue {
                self.start(e, 0, &mut out);
            }
            return out;
        }
        sort_entries(&mut queue);
        let free = ACTIVE_MAX - self.active_count();
        let mut next = 0;
        for e in queue.iter().take(free) {
            self.start(*e, 0, &mut out);
            next += 1;
        }
        // Sort the active table too (free entries count as priority -1).
        sort_slots(&mut self.active);
        let mut low = ACTIVE_MAX - 1;
        while next < count && low > 0 {
            let lowest = self.active[low].map(|e| e.priority).unwrap_or(-1);
            if lowest >= queue[next].priority {
                break;
            }
            if let Some(victim) = self.active[low].take() {
                out.push(Decision::Steal(victim.handle));
            }
            self.start(queue[next], low, &mut out);
            next += 1;
            low -= 1;
        }
        for e in &queue[next..] {
            out.push(Decision::Drop(e.handle));
        }
        out
    }
}

/// `0x82975090` / `0x829750F8`: the game's quicksort, descending by
/// priority (pivot = first element, swapped to the end; Lomuto partition).
fn sort_entries<H: Copy>(v: &mut [Entry<H>]) {
    let mut keyed: Vec<(i8, Entry<H>)> = v.iter().map(|e| (e.priority, *e)).collect();
    quicksort(&mut keyed, 0, v.len() as i32 - 1);
    for (d, (_, e)) in v.iter_mut().zip(keyed) {
        *d = e;
    }
}

fn sort_slots<H: Copy>(v: &mut [Option<Entry<H>>]) {
    let mut keyed: Vec<(i8, Option<Entry<H>>)> = v.iter().map(|e| (e.map(|e| e.priority).unwrap_or(-1), *e)).collect();
    quicksort(&mut keyed, 0, v.len() as i32 - 1);
    for (d, (_, e)) in v.iter_mut().zip(keyed) {
        *d = e;
    }
}

fn quicksort<T: Copy>(v: &mut [(i8, T)], mut lo: i32, hi: i32) {
    while hi > lo {
        let p = partition(v, lo, hi, lo);
        quicksort(v, lo, p - 1);
        lo = p + 1;
    }
}

fn partition<T: Copy>(v: &mut [(i8, T)], lo: i32, hi: i32, pivot: i32) -> i32 {
    let key = v[pivot as usize].0;
    v.swap(pivot as usize, hi as usize);
    let mut store = lo;
    for k in lo..=hi {
        if v[k as usize].0 > key {
            v.swap(store as usize, k as usize);
            store += 1;
        }
    }
    v.swap(store as usize, hi as usize);
    store
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn banks() -> Vec<PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/skate-audio/raw/audiofiles");
        let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
        let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("bnk"))).collect();
        v.sort();
        v
    }

    #[test]
    fn crt_rand_matches_msvc() {
        let mut r = CrtRand { seed: 1 };
        assert_eq!([r.next(), r.next(), r.next()], [41, 18467, 6334]);
    }

    #[test]
    fn shuffle_never_repeats_and_alternates_halves() {
        let mut seed = CrtRand { seed: 7 };
        for n in 2u8..=12 {
            let mut st = 0x0001_0000u32;
            let mut prev = None;
            let mut seen = vec![0u32; n as usize];
            for _ in 0..600 {
                let c = choose(n, 2, &mut st, &mut || seed.next());
                assert!(c < n);
                assert_ne!(Some(c), prev, "n={n}");
                prev = Some(c);
                seen[c as usize] += 1;
            }
            // The file's initial mask holds only item 0 of the first half, so
            // the first pick is always 0; afterwards every item plays.
            assert!(seen.iter().all(|&k| k > 0), "n={n} {seen:?}");
        }
        let mut st = 0x0001_0000u32;
        assert_eq!(choose(4, 2, &mut st, &mut || 12345), 0);
        // Half 0 exhausted: switch to half 1, refill with 2^2 - 1.
        assert_eq!(st, (3 << 16) | 1);
    }

    #[test]
    fn sequential_and_random() {
        let mut st = 0x0001_0000u32;
        // (0x10000 + 1) % 3 = 2
        assert_eq!(choose(3, 1, &mut st, &mut || 0), 2);
        assert_eq!(choose(3, 1, &mut st, &mut || 0), 0);
        assert_eq!(choose(5, 0, &mut st, &mut || 32767), 4);
        assert_eq!(choose(5, 0, &mut st, &mut || 0), 0);
        assert_eq!(choose(1, 0, &mut st, &mut || panic!("no draw")), 0);
    }

    #[test]
    fn cosine_and_curves() {
        for k in 0..=200 {
            let x = -4.0 + k as f32 * 0.04;
            assert!((vcos(x) - x.cos()).abs() < 2e-6, "{x}");
        }
        assert_eq!(curve(2, 0.25), 0.25);
        assert_eq!(curve(0, 0.0), 0.0);
        assert!((curve(0, 1.0) - 1.0).abs() < 1e-5);
        assert!((curve(3, 0.5) - (0.5f32 * std::f32::consts::FRAC_PI_2).sin()).abs() < 1e-5);
        assert_eq!(curve(9, 2.0), 1.0);
    }

    #[test]
    fn manager_drops_when_full_and_equal_priority() {
        let mut m: Manager<u32> = Manager::default();
        let p = Params::at_play(1.0 / 60.0);
        for h in 0..40 {
            assert!(m.enqueue(Entry { handle: h, priority: 0, params: p }));
        }
        assert!(!m.enqueue(Entry { handle: 99, priority: 0, params: p }));
        assert_eq!(m.service().len(), 40);
        for h in 100..140 {
            m.enqueue(Entry { handle: h, priority: 0, params: p });
        }
        let d = m.service();
        assert_eq!(d.iter().filter(|x| matches!(x, Decision::Start(..))).count(), 20);
        assert_eq!(d.iter().filter(|x| matches!(x, Decision::Drop(..))).count(), 20);
        // A higher priority steals the lowest active entry.
        m.enqueue(Entry { handle: 500, priority: 5, params: p });
        let d = m.service();
        assert!(matches!(d[0], Decision::Steal(_)));
        assert_eq!(d[1], Decision::Start(500, p));
    }

    #[test]
    fn parses_every_local_bank() {
        let files = banks();
        if files.is_empty() {
            return;
        }
        let (mut nsounds, mut ngroups, mut nvariants, mut nsamples) = (0, 0, 0, 0);
        for f in &files {
            let bank = Bank::parse(std::fs::read(f).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            assert_eq!(bank.version, 3);
            nsounds += bank.sounds.len();
            ngroups += bank.groups.len();
            nsamples += bank.samples.len();
            for (i, s) in bank.sounds.iter().enumerate() {
                assert_eq!(s.id as usize, i, "{} sound {i}", f.display());
                for l in &s.layers {
                    assert!(l.chooser.count >= 1);
                    assert_eq!(l.chooser.state, 0x0001_0000);
                    for v in &l.variants {
                        nvariants += 1;
                        assert!((v.sample as usize) < bank.samples.len(), "{} sound {i}", f.display());
                        assert!(v.probability > 0.0 && v.probability <= 1.0);
                    }
                }
            }
            for g in &bank.groups {
                assert_eq!(g.chooser.state, 0x0001_0000);
                for &s in &g.sounds {
                    assert!((s as usize) < bank.sounds.len(), "{} group sound {s}", f.display());
                }
            }
            // Every sample is an EA-XMA SNR header; samples are contiguous and
            // each one's seek info lies inside it.
            for (k, e) in bank.samples.iter().enumerate() {
                let snr = bank.snr(k);
                assert_eq!(snr.codec, crate::eac::snr::CODEC_XMA, "{} sample {k}", f.display());
                assert!(snr.channels >= 1 && snr.channels <= 2);
                assert!(e.seek > e.snr);
                let next = bank.samples.get(k + 1).map(|n| n.snr as usize).unwrap_or(bank.raw.len() - bank.data_at);
                assert!((e.seek as usize) < next);
            }
        }
        eprintln!("{} banks: {nsounds} sounds, {ngroups} groups, {nvariants} variants, {nsamples} samples", files.len());
    }

    #[test]
    fn plays_a_local_sound() {
        let Some(f) = banks().into_iter().find(|p| p.file_name().is_some_and(|n| n == "sk8_foley.bnk")) else { return };
        let bank = Bank::parse(std::fs::read(f).unwrap()).unwrap();
        let mut st = bank.states();
        let mut r = CrtRand { seed: 1 };
        let mut rand = || r.next();
        // Id nsounds + 1 is group 1 (seven sounds, shuffled).
        let id = bank.sounds.len() as i32 + 1;
        let mut inst = create(&bank, &mut st, 7, id, &mut rand).unwrap();
        assert!(bank.groups[1].sounds.contains(&(inst.sound as u16)));
        let p = Params::at_play(1.0 / 60.0);
        let cmds = inst.play(&bank, 0, p, &mut rand);
        let snd = &bank.sounds[inst.sound];
        assert!(inst.pitch_rand >= snd.pitch_min && inst.pitch_rand <= snd.pitch_min + snd.pitch_range);
        for c in &cmds {
            let Cmd::Queue { layer, params, .. } = c else { panic!() };
            let start = inst.start(&bank, *layer, *params);
            assert!(matches!(start[0], Cmd::Play { .. }));
        }
        let mut p = Params { gain: 1.0, ..p };
        let mut frames = 0;
        while inst.is_playing() && frames < 600 {
            let out = inst.update(&bank, p, &mut |_| false);
            for c in out {
                if let Cmd::Queue { layer, params, .. } = c {
                    inst.start(&bank, layer, params);
                }
            }
            p.dt = 1.0 / 60.0;
            frames += 1;
        }
        // Every layer ends by its end time (+0.16 s), well under 10 s.
        assert!(!inst.is_playing());
        assert!(frames < 600);
    }
}

//! Skate 3's rolling grains: the board component's *stream mode* (the
//! non-AEMS half of `0x824C6BD8`) and Skate's own "Grain Player"
//! (`0x828EBD88`..`0x828ECF78`, game code that drives EA Audio Core voices),
//! ported from the TU3 image. See the report that came with it for the
//! addresses of every rule.
//!
//! What is here, each exact against the recompiled code:
//! - [`blocks`]: the two 16-byte parameter blocks `{level, pitch, 0,
//!   position}` the board update writes per truck pair `k` into the grain
//!   players `*(comp + 1176 + 8k)` (voice A) and `*(comp + 1180 + 8k)`
//!   (voice B), from skater speed, the surface curve and the evaluator's
//!   packed parameters 1, 2 and 3.
//! - [`GrainPlayer`]: the scheduler. Per audio block (dt = 256/48000 s,
//!   service `0x828EC6F0`) it runs two sub-voices that crossfade: fade in
//!   (`fade_in` s), hold (`hold` s of *source* time: the timer runs at
//!   `dt · pitch`), fade out (`fade_out` s) while the other sub starts the
//!   next grain. A grain also restarts early when the requested position
//!   moves more than `tolerance` away from the position its grain was
//!   started for. Start points (`0x828ECAB0`) are random slots of length
//!   `fade_in + hold + fade_out` inside a `window`-second span of the
//!   recording at `position · (duration − window)`, avoiding the slots
//!   played recently (a 16-entry sorted interval list).
//! - [`Fader`]: EA Audio Core's `GainFader` (`0x82B238A8`) with the
//!   linear-power profile (`0x82B42C98`) the grain player uses for every
//!   fade: `g(i) = g0 + (g1 − g0)·sqrt((i + 1)/n)` rising,
//!   `g1 + (g0 − g1)·sqrt((n − i − 1)/n)` falling, `n = trunc(time · 48000)`,
//!   with the original's VMX square-root estimate (`vrsqrtefp` + one Newton
//!   step) for every group of four and exact `fsqrts` for the tail.
//! - [`GameRng`]: the game's global generator `0x82A8AF10` (six words at
//!   `0x82FD7D74`, shared by ~100 call sites), which picks the slot.
//!
//! The voice each grain plays (`0x828EC3F0`) is an EA Audio Core graph
//! SndPlayer1 → Resample → GainFader → Send: the recording's SNR from
//! `start_sample(rate, t)` (seek table in front of the SNR), pitch = block
//! pitch, the fade, then Send gain = block level, into the truck's submix
//! chain (HighPassIir2 at packed parameter 11 Hz, LowPassIir2 at parameter
//! 12 Hz, FrequencyShiftSsb, Gain).

use crate::ops::{fctiwz, fmadds, fnmsubs, fsel};
use crate::ppc::vmx::{self, V128};

/// One (`0x8231A844`).
const ONE: f32 = 1.0;
/// Zero (`0x82165A10`).
const ZERO: f32 = 0.0;
/// m/s → km/h (`0x822F8628`, 3.5999999).
const KMH: f32 = f32::from_bits(0x4066_6666);
/// `0x822F8898` (1/32767).
const INV_32767: f32 = f32::from_bits(0x3800_0100);
/// `0x822F890C` (1/4096).
const INV_4096: f32 = f32::from_bits(0x3980_0000);
/// Three (`0x82063B08`).
const THREE: f32 = 3.0;
/// Voice B reads 0.1 (normalised) behind voice A (`0x820641A8`).
const B_BEHIND: f32 = f32::from_bits(0x3DCC_CCCD);
/// Minus one (`0x8216DEE0`).
const MINUS_ONE: f32 = -1.0;
/// Minus two (`0x82094178`).
const MINUS_TWO: f32 = -2.0;
/// 2^-31 (`0x822F8AFC`) and 0.5 (`0x8209975C`): the slot pick scale.
const TWO_M31: f32 = f32::from_bits(0x3000_0000);
const HALF: f32 = 0.5;
/// The service period: one EA Audio Core block at 48 kHz (logged dt,
/// 106,034 of 106,034 service calls).
pub const DT: f32 = f32::from_bits(0x3BAE_C33E);
/// The most input samples a grain voice pulls per block at the resampler's
/// 4x pitch cap (plus its history and lookahead).
const MAX_INPUT: usize = 4 * 256 + 16;
/// Mixer rate the fades are timed in.
pub const MIX_RATE: f32 = 48_000.0;

/// `clamp01(speed / top_kmh · 3.6)` in the original's `fsel` form.
fn unit_speed(speed: f32, kmh: f32) -> f32 {
    let f13 = speed / kmh;
    let f8 = f13 * KMH;
    let f6 = fsel(-f8, ZERO, f8);
    let f5 = ONE - f6;
    fsel(f5, f6, ONE)
}

// ---------------------------------------------------------------------------
// The board update's stream-mode blocks (`0x824C6BD8`)
// ---------------------------------------------------------------------------

/// A surface's curve block: the AttribSys layout of its collection
/// (`*(comp + 188 + 16k)` = collection `+36`; class key
/// `0x7AB23C11B6ADA2DE`, collection key from `0x824C8370`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Curve {
    /// Bezier control values `+4, +20, +36, +52` (the x values at `+0,
    /// +16, +32, +48` are not read): `y[0]` at full speed, `y[3]` at rest.
    pub y: [f32; 4],
    /// `+68`: speed (km/h) that maps to the end of the curve.
    pub top_kmh: f32,
    /// `+72`, `+76`: voice B's tilt boost gain and its speed scale (km/h).
    pub boost_gain: f32,
    pub boost_kmh: f32,
    /// `+80`: frequency shift (Hz) per unit tilt on voice A's chain.
    pub shift_a: f32,
    /// `+84`: cap of the slide blend (`comp + 1164`, `0x824C8588`).
    pub blend_cap: f32,
    /// `+88`: voice B's base frequency shift (Hz).
    pub shift_b: f32,
}

impl Curve {
    /// From the layout block's 23 big-endian floats (`+0..+92`).
    pub fn from_floats(f: &[f32]) -> Curve {
        Curve {
            y: [f[1], f[5], f[9], f[13]],
            top_kmh: f[17],
            boost_gain: f[18],
            boost_kmh: f[19],
            shift_a: f[20],
            blend_cap: f[21],
            shift_b: f[22],
        }
    }
}

/// One grain player's parameter block (`player + 0..16`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Block {
    /// Send gain (linear).
    pub level: f32,
    /// Resample ratio.
    pub pitch: f32,
    /// `+8`, always 0.
    pub flags: u32,
    /// Normalised position in the recording (0 = start of the sweep).
    pub position: f32,
}

/// What `0x824C6BD8` reads for one truck pair in stream mode.
#[derive(Clone, Copy, Debug, Default)]
pub struct Inputs {
    /// Skater `+208` (m/s), already times component `+1028` unless byte
    /// `+1032` is set.
    pub speed: f32,
    /// Packed parameter 1 (vtable slot 15, 15 bits): voice A level.
    pub level_a: i32,
    /// Packed parameter 2 (slot 15): voice B level.
    pub level_b: i32,
    /// Packed parameter 3 (slot 14): pitch ratio × 4096 (cents → ratio).
    pub pitch: i32,
    /// Component `+1164`: slide blend (`0x824C8588`, |slewed| ≤ curve cap).
    pub blend: f32,
    /// Component `+1168`: rolling-backwards amount (0..1, ±0.05 a frame).
    pub backwards: f32,
    /// `0x824CA688 || 0x824CA6E0` (latches `+1504`/`+1505`).
    pub latched: bool,
    /// Tuned `0x6BDC44AE7C3C79D0` on the pair's collection (0.65; 0.6 on
    /// metal; 0 when absent).
    pub latch_scale: f32,
    /// Component `+1456` when byte `+1464` is set (area modifier).
    pub area: Option<f32>,
    /// Component `+1508` (tilt one way, 0..1, `0x824CA738`).
    pub tilt: f32,
}

/// The Bezier position (`0x824C6BD8` inline): `u = clamp01(speed·3.6 /
/// top)`, `t = 1 − u`: `u³·y0 + 3·u·t·(y1·u + y2·t) + t³·y3`.
pub fn position(speed: f32, c: &Curve) -> f32 {
    let f4 = unit_speed(speed, c.top_kmh);
    let f3 = ONE - f4;
    let f2 = ONE - f3;
    let f1 = c.y[2] * f3;
    let f0 = f3 * f3;
    let f13 = f2 * f2;
    let f12 = fmadds(c.y[1], f2, f1);
    let f11 = f0 * f3;
    let f8 = f13 * f2;
    let f7 = f12 * f2;
    let f6 = f8 * c.y[0];
    let f5 = f7 * f3;
    let f4 = fmadds(f5, THREE, f6);
    fmadds(f11, c.y[3], f4)
}

/// `0x824C6BD8`, stream mode, pair `k`: blocks for voice A (`+1176 + 8k`)
/// and voice B (`+1180 + 8k`). `own` is pair `k`'s curve, `selected` the
/// curve of pair `comp + 1500` (used by B's tilt boost).
pub fn blocks(i: &Inputs, own: &Curve, selected: &Curve) -> (Block, Block) {
    let pos = position(i.speed, own);
    let f10 = i.level_a as f32 * INV_32767;
    let f13 = i.blend - i.backwards;
    let f11 = fsel(f13, i.blend, i.backwards);
    let f9 = ONE - f11;
    let mut a = f9 * f10;
    if i.latched {
        a *= i.latch_scale;
    }
    if let Some(m) = i.area {
        a *= m;
    }
    let pitch = i.pitch as f32 * INV_4096;
    let block_a = Block { level: a, pitch, flags: 0, position: pos };

    let mut pb = pos - B_BEHIND;
    if pb < ZERO {
        pb = ZERO;
    }
    let f10 = i.level_b as f32 * INV_32767;
    let mut b = i.blend * f10;
    if let Some(m) = i.area {
        b *= m;
    }
    if i.tilt > ZERO {
        let mut f13 = ONE;
        if selected.boost_kmh > ZERO {
            f13 = unit_speed(i.speed, selected.boost_kmh);
        }
        let f13 = i.tilt * f13;
        let f12 = f13 * selected.boost_gain;
        b = fmadds(f12, a, b);
        if b > ONE {
            b = ONE;
        }
    }
    (block_a, Block { level: b, pitch, flags: 0, position: pb })
}

/// `0x824C5CA8`: whether a surface plays grains (stream mode `+1320 + 4k`
/// = 1) rather than AEMS rolling.
pub fn stream_mode(surface: i32) -> bool {
    !(7..=13).contains(&surface) || surface == 9
}

/// SndPlayer1's start offset (`0x82B32DC8`): `fctiwz(f32 rate · f64 t)`.
pub fn start_sample(rate: f32, t: f32) -> i32 {
    let x = rate as f64 * t as f64;
    if x.is_nan() { i32::MIN } else { x as i32 }
}

// ---------------------------------------------------------------------------
// The game's random generator (`0x82A8AF10`)
// ---------------------------------------------------------------------------

/// `0x82A8AF10`: six 32-bit words (`0x82FD7D74`); each call adds the words
/// pairwise with carry from the low end (`s4 += s5`, `s3 += s4`, ...,
/// `s0 += s1`), then increments `s5` (a 192-bit counter carry when it
/// wraps), and returns `s0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameRng {
    pub s: [u32; 6],
}

/// Slot choice for a grain start: `(count, slots) -> index`.
pub type Choose<'a> = dyn FnMut(usize, &[(f32, f32)]) -> i32 + 'a;

impl GameRng {
    pub fn next(&mut self) -> u32 {
        let s = &mut self.s;
        let old5 = s[5];
        let r10 = s[4].wrapping_add(old5);
        let mut c = (r10 < old5) as u32;
        s[4] = r10;
        let mut prev = r10;
        let mut new = [0u32; 4];
        for (n, w) in [3usize, 2, 1].into_iter().enumerate() {
            let v = s[w].wrapping_add(prev).wrapping_add(c);
            c = (v < prev || (v == prev && c != 0)) as u32;
            s[w] = v;
            new[n] = v;
            prev = v;
        }
        let mut r3 = s[0].wrapping_add(prev).wrapping_add(c);
        s[0] = r3;
        s[5] = old5.wrapping_add(1);
        if s[5] == 0 {
            s[4] = r10.wrapping_add(1);
            if s[4] == 0 {
                s[3] = new[0].wrapping_add(1);
                if s[3] == 0 {
                    s[2] = new[1].wrapping_add(1);
                    if s[2] == 0 {
                        s[1] = new[2].wrapping_add(1);
                        if s[1] == 0 {
                            r3 = r3.wrapping_add(1);
                            s[0] = r3;
                        }
                    }
                }
            }
        }
        r3
    }
}

/// The slot index `0x828ECAB0` takes from a random word:
/// `fctiwz(f32(u32) · 2^-31 · 0.5 · f32(count))`. A word within 128 of
/// 2^32 rounds to `count` (one past the slots; the original then reads a
/// cleared slot, start −1.0).
pub fn pick_index(r: u32, count: usize) -> i32 {
    let f11 = r as f32;
    let f9 = f11 * TWO_M31;
    let f7 = f9 * HALF;
    fctiwz(f7 * count as f32)
}

// ---------------------------------------------------------------------------
// The grain player (`0x828EBD88` ..)
// ---------------------------------------------------------------------------

/// The five tuned grain values (`player + 16..32`; AttribSys
/// `0xD18D1174735E5CDE`, index 0 for voice A, 1 for voice B, copied in by
/// `0x824C5CA8` after every recording change).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrainParams {
    pub fade_in: f32,
    pub hold: f32,
    pub fade_out: f32,
    pub window: f32,
    pub tolerance: f32,
}

impl GrainParams {
    /// Constructor values (`0x828EBD88`).
    pub const DEFAULT: GrainParams = GrainParams {
        fade_in: f32::from_bits(0x3C23_D70A),
        hold: 0.5,
        fade_out: f32::from_bits(0x3C23_D70A),
        window: 4.0,
        tolerance: f32::from_bits(0x3D4C_CCCD),
    };
    /// TU3 tuning, every surface collection (heap snapshot): voice A.
    pub const VOICE_A: GrainParams = GrainParams {
        fade_in: f32::from_bits(0x3DCC_CCCD),
        hold: f32::from_bits(0x3E4C_CCCD),
        fade_out: f32::from_bits(0x3DCC_CCCD),
        window: f32::from_bits(0x3FCC_CCCD),
        tolerance: f32::from_bits(0x3D4C_CCCD),
    };
    /// Voice B.
    pub const VOICE_B: GrainParams = GrainParams {
        fade_in: f32::from_bits(0x3E4C_CCCD),
        hold: f32::from_bits(0x3DCC_CCCD),
        fade_out: f32::from_bits(0x3E4C_CCCD),
        window: 1.5,
        tolerance: f32::from_bits(0x3D4C_CCCD),
    };
}

/// One of the two sub-voices (`player + 88 + 28s`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sub {
    /// A voice graph exists (`+4`).
    pub live: bool,
    /// `+8`: seconds left in the current state.
    pub timer: f32,
    /// `+12`: where the grain started in the recording (s, clamped).
    pub start: f32,
    /// `+16`: the requested position (`player + 12`) it was started for.
    pub target: f32,
    /// `+20`: 1 fading in, 2 holding, 3 fading out.
    pub state: u32,
}

impl Sub {
    /// `0x828EBCB0`: destroy the graph and reset.
    fn reset(&mut self) {
        *self = Sub { live: false, timer: ZERO, start: ZERO, target: ZERO, state: 1 };
    }
}

/// A played interval (`player + 172 + 12i`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub start: f32,
    pub end: f32,
    /// `+8` (u16): next index, −1 ends the list.
    pub next: i16,
}

/// What the player asks of its voices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// `0x828EC3F0`: new graph on `sub`, play from `at` seconds (already
    /// clamped to `[0, duration]`), gain set to 0 at once, then a
    /// linear-power fade to 1 over `fade_in`. Send gain = `level`.
    Start { sub: usize, at: f32, fade_in: f32 },
    /// `0x828EC2F8`: linear-power fade to 0 over `time`.
    FadeOut { sub: usize, time: f32 },
    /// `0x828EBCB0`: the graph is destroyed.
    Kill { sub: usize },
}

/// Skate's grain player (372 bytes; four per board component).
#[derive(Clone, Debug)]
pub struct GrainPlayer {
    /// The block written by the board update (`+0..+16`).
    pub level: f32,
    pub pitch: f32,
    pub flags: u32,
    pub position: f32,
    pub params: GrainParams,
    /// `+36`: no new grains (never set by the board code).
    pub hold_off: bool,
    /// `+44`: registered as an audio service.
    pub registered: bool,
    /// `+56`: recording duration (s, from the `.grain` header).
    pub duration: f32,
    pub subs: [Sub; 2],
    /// `+144`: the sub of the newest grain.
    pub current: usize,
    pub spans: [Span; 16],
    /// `+364` list head, `+366` last allocated, `+368` free list head,
    /// `+360` (set to −1).
    pub head: i16,
    pub last: i16,
    pub free: i16,
    pub h360: i16,
}

impl Default for GrainPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl GrainPlayer {
    /// A player with both subs idle (`0x828EBD88`).
    pub fn new() -> GrainPlayer {
        let mut sub = Sub::default();
        sub.reset();
        GrainPlayer {
            level: ONE,
            pitch: ONE,
            flags: 0,
            position: ZERO,
            params: GrainParams::DEFAULT,
            hold_off: false,
            registered: false,
            duration: ZERO,
            subs: [sub; 2],
            current: 0,
            spans: [Span { start: MINUS_ONE, end: MINUS_ONE, next: 0 }; 16],
            head: 0,
            last: 0,
            free: 0,
            h360: 0,
        }
    }

    /// Writes a board block (`0x824C6BD8`: four `stw` into the player).
    pub fn set_block(&mut self, b: &Block) {
        self.level = b.level;
        self.pitch = b.pitch;
        self.flags = b.flags;
        self.position = b.position;
    }

    /// `0x828EBE68`: history reset and service registration.
    fn reset_history(&mut self) {
        self.current = 0;
        self.last = 0;
        self.head = 0;
        self.free = 1;
        for i in 0..16 {
            self.spans[i] = Span { start: MINUS_ONE, end: MINUS_ONE, next: i as i16 + 1 };
        }
        self.spans[0].end = MINUS_ONE;
        self.hold_off = false;
        self.h360 = -1;
        self.spans[0].next = -1;
        self.spans[0].start = MINUS_TWO;
        self.registered = true;
    }

    /// `0x828EC040` (+ the tuned values `0x824C5CA8` copies in): a new
    /// recording. The subs are left alone (the board stops the player
    /// first, `0x828EBF90`).
    pub fn set_recording(&mut self, duration: f32, params: GrainParams) {
        self.reset_history();
        self.duration = duration;
        self.params = params;
    }

    /// `0x828EBF90`: both subs destroyed, service unregistered.
    pub fn stop(&mut self, events: &mut Vec<Event>) {
        if !self.registered {
            return;
        }
        for s in 0..2 {
            self.subs[s].reset();
            events.push(Event::Kill { sub: s });
        }
        self.registered = false;
    }

    /// `0x828ECA08`: tiles `[lo, hi] ∩ [w0, w1]` with slots of length `l`.
    fn tile(range: &mut (f32, f32), l: f32, w0: f32, w1: f32, out: &mut [(f32, f32); 64], count: &mut usize) {
        if range.1 < w0 {
            return;
        }
        if range.0 > w1 {
            return;
        }
        if !(range.0 >= w0) {
            range.0 = w0;
        }
        if range.1 > w1 {
            range.1 = w1;
        }
        let (a, b) = *range;
        if b - a < l {
            return;
        }
        let mut f0 = a;
        loop {
            out[*count] = (f0, range.0 + l);
            *count += 1;
            let a2 = range.0 + l;
            let rem = range.1 - a2;
            range.0 = a2;
            if rem < l {
                return;
            }
            if a2 + l > w1 {
                return;
            }
            if *count >= 64 {
                return;
            }
            f0 = range.0;
        }
    }

    /// The candidate slots `0x828ECAB0` would choose from now (for checks).
    pub fn slots(&self) -> Vec<(f32, f32)> {
        let (slots, count, _, _) = self.collect();
        slots[..count].to_vec()
    }

    fn collect(&self) -> ([(f32, f32); 64], usize, f32, i32) {
        let p = &self.params;
        let l = (p.hold + p.fade_out) + p.fade_in;
        let span = self.duration - p.window;
        let w0 = span * self.position;
        let n_fit = fctiwz(p.window / l);
        let w1 = p.window + w0;
        let mut out = [(MINUS_ONE, MINUS_ONE); 64];
        let mut count = 0usize;
        let head = self.head;
        let e0 = self.span(head).start;
        if w0 < e0 {
            let mut r = (w0, e0);
            Self::tile(&mut r, l, w0, w1, &mut out, &mut count);
        }
        let mut i = head;
        while i != -1 {
            let nx = self.span(i).next;
            let lo = self.span(i).end;
            let hi = if nx == -1 { self.duration } else { self.span(nx).start };
            if count < 64 {
                let mut r = (lo, hi);
                Self::tile(&mut r, l, w0, w1, &mut out, &mut count);
            }
            i = nx;
        }
        (out, count, w0, n_fit)
    }

    fn span(&self, i: i16) -> Span {
        self.spans[i as usize & 15]
    }

    /// `0x828ECAB0` with the slot choice supplied: `choose(count, slots)`
    /// returns the index the random word gave (see [`pick_index`]).
    pub fn next_start_with(&mut self, choose: &mut Choose<'_>) -> f32 {
        loop {
            let (slots, count, w0, n_fit) = self.collect();
            if count == 0 || self.last == 15 {
                // Keep only the newest interval; rebuild the free list.
                let keep = self.span(self.last);
                self.spans[0] = Span { start: keep.start, end: keep.end, next: -1 };
                self.head = 0;
                self.last = 0;
                self.free = 1;
                for i in 1..16 {
                    self.spans[i] = Span { start: MINUS_ONE, end: MINUS_ONE, next: i as i16 + 1 };
                }
                self.h360 = -1;
                if n_fit > 1 {
                    continue;
                }
                return w0;
            }
            let idx = choose(count, &slots[..count]);
            let pick = if idx >= 0 && (idx as usize) < 64 { slots[idx as usize] } else { (MINUS_ONE, MINUS_ONE) };
            let new = self.free;
            self.free = self.span(new).next;
            self.spans[new as usize & 15] = Span { start: pick.0, end: pick.1, next: 0 };
            self.last = new;
            // Insert sorted by start.
            let v = pick.0;
            let mut prev: i16 = -1;
            let mut cur = self.head;
            if self.span(cur).start < v {
                loop {
                    prev = cur;
                    cur = self.span(cur).next;
                    if cur == -1 {
                        break;
                    }
                    if !(self.span(cur).start < v) {
                        break;
                    }
                }
            }
            if cur == -1 {
                self.spans[prev as usize & 15].next = new;
                self.spans[new as usize & 15].next = -1;
            } else if prev == -1 {
                self.head = new;
                self.spans[new as usize & 15].next = cur;
            } else {
                self.spans[new as usize & 15].next = cur;
                self.spans[prev as usize & 15].next = new;
            }
            return pick.0;
        }
    }

    /// `0x828EC3F0` (with `0x828EC208`): a grain on `sub` from `at`.
    pub fn start(&mut self, sub: usize, at: f32, events: &mut Vec<Event>) {
        let mut t = at;
        if t < ZERO {
            t = ZERO;
        } else if t > self.duration {
            t = self.duration;
        }
        let fade_in = self.params.fade_in;
        let s = &mut self.subs[sub];
        s.live = true;
        s.timer = fade_in;
        s.state = 1;
        s.start = t;
        s.target = self.position;
        events.push(Event::Start { sub, at: t, fade_in });
    }

    /// Starts a sub's fade-out (`0x828EC2F8`).
    fn fade_out(&mut self, sub: usize, events: &mut Vec<Event>) {
        let time = self.params.fade_out;
        self.subs[sub].timer = time;
        self.subs[sub].state = 3;
        events.push(Event::FadeOut { sub, time });
    }

    fn kill(&mut self, sub: usize, events: &mut Vec<Event>) {
        self.subs[sub].reset();
        events.push(Event::Kill { sub });
    }

    /// The board's recording change (`0x824C5CA8`, per voice): new
    /// recording, tuned values, the first grain on sub 0.
    pub fn begin(&mut self, duration: f32, params: GrainParams, choose: &mut Choose<'_>, events: &mut Vec<Event>) {
        self.set_recording(duration, params);
        let at = self.next_start_with(choose);
        self.start(0, at, events);
    }

    /// `0x828EC6F0`: the audio-block service (`dt` = [`DT`]). Level and
    /// pitch are re-posted to every live sub's Send and Resample here.
    pub fn service(&mut self, dt: f32, choose: &mut Choose<'_>, events: &mut Vec<Event>) {
        let cur = self.current;
        let d = self.subs[cur].target - self.position;
        let tol = self.params.tolerance;
        if (d > tol || d < -tol) && !self.hold_off && self.subs[cur].live {
            let other = (cur + 1) % 2;
            if self.subs[other].live {
                if self.subs[other].state != 3 {
                    self.fade_out(other, events);
                }
            } else {
                if self.subs[cur].state != 3 {
                    self.fade_out(cur, events);
                }
                let at = self.next_start_with(choose);
                self.start(other, at, events);
                self.current = (self.current + 1) % 2;
            }
        }
        for s in 0..2 {
            if !self.subs[s].live {
                continue;
            }
            let state = self.subs[s].state;
            let timer = if state == 2 { fnmsubs(dt, self.pitch, self.subs[s].timer) } else { self.subs[s].timer - dt };
            self.subs[s].timer = timer;
            if !(timer < ZERO) {
                continue;
            }
            match state {
                1 => {
                    self.subs[s].state = 2;
                    self.subs[s].timer = self.params.hold;
                }
                2 => {
                    self.fade_out(s, events);
                    if !self.hold_off {
                        let o = (s + 1) % 2;
                        if self.subs[o].live {
                            self.kill(o, events);
                        }
                        let at = self.next_start_with(choose);
                        self.start(o, at, events);
                        self.current = o;
                    }
                }
                3 if self.subs[s].live => self.kill(s, events),
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GainFader, linear power (`0x82B238A8`, `0x82B23828`, `0x82B42C98`)
// ---------------------------------------------------------------------------

/// One GainFader instance (the fields the grain path uses).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fader {
    /// `+108`: the gain reached (also the attribute "current gain").
    pub gain: f32,
    /// `+112`: a fade command waiting for the next block.
    pending: Option<(f32, f32, u8)>,
    /// `+113`: 0 idle, 1 armed, 2 fading.
    state: u8,
    /// `+92` fade length, `+96` position (samples).
    total: i32,
    pos: i32,
    /// `+100`, `+104`, `+114`.
    from: f32,
    to: f32,
    kind: u8,
}

impl Default for Fader {
    fn default() -> Self {
        Fader { gain: ONE, pending: None, state: 0, total: 0, pos: 0, from: ZERO, to: ZERO, kind: 0 }
    }
}

impl Fader {
    /// `0x82B23798` → `0x82B23828`: a fade with start time 0. Time 0 sets
    /// the gain at once; otherwise the fade starts at the next block.
    /// `kind` is `round(type)`: the grain player passes 1.0 (linear power).
    pub fn command(&mut self, time: f32, end: f32, kind: u8) {
        if time == ZERO {
            self.gain = end;
            self.state = 0;
            self.pending = None;
        } else {
            self.pending = Some((time, end, kind));
        }
    }

    /// `0x82B238A8` for one block of 256 samples per channel at `rate`
    /// (the mixer's): multiplies `channels` in place. Only the
    /// linear-power profile is ported (the grain player's).
    pub fn process(&mut self, rate: f32, channels: &mut [&mut [f32]]) {
        if let Some((time, end, kind)) = self.pending.take() {
            let n = fctiwz(time * rate);
            self.total = if n > 0 { n } else { 1 };
            self.from = self.gain;
            self.to = end;
            self.kind = kind;
            self.state = 1;
        }
        if self.state == 1 {
            // Start time 0: delay 0, so the fade starts at this block.
            self.pos = 0;
            self.state = 2;
        }
        let mut env = [ZERO; 256];
        if self.state == 2 {
            assert_eq!(self.kind, 1, "only the linear-power fade is ported");
            linear_power(&mut env, self.from, self.to, self.pos, self.total);
            self.pos += 256;
            if self.pos >= self.total {
                self.state = 0;
            }
        } else {
            if self.gain == ONE {
                return;
            }
            env = [self.gain; 256];
        }
        for ch in channels.iter_mut() {
            for (x, g) in ch.iter_mut().zip(env.iter()) {
                *x *= *g;
            }
        }
        self.gain = env[255];
    }
}

/// `x · rsqrt(x)` as the kernel computes it: `vrsqrtefp` estimate, one
/// Newton step (`y = e + e·(0.5 − 0.5x·e²)`), `x·y`; where exactly one of
/// the step's terms is NaN (x = 0 or ∞) it keeps `x`.
fn vsqrt(x: V128) -> V128 {
    let half = vmx::set1_u32(0x3F00_0000);
    let est = vmx::rsqrte(x);
    let est2 = vmx::mul(est, est);
    let h = vmx::mul(x, half);
    let t = vmx::nmadd(h, est2, half);
    let y = vmx::madd(est, t, est);
    let s = vmx::mul(x, y);
    let mask = vmx::xor(vmx::cmpeq(t, t), vmx::cmpeq(est, est));
    vmx::sel(mask, s, x)
}

fn splat(v: f32) -> V128 {
    vmx::set1_u32(v.to_bits())
}

/// `0x82B42C98` for `pos >= 0` (the only case with start time 0): fade
/// samples `pos..=min(pos + 255, n − 1)` into `env[0..]`, the rest of the
/// block at `to`.
pub fn linear_power(env: &mut [f32; 256], from: f32, to: f32, pos: i32, n: i32) {
    let last = (n - 1).min(pos + 255);
    let delta = to - from;
    let nf = n as f32;
    let root = (nf as f64).sqrt() as f32;
    let step = delta / root;
    let rising = !(delta < ZERO);
    let count = last - pos + 1;
    let groups = if count > 0 { count / 4 } else { 0 };
    let x0 = [pos + 1, pos + 2, pos + 3, pos + 4].map(|v| v as f32);
    let mut x = V128::default();
    for l in 0..4 {
        x.set_u32(l, x0[l].to_bits());
    }
    let (inc, scale, base) = if rising {
        (splat(4.0), splat(step), splat(from))
    } else {
        (splat(-4.0), splat(step * MINUS_ONE), splat(to))
    };
    if !rising {
        x = vmx::sub(splat(nf), x);
    }
    let mut o = 0usize;
    for _ in 0..groups {
        let g = vmx::madd(scale, vsqrt(x), base);
        for l in 0..4 {
            env[o + l] = g.f32(l);
        }
        o += 4;
        x = vmx::add(x, inc);
    }
    let mut i = pos + 4 * groups;
    while i <= last {
        let xi = (i + 1) as f64;
        env[o] = if rising {
            fmadds((xi.sqrt()) as f32, step, from)
        } else {
            let f11 = nf - xi as f32;
            fnmsubs((f11 as f64).sqrt() as f32, step, to)
        };
        o += 1;
        i += 1;
    }
    while o < 256 {
        env[o] = to;
        o += 1;
    }
}

// ---------------------------------------------------------------------------
// A dry renderer (composition; see the report for what is exact)
// ---------------------------------------------------------------------------

/// One recording, decoded (mono f32 at its SNR rate).
pub struct Recording {
    pub rate: f32,
    pub pcm: Vec<f32>,
    pub duration: f32,
}

#[derive(Clone)]
struct VoiceState {
    cursor: usize,
    fader: Fader,
    resample: crate::eac::resample::Resample,
    /// Blocks until SndPlayer1 delivers (an XMA entry without a start time
    /// starts one block late, `0x82B31EE0`).
    delay: u32,
    send: f32,
}

/// Grain player + its two voices, rendered dry (before the truck submix
/// chain): SndPlayer1 (continuous decode read from `start_sample`) →
/// Resample (the `eac::resample` port) → GainFader → Send gain.
pub struct DryGrains {
    pub player: GrainPlayer,
    voices: [Option<VoiceState>; 2],
}

impl DryGrains {
    pub fn new() -> DryGrains {
        DryGrains { player: GrainPlayer::new(), voices: [None, None] }
    }

    /// Stop both graphs as well as the service. Calling only the scheduler's
    /// stop would leave the already-created dry voices sounding.
    pub fn stop(&mut self) {
        let mut events = Vec::new();
        self.player.stop(&mut events);
        self.voices = [None, None];
    }

    pub fn set_recording(&mut self, rec: &Recording, params: GrainParams, rng: &mut GameRng) {
        self.stop();
        let mut events = Vec::new();
        self.player.begin(rec.duration, params, &mut |count, _| pick_index(rng.next(), count), &mut events);
        self.apply(rec, &events);
    }

    pub fn voices(&self) -> u32 {
        self.voices.iter().filter(|v| v.is_some()).count() as u32
    }

    fn apply(&mut self, rec: &Recording, events: &[Event]) {
        for e in events {
            match *e {
                Event::Start { sub, at, fade_in } => {
                    let mut fader = Fader::default();
                    fader.command(ZERO, ZERO, 0);
                    fader.command(fade_in, ONE, 1);
                    self.voices[sub] = Some(VoiceState {
                        cursor: start_sample(rec.rate, at).max(0) as usize,
                        fader,
                        resample: crate::eac::resample::Resample::new(1),
                        delay: 1,
                        send: self.player.level,
                    });
                }
                Event::FadeOut { sub, time } => {
                    if let Some(v) = self.voices[sub].as_mut() {
                        v.fader.command(time, ZERO, 1);
                    }
                }
                Event::Kill { sub } => self.voices[sub] = None,
            }
        }
    }

    /// One 256-sample block at 48 kHz, added into `out`.
    pub fn block(&mut self, rec: &Recording, rng: &mut GameRng, out: &mut [f32; 256]) {
        let mut events = Vec::new();
        self.player.service(DT, &mut |count, _| pick_index(rng.next(), count), &mut events);
        self.apply(rec, &events);
        for v in self.voices.iter_mut().flatten() {
            v.resample.pitch = self.player.pitch;
            let mut buf = [ZERO; 256];
            if v.delay > 0 {
                v.delay -= 1;
            } else {
                let mut factor = ONE;
                let need = v.resample.request(MIX_RATE, 256, &mut factor) as usize;
                let end = (v.cursor + need).min(rec.pcm.len());
                let have = &rec.pcm[v.cursor.min(end)..end];
                let (mut stack, mut heap);
                let input: &mut [f32] = if need <= MAX_INPUT {
                    stack = [ZERO; MAX_INPUT];
                    &mut stack[..need]
                } else {
                    heap = vec![ZERO; need];
                    &mut heap
                };
                input[..have.len()].copy_from_slice(have);
                v.cursor += need;
                let mut outs: [&mut [f32]; 1] = [&mut buf[..]];
                // Retail Resample deliberately drops the first block after
                // a source-rate change. Re-requesting then reusing the old
                // request's input can overflow its six-sample history (e.g.
                // a 24 kHz grain after the initial 48 kHz default).
                v.resample.process(rec.rate, need as u32, &[&input[..]], &mut outs);
            }
            v.fader.process(MIX_RATE, &mut [&mut buf[..]]);
            // Send: gain ramps from the last block's value across the block.
            let target = self.player.level;
            for (i, (o, x)) in out.iter_mut().zip(buf.iter()).enumerate() {
                let g = v.send + (target - v.send) * ((i + 1) as f32 / 256.0);
                *o += g * *x;
            }
            v.send = target;
        }
    }
}

impl Default for DryGrains {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve() -> Curve {
        // concrete_rough_hard (surface 2, hard) from the TU3 heap snapshot.
        Curve::from_floats(&[
            1.0, 1.0, 0.0, 0.0, 0.1416667, 0.9137931, 0.0, 0.0, 0.2416667, 0.03448276, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            31.186224, 60.0, 2.0, 10.0, -100.0, 0.6, -10.0,
        ])
    }

    #[test]
    fn rng_matches_a_192_bit_add_chain() {
        let mut g = GameRng { s: [1, 2, 3, 4, 5, 6] };
        let r = g.next();
        // s4 = 5 + 6, s3 = 4 + s4, s2 = 3 + s3, s1 = 2 + s2, s0 = 1 + s1.
        assert_eq!(g.s, [21, 20, 18, 15, 11, 7]);
        assert_eq!(r, 21);
        // Carries ripple.
        let mut g = GameRng { s: [0, 0, 0, 0xFFFF_FFFF, 0xFFFF_FFFF, 1] };
        g.next();
        assert_eq!(g.s, [1, 1, 1, 0, 0, 2]);
        // s5 wrapping increments the higher words.
        let mut g = GameRng { s: [0, 0, 0, 0, 0, 0xFFFF_FFFF] };
        g.next();
        assert_eq!(g.s[5], 0);
        assert_eq!(g.s[4], 0xFFFF_FFFF_u32.wrapping_add(1));
    }

    #[test]
    fn pick_index_scales_to_count() {
        assert_eq!(pick_index(0, 4), 0);
        assert_eq!(pick_index(0x4000_0000, 4), 1);
        assert_eq!(pick_index(0xFFFF_FF00, 4), 3);
        // Rounds up to 2^32: one past the slots.
        assert_eq!(pick_index(0xFFFF_FFFF, 4), 4);
    }

    #[test]
    fn position_follows_the_curve() {
        let c = curve();
        assert_eq!(position(0.0, &c), 0.0);
        assert_eq!(position(100.0, &c), 1.0);
        let mid = position(30.0 / 3.6, &c); // u = 0.5
        let expect = 0.125 * 1.0 + 3.0 * 0.25 * (0.9137931 * 0.5 + 0.03448276 * 0.5);
        assert!((mid - expect).abs() < 1e-6, "{mid} {expect}");
    }

    #[test]
    fn blocks_levels() {
        let c = curve();
        let i = Inputs { speed: 5.0, level_a: 32767, level_b: 16384, pitch: 4096, blend: 0.25, backwards: 0.5, ..Default::default() };
        let (a, b) = blocks(&i, &c, &c);
        assert_eq!(a.level, 0.5 * (32767.0 * INV_32767));
        assert_eq!(a.pitch, 1.0);
        assert_eq!(b.level, 0.25 * (16384.0 * INV_32767));
        assert_eq!(b.position, (a.position - B_BEHIND).max(0.0));
        // Tilt boost: + tilt · clamp01(speed·3.6/10) · 2 · level A, capped at 1.
        let i = Inputs { tilt: 1.0, ..i };
        let (_, b) = blocks(&i, &c, &c);
        assert_eq!(b.level, 1.0);
    }

    #[test]
    fn slots_tile_the_window() {
        let mut p = GrainPlayer::new();
        p.set_recording(21.84, GrainParams::VOICE_A);
        p.position = 0.0;
        let s = p.slots();
        // Window [0, 1.6] in 0.4 s slots (the history's first entry is
        // [-2, -1], so the gap after it starts at -1 and is clipped to 0).
        // In f32, 1.6 - 1.2 < 0.1 + 0.2 + 0.1: three slots, not four.
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].0, 0.0);
        let mut first = |_: usize, _: &[(f32, f32)]| 0;
        let at = p.next_start_with(&mut first);
        assert_eq!(at, 0.0);
        // The played slot is now excluded.
        let s = p.slots();
        assert_eq!(s.len(), 2);
        assert!(s.iter().all(|x| x.0 >= 0.39));
    }

    #[test]
    fn heap_snapshot_history_shape() {
        // Voice B at position 0: slots [0, 0.5], [0.5, 1.0], [1.0, 1.5].
        let mut p = GrainPlayer::new();
        p.set_recording(21.84, GrainParams::VOICE_B);
        let mut k = 0;
        let mut seq = |_: usize, s: &[(f32, f32)]| {
            k += 1;
            // Always the slot after the last one played, like the snapshot.
            let want = [0.0f32, 0.5, 1.0][(k - 1) % 3];
            s.iter().position(|x| x.0 == want).unwrap() as i32
        };
        for _ in 0..3 {
            p.next_start_with(&mut seq);
        }
        let mut list = Vec::new();
        let mut i = p.head;
        while i != -1 {
            list.push((p.spans[i as usize].start, p.spans[i as usize].end));
            i = p.spans[i as usize].next;
        }
        assert_eq!(list, vec![(-2.0, -1.0), (0.0, 0.5), (0.5, 1.0), (1.0, 1.5)]);
        // A full window resets to the newest interval and starts at w0.
        let at = p.next_start_with(&mut seq);
        assert_eq!(at, 0.0);
    }

    #[test]
    fn service_crossfades() {
        let mut p = GrainPlayer::new();
        let mut ev = Vec::new();
        let mut first = |_: usize, _: &[(f32, f32)]| 0;
        p.begin(21.84, GrainParams::VOICE_A, &mut first, &mut ev);
        assert_eq!(ev, vec![Event::Start { sub: 0, at: 0.0, fade_in: GrainParams::VOICE_A.fade_in }]);
        let mut starts = Vec::new();
        for n in 0..400 {
            ev.clear();
            p.service(DT, &mut first, &mut ev);
            for e in &ev {
                if let Event::Start { .. } = e {
                    starts.push(n);
                }
            }
        }
        // fade in 0.1 s (19 blocks) + hold 0.2 s (38 blocks) at pitch 1;
        // a grain started on sub 1 from sub 0's iteration is ticked in the
        // same service call, one on sub 0 is not: gaps alternate 56 / 57.
        let gaps: Vec<i32> = starts.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(starts[0], 56);
        assert!(gaps.iter().enumerate().all(|(i, &g)| g == if i % 2 == 0 { 56 } else { 57 }), "{gaps:?}");
    }

    #[test]
    fn fader_linear_power() {
        let mut f = Fader::default();
        f.command(0.0, 0.0, 0);
        assert_eq!(f.gain, 0.0);
        f.command(0.1, 1.0, 1);
        let n = 4800;
        let mut got = Vec::new();
        for _ in 0..20 {
            let mut b = [1.0f32; 256];
            f.process(MIX_RATE, &mut [&mut b[..]]);
            got.extend_from_slice(&b);
        }
        for (i, g) in got.iter().enumerate().take(n) {
            let exact = (((i + 1) as f64) / n as f64).sqrt() as f32;
            assert!((g - exact).abs() < 2e-6, "{i} {g} {exact}");
        }
        // The estimate ends a hair below 1 (0.9999999); the rest of the
        // block is filled with the end gain, so the fader rests at 1.
        assert!((got[n - 1] - 1.0).abs() < 2e-7);
        assert!(got[n..].iter().all(|&g| g == 1.0));
        // Fade out from 1 to 0.
        f.command(0.1, 0.0, 1);
        let mut out = Vec::new();
        for _ in 0..19 {
            let mut b = [1.0f32; 256];
            f.process(MIX_RATE, &mut [&mut b[..]]);
            out.extend_from_slice(&b);
        }
        for (i, g) in out.iter().enumerate().take(n) {
            let exact = (((n - i - 1) as f64) / n as f64).sqrt() as f32;
            assert!((g - exact).abs() < 2e-6, "{i} {g} {exact}");
        }
        assert_eq!(out[n - 1], 0.0);
    }

    #[test]
    fn stream_mode_surfaces() {
        assert!(stream_mode(1) && stream_mode(6) && stream_mode(9) && stream_mode(0));
        assert!(!stream_mode(7) && !stream_mode(8) && !stream_mode(10) && !stream_mode(13));
    }
}

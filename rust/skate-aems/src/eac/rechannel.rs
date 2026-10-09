//! `Rechannel` (descriptor `0x82FD1868`, 4CC `Rch0`, process `0x82B2C8F0`):
//! converts the chain's channel count to the instance's own by folding up
//! or down the standard layouts (1, 2, 4, 6, 8) through a routing matrix, or
//! by copying and zero-filling channels for any other count.
//!
//! Instance (44 bytes, size function `0x82B2C8C0`): the plug-in header
//! only. The constructor `0x82B2C8C8` stores the vtable `0x8231B924`; the
//! descriptor declares no attributes, so nothing changes the channel counts
//! after creation. The graph builder `0x82B48C48` sets them like every
//! node's: `+41` = the upstream node's channel count, `+42` = the count in
//! the node's spec. The voice builder `0x824A3140` gives Rechannel (spec 1,
//! after SndPlayer1) the voice's channel count (its `+68`, from the sample
//! header read by `0x82B31D90`), the same as SndPlayer1's, so in the voice
//! chain it only acts when a block arrives with another count (context
//! `+60`).
//!
//! Layouts follow the matrix: 6 = L C R Ls Rs LFE, 8 = L C R Ls Rs Lb Rb
//! LFE, 4 = L R Ls Rs. The LFE is dropped when folding down; the centre goes
//! to both sides at 0.707; every other contribution has gain 1.

use super::lfs;
use crate::ops::fmadds;
use crate::ppc::vmx;

/// The plain-copy gain of the non-standard path (`0x8231A844`, 1.0).
pub const COPY_GAIN: f32 = f32::from_bits(0x3F80_0000);

/// Route gains (`0x820ED6C0`): 1.0, 0.707, 0.5 and a word of whatever
/// follows (no route uses the last two).
pub const GAINS: [f32; 4] = [
    f32::from_bits(0x3F80_0000),
    f32::from_bits(0x3F34_FDF4),
    f32::from_bits(0x3F00_0000),
    f32::from_bits(0x0006_070E),
];

/// Route ranges `[first, last]` into [`ROUTES`], indexed `in·8 + out − 9`
/// (`0x820ED700`, 64 pairs; only standard pairs are ever looked up).
pub const RANGES: [[u8; 2]; 64] = [
    [0x54, 0x54], [0x00, 0x01], [0xff, 0xff], [0x02, 0x03], [0xff, 0xff], [0x04, 0x04], [0xff, 0xff], [0x05, 0x05],
    [0x06, 0x07], [0x54, 0x55], [0xff, 0xff], [0x08, 0x09], [0xff, 0xff], [0x0a, 0x0b], [0xff, 0xff], [0x0c, 0x0d],
    [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff],
    [0x0e, 0x11], [0x12, 0x15], [0xff, 0xff], [0x54, 0x57], [0xff, 0xff], [0x16, 0x19], [0xff, 0xff], [0x1a, 0x1d],
    [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff],
    [0x1e, 0x22], [0x23, 0x28], [0xff, 0xff], [0x29, 0x2e], [0xff, 0xff], [0x54, 0x59], [0xff, 0xff], [0x2f, 0x34],
    [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff], [0xff, 0xff],
    [0x35, 0x3b], [0x3c, 0x43], [0xff, 0xff], [0x44, 0x4b], [0xff, 0xff], [0x4c, 0x53], [0xff, 0xff], [0x54, 0x5b],
];

/// Routes (`0x820ED780`, 92 bytes): bits 0-2 output channel, 3-5 input
/// channel, 6-7 gain index. The first route to an output scales into it,
/// later ones accumulate, in table order.
pub const ROUTES: [u8; 92] = [
    0x40, 0x41, 0x40, 0x41, 0x01, 0x01, 0x00, 0x08, 0x00, 0x09, 0x00, 0x0a, 0x00, 0x0a, 0x00, 0x08,
    0x10, 0x18, 0x00, 0x09, 0x10, 0x19, 0x00, 0x0a, 0x13, 0x1c, 0x00, 0x0a, 0x13, 0x1c, 0x08, 0x10,
    0x20, 0x18, 0x00, 0x48, 0x00, 0x18, 0x49, 0x11, 0x21, 0x48, 0x00, 0x1a, 0x49, 0x11, 0x23, 0x09,
    0x00, 0x1b, 0x12, 0x24, 0x2f, 0x08, 0x00, 0x18, 0x28, 0x10, 0x20, 0x30, 0x48, 0x00, 0x18, 0x28,
    0x49, 0x11, 0x21, 0x31, 0x48, 0x00, 0x1a, 0x2a, 0x49, 0x11, 0x23, 0x33, 0x09, 0x00, 0x1b, 0x2b,
    0x12, 0x24, 0x34, 0x3d, 0x00, 0x09, 0x12, 0x1b, 0x24, 0x2d, 0x36, 0x3f,
];

/// Most channels the process stages (its pointer arrays hold eight inputs;
/// the matrix's written flags eight outputs).
pub const MAX_CHANNELS: u8 = 8;

/// Whether a channel count has a matrix row (`0x82B468C0`).
pub fn standard(channels: u8) -> bool {
    matches!(channels, 1 | 2 | 4 | 6 | 8)
}

/// One route decoded: `(input, output, gain)`.
pub fn route(e: u8) -> (usize, usize, f32) {
    (((e >> 3) & 7) as usize, (e & 7) as usize, GAINS[((e >> 6) & 3) as usize])
}

/// Where the chain's buffer sets sit in guest memory (context `+28`/`+32`:
/// `+4` data, `+14` stride in samples). Only the kernels' choice of path
/// depends on it: they run VMX when both pointers are 128-byte aligned and
/// the count is a multiple of 64.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub input: u32,
    pub input_stride: u16,
    pub output: u32,
    pub output_stride: u16,
}

impl Placement {
    /// Aligned 256-sample buffers.
    pub const ALIGNED: Placement = Placement { input: 0, input_stride: 256, output: 0, output_stride: 256 };

    fn input_at(&self, ch: usize) -> u32 {
        self.input.wrapping_add((self.input_stride as u32 * 4).wrapping_mul(ch as u32))
    }

    fn output_at(&self, ch: usize) -> u32 {
        self.output.wrapping_add((self.output_stride as u32 * 4).wrapping_mul(ch as u32))
    }

    /// The kernels' path test for input `i` into output `o`.
    pub fn vector(&self, i: usize, o: usize, count: u32) -> bool {
        (self.input_at(i) | self.output_at(o)) & 127 == 0 && count & 63 == 0
    }
}

/// `0x82B3BED8(out, in, gain, count)`: `out = in·gain`. The VMX path
/// (`vmulfp` on 128-byte lines) and the scalar path (`lfs`/`fmuls`, four at
/// a time then one) give the same single product. A VMX call with count 0
/// would never end; the callers never make one.
pub fn scale(out: &mut [f32], input: &[f32], gain: f32, count: u32) {
    // Opaque, so a constant 1.0 is not folded away: the multiply quiets
    // signalling NaNs, as the game's does.
    let gain = std::hint::black_box(gain);
    let n = count as usize;
    for (o, &x) in out[..n].iter_mut().zip(&input[..n]) {
        *o = lfs(x) * gain;
    }
}

/// `0x82B44B20(out, in, gain, count)`: `out += in·gain`. The VMX path is
/// `vmaddfp` as the recompiled game lowers it (`in·gain`, rounded, `+ out`,
/// rounded); the scalar path is `fmadds` (one fused operation in double).
pub fn mix(out: &mut [f32], input: &[f32], gain: f32, count: u32, vector: bool) {
    let gain = std::hint::black_box(gain);
    let n = count as usize;
    if vector {
        // Through the same lane helper as the translation, so that even
        // the payload kept when both addends are NaN (which LLVM may pick
        // either way) tends to agree.
        let g = vmx::set1_u32(gain.to_bits());
        for (o, x) in out[..n].chunks_exact_mut(4).zip(input[..n].chunks_exact(4)) {
            let mut vx = vmx::V128::default();
            let mut vo = vmx::V128::default();
            for l in 0..4 {
                vx.set_u32(l, lfs(x[l]).to_bits());
                vo.set_u32(l, lfs(o[l]).to_bits());
            }
            let r = vmx::madd(g, vx, vo);
            for l in 0..4 {
                o[l] = f32::from_bits(r.u32(l));
            }
        }
        return;
    }
    for (o, &x) in out[..n].iter_mut().zip(&input[..n]) {
        *o = fmadds(lfs(x), gain, lfs(*o));
    }
}

/// `0x82EE5E80(out, 0, count·4)`.
fn zero(out: &mut [f32], count: u32) {
    out[..count as usize].fill(0.0);
}

/// `0x82B426D0(outs, ins, out_channels, count, range, ROUTES)`: the routes
/// of `RANGES[in·8 + out − 9]` in order, then zeros for any output no route
/// wrote.
pub fn matrix(output: &mut [&mut [f32]], input: &[&[f32]], in_channels: u8, out_channels: u8, count: u32, at: &Placement) {
    let [first, last] = RANGES[in_channels as usize * 8 + out_channels as usize - 9];
    let mut written = [false; 8];
    if first <= last {
        for &e in &ROUTES[first as usize..=last as usize] {
            let (i, o, gain) = route(e);
            if written[o] {
                mix(output[o], input[i], gain, count, at.vector(i, o, count));
            } else {
                scale(output[o], input[i], gain, count);
                written[o] = true;
            }
        }
    }
    for (o, out) in output.iter_mut().enumerate().take(out_channels as usize) {
        if !written[o] {
            zero(out, count);
        }
    }
}

/// `0x82B468C0(outs, ins, out_channels, in_channels, count)`: the matrix for
/// two standard counts; otherwise channel `k` is copied for `k <
/// min(in, out)` and any further outputs are zeroed.
pub fn rechannel(output: &mut [&mut [f32]], input: &[&[f32]], out_channels: u8, in_channels: u8, count: u32, at: &Placement) {
    assert!(in_channels <= MAX_CHANNELS && out_channels <= MAX_CHANNELS, "more than eight channels");
    if standard(out_channels) && standard(in_channels) {
        matrix(output, input, in_channels, out_channels, count, at);
        return;
    }
    let copies = in_channels.min(out_channels) as usize;
    for k in 0..copies {
        scale(output[k], input[k], COPY_GAIN, count);
    }
    for out in output.iter_mut().take(out_channels as usize).skip(copies) {
        zero(out, count);
    }
}

/// What a process call did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Processed {
    /// The chain's channel count after the call (context `+60`).
    pub channels: u8,
    /// The buffer sets were swapped: the block is in `output` now (only its
    /// first `count` samples are written; with count 0 nothing is).
    pub swapped: bool,
}

/// One Rechannel instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rechannel {
    /// `+41`: the upstream channel count given at creation; the process
    /// overwrites it with the block's count, but only on an empty block.
    pub in_channels: u8,
    /// `+42`: the channel count this plug-in produces.
    pub out_channels: u8,
}

impl Rechannel {
    /// As the graph builder leaves it: `upstream` channels in, `channels`
    /// out.
    pub fn new(upstream: u8, channels: u8) -> Rechannel {
        Rechannel { in_channels: upstream, out_channels: channels }
    }

    /// One block (`0x82B2C8F0`): `channels` (context `+60`) channels of
    /// `count` (context `+48`) samples. Passes through when the count
    /// already matches; otherwise writes `out_channels` channels to `output`
    /// (when `count` is not 0) and reports the swap.
    pub fn process(&mut self, channels: u8, count: u32, input: &[&[f32]], output: &mut [&mut [f32]], at: &Placement) -> Processed {
        if count == 0 {
            self.in_channels = channels;
        }
        if channels == self.out_channels {
            return Processed { channels, swapped: false };
        }
        if count != 0 {
            rechannel(output, input, self.out_channels, channels, count, at);
        }
        Processed { channels: self.out_channels, swapped: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = 256;
    const C707: f32 = f32::from_bits(0x3F34_FDF4);

    fn run(r: &mut Rechannel, input: &[Vec<f32>], count: u32) -> (Processed, Vec<Vec<f32>>) {
        let ins: Vec<&[f32]> = input.iter().map(|v| v.as_slice()).collect();
        let mut out = vec![vec![f32::from_bits(0x7FC0_DEAD); N]; 8];
        let mut outs: Vec<&mut [f32]> = out.iter_mut().map(|v| v.as_mut_slice()).collect();
        let p = r.process(input.len() as u8, count, &ins, &mut outs, &Placement::ALIGNED);
        (p, out)
    }

    fn ramp(k: f32) -> Vec<f32> {
        (0..N).map(|i| (i as f32 + 1.0) * k).collect()
    }

    #[test]
    fn same_count_passes_through() {
        let mut r = Rechannel::new(1, 1);
        let (p, out) = run(&mut r, &[ramp(0.01)], N as u32);
        assert_eq!(p, Processed { channels: 1, swapped: false });
        assert!(out[0][0].is_nan());
        let mut r = Rechannel::new(3, 3);
        assert!(!run(&mut r, &[ramp(1.0), ramp(2.0), ramp(3.0)], 17).0.swapped);
    }

    #[test]
    fn stereo_to_mono_sums() {
        let mut r = Rechannel::new(2, 1);
        let (l, rr) = (ramp(0.25), ramp(-0.5));
        let (p, out) = run(&mut r, &[l.clone(), rr.clone()], N as u32);
        assert_eq!(p, Processed { channels: 1, swapped: true });
        for i in 0..N {
            assert_eq!(out[0][i], l[i] + rr[i]);
        }
    }

    #[test]
    fn mono_to_stereo_at_707() {
        let mut r = Rechannel::new(1, 2);
        let m = ramp(0.125);
        let (p, out) = run(&mut r, std::slice::from_ref(&m), N as u32);
        assert_eq!(p.channels, 2);
        for i in 0..N {
            assert_eq!(out[0][i], m[i] * C707);
            assert_eq!(out[1][i], m[i] * C707);
        }
    }

    #[test]
    fn mono_to_six_is_centre_only() {
        let mut r = Rechannel::new(1, 6);
        let m = ramp(1.0);
        let (_, out) = run(&mut r, std::slice::from_ref(&m), 64);
        assert_eq!(&out[1][..64], &m[..64]);
        for ch in [0, 2, 3, 4, 5] {
            assert!(out[ch][..64].iter().all(|&s| s.to_bits() == 0));
        }
        assert!(out[0][64].is_nan(), "only count samples are written");
    }

    #[test]
    fn six_to_stereo_drops_lfe() {
        let mut r = Rechannel::new(6, 2);
        let ch: Vec<Vec<f32>> = (0..6).map(|k| ramp(1.0 + k as f32)).collect();
        let (_, out) = run(&mut r, &ch, N as u32);
        for i in 0..N {
            // Table order: C·0.707, then + L, then + Ls (fused in the scalar
            // path, rounded twice in the VMX one: exact here either way).
            assert_eq!(out[0][i], (ch[1][i] * C707 + ch[0][i]) + ch[3][i]);
            assert_eq!(out[1][i], (ch[1][i] * C707 + ch[2][i]) + ch[4][i]);
        }
    }

    #[test]
    fn non_standard_copies_and_zero_fills() {
        let mut r = Rechannel::new(3, 5);
        let ch: Vec<Vec<f32>> = (0..3).map(|k| ramp(1.0 + k as f32)).collect();
        let (p, out) = run(&mut r, &ch, 10);
        assert_eq!(p, Processed { channels: 5, swapped: true });
        for k in 0..3 {
            assert_eq!(&out[k][..10], &ch[k][..10]);
        }
        assert!(out[3][..10].iter().chain(&out[4][..10]).all(|&s| s.to_bits() == 0));
        let mut r = Rechannel::new(3, 1);
        let (_, out) = run(&mut r, &ch, 10);
        assert_eq!(&out[0][..10], &ch[0][..10]);
    }

    #[test]
    fn empty_block_swaps_and_takes_the_input_count() {
        let mut r = Rechannel::new(2, 1);
        let (p, out) = run(&mut r, &vec![ramp(1.0); 4], 0);
        assert_eq!(p, Processed { channels: 1, swapped: true });
        assert_eq!(r.in_channels, 4);
        assert!(out[0][0].is_nan());
        let mut r = Rechannel::new(2, 1);
        run(&mut r, &vec![ramp(1.0); 4], 8);
        assert_eq!(r.in_channels, 2, "a non-empty block leaves +41 alone");
    }

    #[test]
    fn denormals_load_as_zero() {
        let mut r = Rechannel::new(3, 2);
        let d = vec![f32::from_bits(0x8000_0001); N];
        let (_, out) = run(&mut r, &[d.clone(), d.clone(), d], 4);
        assert_eq!(out[0][0].to_bits(), 0x8000_0000);
    }

    #[test]
    fn matrix_routes_stay_in_range() {
        for i in [1u8, 2, 4, 6, 8] {
            for o in [1u8, 2, 4, 6, 8] {
                let [a, b] = RANGES[i as usize * 8 + o as usize - 9];
                assert!(a <= b);
                let mut seen = [false; 8];
                for &e in &ROUTES[a as usize..=b as usize] {
                    let (ri, ro, g) = route(e);
                    assert!(ri < i as usize && ro < o as usize, "{i}->{o} route {e:#x}");
                    assert!(!seen[ro] || g == 1.0, "accumulations use gain 1");
                    seen[ro] = true;
                }
            }
        }
    }
}

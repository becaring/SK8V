// Port of FFmpeg 4.4 libavcodec/fft_template.c (float, C path:
// split-radix fft4/fft8/fft16/pass, FF_FFT_PERM_DEFAULT revtab with the
// fork's split_radix_permutation) and libavcodec/mdct_template.c
// (ff_imdct_half_c). Operation order is kept exactly; no fused operations.
// LGPL-2.1-or-later, see lib.rs.

use crate::dsptables::{DspTables, COS_MIN_BITS, MDCT_MIN_BITS};

/// split_radix_permutation as written in the rexglue-sdk fork.
fn split_radix_permutation(i: i32, mut n: i32, inverse: i32) -> i32 {
    let mut multiplier: i32 = 1;
    let mut v4: i32 = 0;
    while n > 2 {
        loop {
            let m = n >> 1;
            if (m & i) != 0 {
                break;
            }
            n >>= 1;
            multiplier *= 2;
            if m <= 2 {
                return v4 + (i & 1) * multiplier;
            }
        }
        n >>= 2;
        if inverse == ((n & i) == 0) as i32 {
            v4 += multiplier;
        } else {
            v4 -= multiplier;
        }
        multiplier *= 4;
    }
    v4 + (i & 1) * multiplier
}

pub struct Imdct {
    mdct_bits: u32,
    revtab: Vec<u16>,
    tcos: Vec<f32>,
    tsin: Vec<f32>,
}

/// Cosine tables ff_cos_16 .. by fft nbits.
pub struct CosTabs<'a> {
    tabs: &'a [Vec<f32>],
}

impl<'a> CosTabs<'a> {
    #[inline(always)]
    fn get(&self, nbits: u32) -> &'a [f32] {
        &self.tabs[(nbits - COS_MIN_BITS) as usize]
    }
}

const SQRTHALF: f32 = std::f64::consts::FRAC_1_SQRT_2 as f32;

// z is interleaved complex: re at 2k, im at 2k+1.
#[inline(always)]
fn re(z: &[f32], k: usize) -> f32 {
    z[2 * k]
}
#[inline(always)]
fn im(z: &[f32], k: usize) -> f32 {
    z[2 * k + 1]
}

/// BUTTERFLIES(a0,a1,a2,a3) with t1,t2,t5,t6 inputs.
#[inline(always)]
fn butterflies(z: &mut [f32], a0: usize, a1: usize, a2: usize, a3: usize, t1: f32, t2: f32, t5: f32, t6: f32) {
    // BF(t3, t5, t5, t1)
    let t3 = t5 - t1;
    let t5 = t5 + t1;
    // BF(a2.re, a0.re, a0.re, t5)
    let r0 = re(z, a0);
    z[2 * a2] = r0 - t5;
    z[2 * a0] = r0 + t5;
    // BF(a3.im, a1.im, a1.im, t3)
    let i1 = im(z, a1);
    z[2 * a3 + 1] = i1 - t3;
    z[2 * a1 + 1] = i1 + t3;
    // BF(t4, t6, t2, t6)
    let t4 = t2 - t6;
    let t6 = t2 + t6;
    // BF(a3.re, a1.re, a1.re, t4)
    let r1 = re(z, a1);
    z[2 * a3] = r1 - t4;
    z[2 * a1] = r1 + t4;
    // BF(a2.im, a0.im, a0.im, t6)
    let i0 = im(z, a0);
    z[2 * a2 + 1] = i0 - t6;
    z[2 * a0 + 1] = i0 + t6;
}

/// TRANSFORM(a0,a1,a2,a3,wre,wim)
#[inline(always)]
fn transform(z: &mut [f32], a0: usize, a1: usize, a2: usize, a3: usize, wre: f32, wim: f32) {
    // CMUL(t1, t2, a2.re, a2.im, wre, -wim)
    let (are, aim) = (re(z, a2), im(z, a2));
    let nwim = -wim;
    let t1 = are * wre - aim * nwim;
    let t2 = are * nwim + aim * wre;
    // CMUL(t5, t6, a3.re, a3.im, wre, wim)
    let (bre, bim) = (re(z, a3), im(z, a3));
    let t5 = bre * wre - bim * wim;
    let t6 = bre * wim + bim * wre;
    butterflies(z, a0, a1, a2, a3, t1, t2, t5, t6);
}

#[inline(always)]
fn transform_zero(z: &mut [f32], a0: usize, a1: usize, a2: usize, a3: usize) {
    let t1 = re(z, a2);
    let t2 = im(z, a2);
    let t5 = re(z, a3);
    let t6 = im(z, a3);
    butterflies(z, a0, a1, a2, a3, t1, t2, t5, t6);
}

/// PASS: z[0...8n-1], w[1...2n-1]
fn pass(z: &mut [f32], wre_tab: &[f32], n: usize) {
    let o1 = 2 * n;
    let o2 = 4 * n;
    let o3 = 6 * n;
    // wim = wre + o1
    let mut zo = 0usize;
    let mut wre = 0usize;
    let mut wim = o1;
    transform_zero(z, zo, zo + o1, zo + o2, zo + o3);
    transform(z, zo + 1, zo + o1 + 1, zo + o2 + 1, zo + o3 + 1, wre_tab[wre + 1], wre_tab[wim - 1]);
    let mut cnt = n - 1;
    loop {
        zo += 2;
        wre += 2;
        wim -= 2;
        transform(z, zo, zo + o1, zo + o2, zo + o3, wre_tab[wre], wre_tab[wim]);
        transform(z, zo + 1, zo + o1 + 1, zo + o2 + 1, zo + o3 + 1, wre_tab[wre + 1], wre_tab[wim - 1]);
        cnt -= 1;
        if cnt == 0 {
            break;
        }
    }
}

fn fft4(z: &mut [f32]) {
    // BF(t3, t1, z[0].re, z[1].re)
    let t3 = z[0] - z[2];
    let t1 = z[0] + z[2];
    // BF(t8, t6, z[3].re, z[2].re)
    let t8 = z[6] - z[4];
    let t6 = z[6] + z[4];
    // BF(z[2].re, z[0].re, t1, t6)
    z[4] = t1 - t6;
    z[0] = t1 + t6;
    // BF(t4, t2, z[0].im, z[1].im)
    let t4 = z[1] - z[3];
    let t2 = z[1] + z[3];
    // BF(t7, t5, z[2].im, z[3].im)
    let t7 = z[5] - z[7];
    let t5 = z[5] + z[7];
    // BF(z[3].im, z[1].im, t4, t8)
    z[7] = t4 - t8;
    z[3] = t4 + t8;
    // BF(z[3].re, z[1].re, t3, t7)
    z[6] = t3 - t7;
    z[2] = t3 + t7;
    // BF(z[2].im, z[0].im, t2, t5)
    z[5] = t2 - t5;
    z[1] = t2 + t5;
}

fn fft8(z: &mut [f32]) {
    fft4(z);
    // BF(t1, z[5].re, z[4].re, -z[5].re)
    let (a, b) = (z[8], -z[10]);
    let t1 = a - b;
    z[10] = a + b;
    // BF(t2, z[5].im, z[4].im, -z[5].im)
    let (a, b) = (z[9], -z[11]);
    let t2 = a - b;
    z[11] = a + b;
    // BF(t5, z[7].re, z[6].re, -z[7].re)
    let (a, b) = (z[12], -z[14]);
    let t5 = a - b;
    z[14] = a + b;
    // BF(t6, z[7].im, z[6].im, -z[7].im)
    let (a, b) = (z[13], -z[15]);
    let t6 = a - b;
    z[15] = a + b;

    butterflies(z, 0, 2, 4, 6, t1, t2, t5, t6);
    transform(z, 1, 3, 5, 7, SQRTHALF, SQRTHALF);
}

fn fft16(z: &mut [f32], cos16: &[f32]) {
    let cos_16_1 = cos16[1];
    let cos_16_3 = cos16[3];
    fft8(z);
    fft4(&mut z[16..]);
    fft4(&mut z[24..]);
    transform_zero(z, 0, 4, 8, 12);
    transform(z, 2, 6, 10, 14, SQRTHALF, SQRTHALF);
    transform(z, 1, 5, 9, 13, cos_16_1, cos_16_3);
    transform(z, 3, 7, 11, 15, cos_16_3, cos_16_1);
}

/// fft_dispatch[nbits-2](z)
fn fft(z: &mut [f32], nbits: u32, cos: &CosTabs) {
    match nbits {
        2 => fft4(z),
        3 => fft8(z),
        4 => fft16(z, cos.get(4)),
        _ => {
            // DECL_FFT(n, n2, n4)
            let n = 1usize << nbits;
            let n4 = n / 4;
            fft(z, nbits - 1, cos);
            fft(&mut z[2 * (n4 * 2)..], nbits - 2, cos);
            fft(&mut z[2 * (n4 * 3)..], nbits - 2, cos);
            pass(z, cos.get(nbits), n4 / 2);
        }
    }
}

impl Imdct {
    /// ff_mdct_init(s, nbits, inverse = 1, scale) with the baked twiddles.
    pub fn new(nbits: u32, tables: &DspTables) -> Self {
        let fft_bits = nbits - 2;
        let n = 1i32 << fft_bits;
        let mut revtab = vec![0u16; n as usize];
        for i in 0..n {
            let k = (-split_radix_permutation(i, n, 1)) & (n - 1);
            revtab[k as usize] = i as u16;
        }
        let (tcos, tsin) = tables.mdct[(nbits - MDCT_MIN_BITS) as usize].clone();
        Imdct { mdct_bits: nbits, revtab, tcos, tsin }
    }

    /// ff_imdct_half_c: output N/2 samples (interleaved as N/4 complex),
    /// input N/2 samples.
    pub fn imdct_half(&self, tables: &DspTables, output: &mut [f32], input: &[f32]) {
        let n = 1usize << self.mdct_bits;
        let n2 = n >> 1;
        let n4 = n >> 2;
        let n8 = n >> 3;
        let tcos = &self.tcos;
        let tsin = &self.tsin;
        let z = &mut output[..n2];

        // pre rotation
        let mut in1 = 0usize;
        let mut in2 = n2 - 1;
        for k in 0..n4 {
            let j = self.revtab[k] as usize;
            // CMUL(z[j].re, z[j].im, *in2, *in1, tcos[k], tsin[k])
            let (are, aim, bre, bim) = (input[in2], input[in1], tcos[k], tsin[k]);
            z[2 * j] = are * bre - aim * bim;
            z[2 * j + 1] = are * bim + aim * bre;
            in1 += 2;
            in2 = in2.wrapping_sub(2);
        }
        let cos = CosTabs { tabs: &tables.cos };
        fft(z, self.mdct_bits - 2, &cos);

        // post rotation + reordering
        for k in 0..n8 {
            let a = n8 - k - 1;
            let b = n8 + k;
            // CMUL(r0, i1, z[a].im, z[a].re, tsin[a], tcos[a])
            let (are, aim) = (z[2 * a + 1], z[2 * a]);
            let r0 = are * tsin[a] - aim * tcos[a];
            let i1 = are * tcos[a] + aim * tsin[a];
            // CMUL(r1, i0, z[b].im, z[b].re, tsin[b], tcos[b])
            let (bre, bim) = (z[2 * b + 1], z[2 * b]);
            let r1 = bre * tsin[b] - bim * tcos[b];
            let i0 = bre * tcos[b] + bim * tsin[b];
            z[2 * a] = r0;
            z[2 * a + 1] = i0;
            z[2 * b] = r1;
            z[2 * b + 1] = i1;
        }
    }
}

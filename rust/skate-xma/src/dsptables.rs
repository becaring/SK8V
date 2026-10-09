// Float tables of FFmpeg 4.4 (rexglue-sdk fork) as the reference build
// computes them at init: sine windows (libavcodec/sinewin_tablegen.h), FFT
// cosine tables (libavcodec/fft_template.c init_ff_cos_tabs) and the IMDCT
// twiddles (libavcodec/mdct_template.c ff_mdct_init, which in this fork uses
// the `cosfast` polynomial for tcos and libm sin for tsin).
// LGPL-2.1-or-later, see lib.rs.
//
// The values are baked into `dsptables_baked.rs` as f32 bit patterns, so the
// decoder does not depend on the host libm.

/// Smallest/largest MDCT size (log2 of the full transform) the XMA decoder
/// can use: subframes of 128, 256 and 512 samples.
pub const MDCT_MIN_BITS: u32 = 8;
pub const MDCT_MAX_BITS: u32 = 10;
/// Sine windows of 2^7 .. 2^9.
pub const WIN_MIN_BITS: u32 = 7;
/// FFT cosine tables ff_cos_16 .. ff_cos_256 (fft nbits 4..8).
pub const COS_MIN_BITS: u32 = 4;

pub struct DspTables {
    /// windows[k] = sine window of 2^(WIN_MIN_BITS + k) samples
    pub windows: Vec<Vec<f32>>,
    /// cos[k] = ff_cos_(2^(COS_MIN_BITS + k))
    pub cos: Vec<Vec<f32>>,
    /// mdct[k] = (tcos, tsin) for nbits MDCT_MIN_BITS + k
    pub mdct: Vec<(Vec<f32>, Vec<f32>)>,
}

impl DspTables {
    pub fn baked() -> Self {
        use crate::dsptables_baked as b;
        let f = |v: &[u32]| v.iter().map(|&x| f32::from_bits(x)).collect::<Vec<f32>>();
        DspTables {
            windows: b::WINDOWS.iter().map(|w| f(w)).collect(),
            cos: b::COS.iter().map(|w| f(w)).collect(),
            mdct: b::MDCT.iter().map(|(c, s)| (f(c), f(s))).collect(),
        }
    }
}

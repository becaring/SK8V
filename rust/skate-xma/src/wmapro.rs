// Port of FFmpeg 4.4 libavcodec/wmaprodec.c (rexglue-sdk/Xenia fork) as the
// `xmaframes` decoder runs it: decode_init for AV_CODEC_ID_XMAFRAMES,
// xmaframes_decode_packet, save_bits, decode_frame, decode_tilehdr,
// decode_subframe_length, decode_channel_transform (1-2 channels),
// decode_coeffs, decode_scale_factors, inverse_channel_transform,
// wmapro_window; plus libavcodec/wma.c ff_wma_run_level_decode /
// ff_wma_get_large_val, libavutil/float_dsp.c vector_fmul_scalar_c /
// vector_fmul_window_c and libavutil/ffmath.h ff_exp10 (the fork's
// fastexp2). Every float operation is in the C order (the oracle compiled
// this with clang -O2 -ffp-model=strict: no contraction, no reassociation).
// LGPL-2.1-or-later, see lib.rs.

use crate::dsptables::{DspTables, MDCT_MIN_BITS, WIN_MIN_BITS};
use crate::getbits::GetBits;
use crate::mdct::Imdct;
use crate::vlc::init_vlc;
use crate::wmaprodata::*;

const WMAPRO_MAX_CHANNELS: usize = 8;
const MAX_SUBFRAMES: usize = 32;
const MAX_BANDS: usize = 29;
const MAX_FRAMESIZE: usize = 32768;
const AV_INPUT_BUFFER_PADDING_SIZE: usize = 64;
const XMA_MAX_CHANNELS_STREAM: i32 = 2;

const WMAPRO_BLOCK_MIN_BITS: u32 = 6;
const WMAPRO_BLOCK_MAX_BITS: u32 = 13;
const WMAPRO_BLOCK_MAX_SIZE: usize = 1 << WMAPRO_BLOCK_MAX_BITS;
const WMAPRO_BLOCK_SIZES: usize = (WMAPRO_BLOCK_MAX_BITS - WMAPRO_BLOCK_MIN_BITS + 1) as usize;

const VLCBITS: u32 = 9;
const SCALEVLCBITS: u32 = 8;
const VEC4MAXDEPTH: u32 = (HUFF_VEC4_MAXBITS as u32).div_ceil(VLCBITS);
const VEC2MAXDEPTH: u32 = (HUFF_VEC2_MAXBITS as u32).div_ceil(VLCBITS);
const VEC1MAXDEPTH: u32 = (HUFF_VEC1_MAXBITS as u32).div_ceil(VLCBITS);
const SCALEMAXDEPTH: u32 = (HUFF_SCALE_MAXBITS as u32).div_ceil(SCALEVLCBITS);
const SCALERLMAXDEPTH: u32 = (HUFF_SCALE_RL_MAXBITS as u32).div_ceil(VLCBITS);
// wma.h: VLCBITS 9, VLCMAX ((22 + VLCBITS - 1) / VLCBITS)
const WMA_VLCMAX: u32 = 22u32.div_ceil(9);

/// av_log2
#[inline(always)]
fn av_log2(v: u32) -> i32 {
    if v == 0 { 0 } else { 31 - v.leading_zeros() as i32 }
}

/// The fork's fastexp2 (libavutil/ffmath.h).
#[inline(always)]
fn fastexp2(mut xmm0: f64) -> f64 {
    xmm0 = if xmm0 > -1022.0 { xmm0 } else { -1022.0 };
    let mut xmm2: f64 = 4.8425778448581696;
    let xmm3: f64 = 27.728333711624146;
    let mut xmm1 = xmm0.floor();
    xmm0 -= xmm1;
    xmm1 += 1017.2740579843521;
    xmm2 -= xmm0;
    xmm1 += xmm0 * -0.49013227410614491;
    xmm2 = xmm3 / xmm2;
    xmm0 = xmm1 + xmm2;
    xmm0 *= 4503599627370496.0;
    // (signed long long)(xmm0): cvttsd2si, 0x8000000000000000 when out of range
    let r0: i64 = if (-9.223372036854775808e18..9.223372036854775808e18).contains(&xmm0) {
        xmm0 as i64
    } else {
        i64::MIN
    };
    f64::from_bits(r0 as u64)
}

/// ff_exp10
#[inline(always)]
fn ff_exp10(x: f64) -> f64 {
    const M_LOG2_10: f64 = 3.32192809488736234787;
    fastexp2(M_LOG2_10 * x)
}

/// vector_fmul_scalar_c
#[inline(always)]
fn vector_fmul_scalar(dst: &mut [f32], src: &[f32], mul: f32, len: usize) {
    for i in 0..len {
        dst[i] = src[i] * mul;
    }
}

/// Static VLC tables (INIT_VLC_STATIC in decode_init).
pub struct Vlcs {
    sf: Vec<[i16; 2]>,
    sf_rl: Vec<[i16; 2]>,
    coef: [Vec<[i16; 2]>; 2],
    vec4: Vec<[i16; 2]>,
    vec2: Vec<[i16; 2]>,
    vec1: Vec<[i16; 2]>,
}

impl Vlcs {
    pub fn new() -> Self {
        let w16 = |v: &[u16]| v.iter().map(|&x| x as u32).collect::<Vec<u32>>();
        Vlcs {
            sf: init_vlc(SCALEVLCBITS, &SCALE_HUFFBITS, &w16(&SCALE_HUFFCODES)),
            sf_rl: init_vlc(VLCBITS, &SCALE_RL_HUFFBITS, &SCALE_RL_HUFFCODES),
            coef: [
                init_vlc(VLCBITS, &COEF0_HUFFBITS, &COEF0_HUFFCODES),
                init_vlc(VLCBITS, &COEF1_HUFFBITS, &COEF1_HUFFCODES),
            ],
            vec4: init_vlc(VLCBITS, &VEC4_HUFFBITS, &w16(&VEC4_HUFFCODES)),
            vec2: init_vlc(VLCBITS, &VEC2_HUFFBITS, &w16(&VEC2_HUFFCODES)),
            vec1: init_vlc(VLCBITS, &VEC1_HUFFBITS, &w16(&VEC1_HUFFCODES)),
        }
    }
}

/// Shared immutable state (VLCs, float tables, IMDCTs).
pub struct Shared {
    vlcs: Vlcs,
    tables: DspTables,
    /// mdct[k] for nbits MDCT_MIN_BITS + k
    mdct: Vec<Imdct>,
}

impl Shared {
    pub fn new() -> Self {
        let tables = DspTables::baked();
        let mdct = (MDCT_MIN_BITS..=crate::dsptables::MDCT_MAX_BITS).map(|b| Imdct::new(b, &tables)).collect();
        Shared { vlcs: Vlcs::new(), tables, mdct }
    }

}

#[derive(Clone)]
struct ChannelCtx {
    prev_block_len: i16,
    transmit_coefs: u8,
    num_subframes: u8,
    subframe_len: [u16; MAX_SUBFRAMES],
    subframe_offset: [u16; MAX_SUBFRAMES],
    cur_subframe: u8,
    decoded_samples: u16,
    grouped: u8,
    quant_step: i32,
    reuse_sf: i8,
    scale_factor_step: i8,
    max_scale_factor: i32,
    saved_scale_factors: [[i32; MAX_BANDS]; 2],
    scale_factor_idx: i8,
    /// which saved_scale_factors buffer `scale_factors` points at
    scale_factors: usize,
    table_idx: u8,
    /// offset of `coeffs` in `out`
    coeffs: usize,
    num_vec_coeffs: u16,
    out: Vec<f32>,
}

impl ChannelCtx {
    fn new() -> Self {
        ChannelCtx {
            prev_block_len: 0,
            transmit_coefs: 0,
            num_subframes: 0,
            subframe_len: [0; MAX_SUBFRAMES],
            subframe_offset: [0; MAX_SUBFRAMES],
            cur_subframe: 0,
            decoded_samples: 0,
            grouped: 0,
            quant_step: 0,
            reuse_sf: 0,
            scale_factor_step: 0,
            max_scale_factor: 0,
            saved_scale_factors: [[0; MAX_BANDS]; 2],
            scale_factor_idx: 0,
            scale_factors: 0,
            table_idx: 0,
            coeffs: 0,
            num_vec_coeffs: 0,
            out: vec![0.0; WMAPRO_BLOCK_MAX_SIZE + WMAPRO_BLOCK_MAX_SIZE / 2],
        }
    }
}

#[derive(Clone)]
struct ChannelGrp {
    num_channels: u8,
    transform: i8,
    transform_band: [i8; MAX_BANDS],
    decorrelation_matrix: [f32; WMAPRO_MAX_CHANNELS * WMAPRO_MAX_CHANNELS],
    /// channel indices (the C keeps pointers to their coeffs)
    channel_data: [usize; WMAPRO_MAX_CHANNELS],
}

/// Error-level messages FFmpeg would have logged (xmadec counts these).
#[derive(Default, Debug, Clone, Copy)]
pub struct AvErrors(pub u32);

/// One `xmaframes` decoder instance (AVCodecContext + WMAProDecodeCtx).
pub struct XmaFrames {
    pub sample_rate: i32,
    pub channels: i32,
    decode_flags: u32,
    len_prefix: bool,
    dynamic_range_compression: bool,
    bits_per_sample: u8,
    samples_per_frame: u16,
    log2_frame_size: u16,
    lfe_channel: i8,
    max_num_subframes: u8,
    subframe_len_bits: u8,
    max_subframe_len_bit: u8,
    min_samples_per_subframe: u16,
    num_sfb: [i8; WMAPRO_BLOCK_SIZES],
    sfb_offsets: [[i16; MAX_BANDS]; WMAPRO_BLOCK_SIZES],
    sf_offsets: [[[i8; MAX_BANDS]; WMAPRO_BLOCK_SIZES]; WMAPRO_BLOCK_SIZES],
    subwoofer_cutoffs: [i16; WMAPRO_BLOCK_SIZES],

    frame_data: Vec<u8>,
    num_saved_bits: i32,
    frame_offset: i32,
    subframe_offset: i32,
    packet_loss: u8,
    frame_num: u32,
    drc_gain: u8,
    skip_frame: i8,
    parsed_all_subframes: i8,

    subframe_len: i16,
    nb_channels: i8,
    channels_for_cur_subframe: i8,
    channel_indexes_for_cur_subframe: [i8; WMAPRO_MAX_CHANNELS],
    num_bands: i8,
    transmit_num_vec_coeffs: i8,
    table_idx: u8,
    esc_len: i8,
    num_chgroups: u8,
    chgroup: Vec<ChannelGrp>,
    channel: Vec<ChannelCtx>,
    tmp: Vec<f32>,

    pub errors: AvErrors,
}

/// One decoded 512-sample frame, planar.
pub struct Frame {
    pub ch: [Vec<f32>; 2],
}

pub enum PacketError {
    /// xmaframes_decode_packet returned AVERROR_INVALIDDATA
    InvalidData,
}

impl XmaFrames {
    /// avcodec_open2 of AV_CODEC_ID_XMAFRAMES with the given rate/channels
    /// (decode_init).
    pub fn new(sample_rate: i32, channels: i32) -> Result<Self, String> {
        let decode_flags: u32 = 0x10d6;
        let bits_per_sample: u8 = 16;
        let nb_channels = channels;
        let block_align = 2048u32;
        let log2_frame_size = (av_log2(block_align) + 4) as u16;
        let len_prefix = decode_flags & 0x40 != 0;
        let samples_per_frame: u16 = 512;
        let log2_max_num_subframes = ((decode_flags & 0x38) >> 3) as i32;
        let max_num_subframes: u8 = 1 << log2_max_num_subframes;
        let max_subframe_len_bit = (max_num_subframes == 16 || max_num_subframes == 4) as u8;
        let subframe_len_bits = (av_log2(log2_max_num_subframes as u32) + 1) as u8;
        let num_possible_block_sizes = (log2_max_num_subframes + 1) as usize;
        let min_samples_per_subframe = samples_per_frame / max_num_subframes as u16;
        let dynamic_range_compression = decode_flags & 0x80 != 0;
        if sample_rate <= 0 {
            return Err("invalid sample rate".into());
        }
        if nb_channels <= 0 || nb_channels > XMA_MAX_CHANNELS_STREAM {
            return Err(format!("invalid number of channels per XMA stream {nb_channels}"));
        }
        let mut s = XmaFrames {
            sample_rate,
            channels,
            decode_flags,
            len_prefix,
            dynamic_range_compression,
            bits_per_sample,
            samples_per_frame,
            log2_frame_size,
            lfe_channel: -1,
            max_num_subframes,
            subframe_len_bits,
            max_subframe_len_bit,
            min_samples_per_subframe,
            num_sfb: [0; WMAPRO_BLOCK_SIZES],
            sfb_offsets: [[0; MAX_BANDS]; WMAPRO_BLOCK_SIZES],
            sf_offsets: [[[0; MAX_BANDS]; WMAPRO_BLOCK_SIZES]; WMAPRO_BLOCK_SIZES],
            subwoofer_cutoffs: [0; WMAPRO_BLOCK_SIZES],
            frame_data: vec![0u8; MAX_FRAMESIZE + AV_INPUT_BUFFER_PADDING_SIZE],
            num_saved_bits: 0,
            frame_offset: 0,
            subframe_offset: 0,
            packet_loss: 1,
            frame_num: 0,
            drc_gain: 0,
            skip_frame: 0,
            parsed_all_subframes: 0,
            subframe_len: 0,
            nb_channels: nb_channels as i8,
            channels_for_cur_subframe: 0,
            channel_indexes_for_cur_subframe: [0; WMAPRO_MAX_CHANNELS],
            num_bands: 0,
            transmit_num_vec_coeffs: 0,
            table_idx: 0,
            esc_len: 0,
            num_chgroups: 0,
            chgroup: vec![
                ChannelGrp {
                    num_channels: 0,
                    transform: 0,
                    transform_band: [0; MAX_BANDS],
                    decorrelation_matrix: [0.0; 64],
                    channel_data: [0; WMAPRO_MAX_CHANNELS],
                };
                WMAPRO_MAX_CHANNELS
            ],
            channel: vec![ChannelCtx::new(); WMAPRO_MAX_CHANNELS],
            tmp: vec![0.0; WMAPRO_BLOCK_MAX_SIZE],
            errors: AvErrors::default(),
        };
        for i in 0..nb_channels as usize {
            s.channel[i].prev_block_len = samples_per_frame as i16;
        }
        // get_rate (codec_id != WMAPRO)
        let rate = if sample_rate > 44100 {
            48000
        } else if sample_rate > 32000 {
            44100
        } else if sample_rate > 24000 {
            32000
        } else {
            24000
        };
        for i in 0..num_possible_block_sizes {
            let subframe_len = (samples_per_frame as i32) >> i;
            let mut band = 1usize;
            s.sfb_offsets[i][0] = 0;
            let mut x = 0usize;
            while x < MAX_BANDS - 1 && (s.sfb_offsets[i][band - 1] as i32) < subframe_len {
                let mut offset = (subframe_len * 2 * CRITICAL_FREQ[x] as i32) / rate + 2;
                offset &= !3;
                if offset > s.sfb_offsets[i][band - 1] as i32 {
                    s.sfb_offsets[i][band] = offset as i16;
                    band += 1;
                }
                if offset >= subframe_len {
                    break;
                }
                x += 1;
            }
            s.sfb_offsets[i][band - 1] = subframe_len as i16;
            s.num_sfb[i] = (band - 1) as i8;
            if s.num_sfb[i] <= 0 {
                return Err("num_sfb invalid".into());
            }
        }
        for i in 0..num_possible_block_sizes {
            for b in 0..s.num_sfb[i] as usize {
                let offset = ((s.sfb_offsets[i][b] as i32 + s.sfb_offsets[i][b + 1] as i32 - 1) << i) >> 1;
                for x in 0..num_possible_block_sizes {
                    let mut v = 0usize;
                    while ((s.sfb_offsets[x][v + 1] as i32) << x) < offset {
                        v += 1;
                        assert!(v < MAX_BANDS);
                    }
                    s.sf_offsets[i][x][b] = v as i8;
                }
            }
        }
        for i in 0..num_possible_block_sizes {
            let block_size = (samples_per_frame as i64) >> i;
            let cutoff = (440 * block_size + 3i64 * (sample_rate as i64 >> 1) - 1) / sample_rate as i64;
            s.subwoofer_cutoffs[i] = cutoff.clamp(4, block_size) as i16;
        }
        let _ = s.decode_flags;
        Ok(s)
    }

    /// xmaframes_decode_packet. `pkt` must contain the packet bytes followed
    /// by the bytes FFmpeg would see after it (xmadec's zero-filled
    /// 4097-byte xma_frame buffer); `size` is avpkt->size.
    pub fn decode_packet(&mut self, sh: &Shared, pkt: &[u8], size: usize) -> Result<Option<Frame>, PacketError> {
        if size < 3 {
            self.errors.0 += 1;
            return Err(PacketError::InvalidData);
        }
        let buf_bit_size = (size << 3) as i32;
        let mut gb = GetBits::new(pkt, buf_bit_size);
        let padding_start = gb.get_bits(3) as u8;
        let padding_end = gb.get_bits(3) as u8;
        gb.skip_bits(2);
        gb.skip_bits(padding_start as i32);
        let xma_frame_len = gb.show_bits(self.log2_frame_size as u32) as i32;
        if buf_bit_size != 8 + padding_start as i32 + xma_frame_len + padding_end as i32 {
            self.errors.0 += 1;
            return Err(PacketError::InvalidData);
        }
        // save_bits(s, gb, xma_frame_len, 0)
        let len = xma_frame_len;
        self.frame_offset = gb.get_bits_count() & 7;
        self.num_saved_bits = self.frame_offset;
        let buflen = (self.num_saved_bits + len + 7) >> 3;
        if len <= 0 || buflen as usize > MAX_FRAMESIZE {
            // avpriv_request_sample + packet_loss; s->gb would still read
            // the packet. Never happens for frames xmadec accepts.
            self.packet_loss = 1;
            return Err(PacketError::InvalidData);
        }
        self.num_saved_bits += len;
        let src = (gb.get_bits_count() >> 3) as usize;
        let nbytes = ((self.num_saved_bits + 7) >> 3) as usize;
        self.frame_data[..nbytes].copy_from_slice(&pkt[src..src + nbytes]);
        let rem = self.num_saved_bits & 7;
        if rem != 0 {
            self.frame_data[nbytes - 1] &= 0xFFu8 << (8 - rem);
        }
        // decode_frame on s->gb = frame_data
        let fd = std::mem::take(&mut self.frame_data);
        let mut fgb = GetBits::new(&fd, self.num_saved_bits);
        fgb.skip_bits(self.frame_offset);
        let mut frame = Frame { ch: [vec![0.0; 512], Vec::new()] };
        if self.nb_channels > 1 {
            frame.ch[1] = vec![0.0; 512];
        }
        let got = self.decode_frame(sh, &mut fgb, &mut frame);
        self.frame_data = fd;
        Ok(if got { Some(frame) } else { None })
    }

    /// decode_frame; returns got_frame.
    fn decode_frame(&mut self, sh: &Shared, gb: &mut GetBits, frame: &mut Frame) -> bool {
        let mut len = 0i32;
        if self.len_prefix {
            len = gb.get_bits(self.log2_frame_size as u32) as i32;
        }
        if self.decode_tilehdr(gb).is_err() {
            self.packet_loss = 1;
            return false;
        }
        if self.nb_channels > 1 && gb.get_bits1() != 0 && gb.get_bits1() != 0 {
            for _ in 0..(self.nb_channels as i32 * self.nb_channels as i32) {
                gb.skip_bits(4);
            }
        }
        if self.dynamic_range_compression {
            self.drc_gain = gb.get_bits(8) as u8;
        }
        if gb.get_bits1() != 0 {
            let nb = av_log2(self.samples_per_frame as u32 * 2) as u32;
            if gb.get_bits1() != 0 {
                let _skip = gb.get_bits(nb);
            }
            if gb.get_bits1() != 0 {
                let _skip = gb.get_bits(nb);
            }
        }
        self.parsed_all_subframes = 0;
        for i in 0..self.nb_channels as usize {
            self.channel[i].decoded_samples = 0;
            self.channel[i].cur_subframe = 0;
            self.channel[i].reuse_sf = 0;
        }
        while self.parsed_all_subframes == 0 {
            if self.decode_subframe(sh, gb).is_err() {
                self.packet_loss = 1;
                return false;
            }
        }
        let spf = self.samples_per_frame as usize;
        for i in 0..self.nb_channels as usize {
            frame.ch[i][..spf].copy_from_slice(&self.channel[i].out[..spf]);
        }
        for i in 0..self.nb_channels as usize {
            self.channel[i].out.copy_within(spf..spf + (spf >> 1), 0);
        }
        let got_frame = if self.skip_frame != 0 {
            self.skip_frame = 0;
            false
        } else {
            true
        };
        if self.len_prefix {
            if len != (gb.get_bits_count() - self.frame_offset) + 2 {
                // "frame[%u] would have to skip %i bits"
                self.errors.0 += 1;
                self.packet_loss = 1;
                return got_frame;
            }
            gb.skip_bits_long(len - (gb.get_bits_count() - self.frame_offset) - 1);
        } else {
            while gb.get_bits_count() < self.num_saved_bits && gb.get_bits1() == 0 {}
        }
        let _more_frames = gb.get_bits1();
        self.frame_num = self.frame_num.wrapping_add(1);
        got_frame
    }

    fn decode_subframe_length(&mut self, gb: &mut GetBits, offset: i32) -> Result<i32, ()> {
        let mut frame_len_shift = 0i32;
        if offset == self.samples_per_frame as i32 - self.min_samples_per_subframe as i32 {
            return Ok(self.min_samples_per_subframe as i32);
        }
        if gb.get_bits_left() < 1 {
            return Err(());
        }
        if self.max_subframe_len_bit != 0 {
            if gb.get_bits1() != 0 {
                frame_len_shift = 1 + gb.get_bits(self.subframe_len_bits as u32 - 1) as i32;
            }
        } else {
            frame_len_shift = gb.get_bits(self.subframe_len_bits as u32) as i32;
        }
        let subframe_len = (self.samples_per_frame as i32) >> frame_len_shift;
        if subframe_len < self.min_samples_per_subframe as i32 || subframe_len > self.samples_per_frame as i32 {
            self.errors.0 += 1;
            return Err(());
        }
        Ok(subframe_len)
    }

    fn decode_tilehdr(&mut self, gb: &mut GetBits) -> Result<(), ()> {
        let mut num_samples = [0u16; WMAPRO_MAX_CHANNELS];
        let mut contains_subframe = [0u8; WMAPRO_MAX_CHANNELS];
        let mut channels_for_cur_subframe = self.nb_channels as i32;
        let mut fixed_channel_layout = false;
        let mut min_channel_len = 0i32;
        let nb = self.nb_channels as usize;
        for c in 0..nb {
            self.channel[c].num_subframes = 0;
        }
        if self.max_num_subframes == 1 || gb.get_bits1() != 0 {
            fixed_channel_layout = true;
        }
        loop {
            for c in 0..nb {
                if num_samples[c] as i32 == min_channel_len {
                    if fixed_channel_layout
                        || channels_for_cur_subframe == 1
                        || min_channel_len == self.samples_per_frame as i32 - self.min_samples_per_subframe as i32
                    {
                        contains_subframe[c] = 1;
                    } else {
                        contains_subframe[c] = gb.get_bits1() as u8;
                    }
                } else {
                    contains_subframe[c] = 0;
                }
            }
            let subframe_len = match self.decode_subframe_length(gb, min_channel_len) {
                Ok(v) if v > 0 => v,
                _ => return Err(()),
            };
            min_channel_len += subframe_len;
            for c in 0..nb {
                if contains_subframe[c] != 0 {
                    let chan = &mut self.channel[c];
                    if chan.num_subframes as usize >= MAX_SUBFRAMES {
                        self.errors.0 += 1;
                        return Err(());
                    }
                    chan.subframe_len[chan.num_subframes as usize] = subframe_len as u16;
                    num_samples[c] = num_samples[c].wrapping_add(subframe_len as u16);
                    chan.num_subframes += 1;
                    if num_samples[c] as i32 > self.samples_per_frame as i32 {
                        self.errors.0 += 1;
                        return Err(());
                    }
                } else if num_samples[c] as i32 <= min_channel_len {
                    if (num_samples[c] as i32) < min_channel_len {
                        channels_for_cur_subframe = 0;
                        min_channel_len = num_samples[c] as i32;
                    }
                    channels_for_cur_subframe += 1;
                }
            }
            if min_channel_len >= self.samples_per_frame as i32 {
                break;
            }
        }
        for c in 0..nb {
            let mut offset = 0u16;
            for i in 0..self.channel[c].num_subframes as usize {
                self.channel[c].subframe_offset[i] = offset;
                offset = offset.wrapping_add(self.channel[c].subframe_len[i]);
            }
        }
        Ok(())
    }

    fn decode_channel_transform(&mut self, gb: &mut GetBits) -> Result<(), ()> {
        self.num_chgroups = 0;
        if self.nb_channels > 1 {
            let mut remaining_channels = self.channels_for_cur_subframe as i32;
            if gb.get_bits1() != 0 {
                // avpriv_request_sample "Channel transform bit"
                return Err(());
            }
            self.num_chgroups = 0;
            while remaining_channels != 0 && (self.num_chgroups as i32) < self.channels_for_cur_subframe as i32 {
                let g = self.num_chgroups as usize;
                let mut cd = 0usize;
                self.chgroup[g].num_channels = 0;
                self.chgroup[g].transform = 0;
                if remaining_channels > 2 {
                    for i in 0..self.channels_for_cur_subframe as usize {
                        let channel_idx = self.channel_indexes_for_cur_subframe[i] as usize;
                        if self.channel[channel_idx].grouped == 0 && gb.get_bits1() != 0 {
                            self.chgroup[g].num_channels += 1;
                            self.channel[channel_idx].grouped = 1;
                            self.chgroup[g].channel_data[cd] = channel_idx;
                            cd += 1;
                        }
                    }
                } else {
                    self.chgroup[g].num_channels = remaining_channels as u8;
                    for i in 0..self.channels_for_cur_subframe as usize {
                        let channel_idx = self.channel_indexes_for_cur_subframe[i] as usize;
                        if self.channel[channel_idx].grouped == 0 {
                            self.chgroup[g].channel_data[cd] = channel_idx;
                            cd += 1;
                        }
                        self.channel[channel_idx].grouped = 1;
                    }
                }
                if self.chgroup[g].num_channels == 2 {
                    if gb.get_bits1() != 0 {
                        if gb.get_bits1() != 0 {
                            // "Unknown channel transform type"
                            return Err(());
                        }
                    } else {
                        self.chgroup[g].transform = 1;
                        let m = &mut self.chgroup[g].decorrelation_matrix;
                        if self.nb_channels == 2 {
                            m[0] = 1.0;
                            m[1] = -1.0;
                            m[2] = 1.0;
                            m[3] = 1.0;
                        } else {
                            m[0] = 0.70703125;
                            m[1] = -0.70703125;
                            m[2] = 0.70703125;
                            m[3] = 0.70703125;
                        }
                    }
                } else if self.chgroup[g].num_channels > 2 {
                    // Only reachable with > 2 channels per stream, which
                    // XMA (and decode_init) rules out.
                    unreachable!("more than 2 coupled channels in an XMA stream");
                }
                if self.chgroup[g].transform != 0 {
                    if gb.get_bits1() == 0 {
                        for i in 0..self.num_bands as usize {
                            self.chgroup[g].transform_band[i] = gb.get_bits1() as i8;
                        }
                    } else {
                        for i in 0..self.num_bands as usize {
                            self.chgroup[g].transform_band[i] = 1;
                        }
                    }
                }
                remaining_channels -= self.chgroup[g].num_channels as i32;
                self.num_chgroups += 1;
            }
        }
        Ok(())
    }

    fn decode_coeffs(&mut self, sh: &Shared, gb: &mut GetBits, c: usize) -> i32 {
        const FVAL_TAB: [u32; 16] = [
            0x00000000, 0x3f800000, 0x40000000, 0x40400000, 0x40800000, 0x40a00000, 0x40c00000, 0x40e00000,
            0x41000000, 0x41100000, 0x41200000, 0x41300000, 0x41400000, 0x41500000, 0x41600000, 0x41700000,
        ];
        let mut rl_mode = false;
        let mut cur_coeff = 0i32;
        let mut num_zeros = 0i32;
        let vlctable = gb.get_bits1() as usize;
        let vlc = &sh.vlcs.coef[vlctable];
        let (run, level): (&[u16], &[f32]) =
            if vlctable != 0 { (&COEF1_RUN, &COEF1_LEVEL) } else { (&COEF0_RUN, &COEF0_LEVEL) };
        let subframe_len = self.subframe_len as i32;
        let ci = &mut self.channel[c];
        let co = ci.coeffs;
        while (self.transmit_num_vec_coeffs != 0 || !rl_mode) && (cur_coeff + 3 < ci.num_vec_coeffs as i32) {
            let mut vals = [0u32; 4];
            let mut idx = gb.get_vlc2(&sh.vlcs.vec4, VLCBITS, VEC4MAXDEPTH) as u32;
            if idx == HUFF_VEC4_SIZE as u32 - 1 {
                let mut i = 0;
                while i < 4 {
                    idx = gb.get_vlc2(&sh.vlcs.vec2, VLCBITS, VEC2MAXDEPTH) as u32;
                    if idx == HUFF_VEC2_SIZE as u32 - 1 {
                        let mut v0 = gb.get_vlc2(&sh.vlcs.vec1, VLCBITS, VEC1MAXDEPTH) as u32;
                        if v0 == HUFF_VEC1_SIZE as u32 - 1 {
                            v0 = v0.wrapping_add(ff_wma_get_large_val(gb));
                        }
                        let mut v1 = gb.get_vlc2(&sh.vlcs.vec1, VLCBITS, VEC1MAXDEPTH) as u32;
                        if v1 == HUFF_VEC1_SIZE as u32 - 1 {
                            v1 = v1.wrapping_add(ff_wma_get_large_val(gb));
                        }
                        vals[i] = (v0 as f32).to_bits();
                        vals[i + 1] = (v1 as f32).to_bits();
                    } else {
                        vals[i] = FVAL_TAB[(SYMBOL_TO_VEC2[idx as usize] >> 4) as usize];
                        vals[i + 1] = FVAL_TAB[(SYMBOL_TO_VEC2[idx as usize] & 0xF) as usize];
                    }
                    i += 2;
                }
            } else {
                let s = SYMBOL_TO_VEC4[idx as usize];
                vals[0] = FVAL_TAB[(s >> 12) as usize];
                vals[1] = FVAL_TAB[((s >> 8) & 0xF) as usize];
                vals[2] = FVAL_TAB[((s >> 4) & 0xF) as usize];
                vals[3] = FVAL_TAB[(s & 0xF) as usize];
            }
            for i in 0..4 {
                if vals[i] != 0 {
                    let sign = gb.get_bits1().wrapping_sub(1);
                    ci.out[co + cur_coeff as usize] = f32::from_bits(vals[i] ^ (sign << 31));
                    num_zeros = 0;
                } else {
                    ci.out[co + cur_coeff as usize] = 0.0;
                    num_zeros += 1;
                    rl_mode |= num_zeros > (subframe_len >> 8);
                }
                cur_coeff += 1;
            }
        }
        if cur_coeff < subframe_len {
            for v in &mut ci.out[co + cur_coeff as usize..co + subframe_len as usize] {
                *v = 0.0;
            }
            if ff_wma_run_level_decode(
                gb,
                vlc,
                level,
                run,
                &mut ci.out[co..co + subframe_len as usize],
                cur_coeff,
                subframe_len,
                subframe_len,
                self.esc_len as u32,
                &mut self.errors,
            ) != 0
            {
                return -1;
            }
        }
        0
    }

    fn decode_scale_factors(&mut self, sh: &Shared, gb: &mut GetBits) -> Result<(), ()> {
        let num_bands = self.num_bands as usize;
        for i in 0..self.channels_for_cur_subframe as usize {
            let c = self.channel_indexes_for_cur_subframe[i] as usize;
            let table_idx = self.table_idx as usize;
            let ch = &mut self.channel[c];
            ch.scale_factors = (ch.scale_factor_idx == 0) as usize;
            let cur = ch.scale_factors;
            if ch.reuse_sf != 0 {
                let sf_offsets = &self.sf_offsets[table_idx][ch.table_idx as usize];
                let src = ch.scale_factor_idx as usize;
                for b in 0..num_bands {
                    ch.saved_scale_factors[cur][b] = ch.saved_scale_factors[src][sf_offsets[b] as usize];
                }
            }
            if ch.cur_subframe == 0 || gb.get_bits1() != 0 {
                if ch.reuse_sf == 0 {
                    ch.scale_factor_step = (gb.get_bits(2) + 1) as i8;
                    let mut val: i32 = 45 / ch.scale_factor_step as i32;
                    for b in 0..num_bands {
                        val += gb.get_vlc2(&sh.vlcs.sf, SCALEVLCBITS, SCALEMAXDEPTH) - 60;
                        ch.saved_scale_factors[cur][b] = val;
                    }
                } else {
                    let mut i = 0usize;
                    while i < num_bands {
                        let idx = gb.get_vlc2(&sh.vlcs.sf_rl, VLCBITS, SCALERLMAXDEPTH);
                        let skip: i32;
                        let val: i32;
                        let sign: i32;
                        if idx == 0 {
                            let code = gb.get_bits(14);
                            val = (code >> 6) as i32;
                            sign = (code & 1) as i32 - 1;
                            skip = ((code & 0x3f) >> 1) as i32;
                        } else if idx == 1 {
                            break;
                        } else {
                            skip = SCALE_RL_RUN[idx as usize] as i32;
                            val = SCALE_RL_LEVEL[idx as usize] as i32;
                            sign = gb.get_bits1() as i32 - 1;
                        }
                        let ni = i as i32 + skip;
                        if ni >= num_bands as i32 {
                            self.errors.0 += 1;
                            return Err(());
                        }
                        i = ni as usize;
                        ch.saved_scale_factors[cur][i] =
                            ch.saved_scale_factors[cur][i].wrapping_add((val ^ sign).wrapping_sub(sign));
                        i += 1;
                    }
                }
                ch.scale_factor_idx = (ch.scale_factor_idx == 0) as i8;
                ch.table_idx = self.table_idx;
                ch.reuse_sf = 1;
            }
            let sf = &ch.saved_scale_factors[ch.scale_factors];
            ch.max_scale_factor = sf[0];
            for b in 1..num_bands {
                ch.max_scale_factor = ch.max_scale_factor.max(sf[b]);
            }
        }
        Ok(())
    }

    fn inverse_channel_transform(&mut self) {
        for i in 0..self.num_chgroups as usize {
            if self.chgroup[i].transform != 0 {
                let num_channels = self.chgroup[i].num_channels as usize;
                let grp = self.chgroup[i].clone();
                let cur = self.table_idx as usize;
                for (bi, b) in (0..self.num_bands as usize).enumerate() {
                    let s0 = self.sfb_offsets[cur][b] as i32;
                    let s1 = self.sfb_offsets[cur][b + 1] as i32;
                    let end = s1.min(self.subframe_len as i32);
                    if grp.transform_band[bi] == 1 {
                        let mut data = [0f32; WMAPRO_MAX_CHANNELS];
                        let mut y = s0;
                        while y < end {
                            for (k, &ch) in grp.channel_data[..num_channels].iter().enumerate() {
                                let co = self.channel[ch].coeffs;
                                data[k] = self.channel[ch].out[co + y as usize];
                            }
                            let mut mat = 0usize;
                            for &ch in &grp.channel_data[..num_channels] {
                                let mut sum: f32 = 0.0;
                                for k in 0..num_channels {
                                    sum += data[k] * grp.decorrelation_matrix[mat];
                                    mat += 1;
                                }
                                let co = self.channel[ch].coeffs;
                                self.channel[ch].out[co + y as usize] = sum;
                            }
                            y += 1;
                        }
                    } else if self.nb_channels == 2 {
                        let len = end - s0;
                        if len > 0 {
                            for k in 0..2 {
                                let ch = grp.channel_data[k];
                                let co = self.channel[ch].coeffs + s0 as usize;
                                let v = &mut self.channel[ch].out[co..co + len as usize];
                                for x in v.iter_mut() {
                                    *x *= (181.0f64 / 128.0) as f32;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn wmapro_window(&mut self, sh: &Shared) {
        for i in 0..self.channels_for_cur_subframe as usize {
            let c = self.channel_indexes_for_cur_subframe[i] as usize;
            let ch = &mut self.channel[c];
            let mut winlen = ch.prev_block_len as i32;
            let mut start = ch.coeffs as i32 - (winlen >> 1);
            if (self.subframe_len as i32) < winlen {
                start += (winlen - self.subframe_len as i32) >> 1;
                winlen = self.subframe_len as i32;
            }
            let window = &sh.tables.windows[(av_log2(winlen as u32) - WIN_MIN_BITS as i32) as usize];
            winlen >>= 1;
            // vector_fmul_window(start, start, start + winlen, window, winlen)
            let len = winlen as usize;
            let base = start as usize;
            let out = &mut ch.out;
            // dst += len; win += len; src0 += len;
            let mut i: isize = -(len as isize);
            let mut j: usize = len - 1;
            while i < 0 {
                let di = (base as isize + len as isize + i) as usize;
                let dj = base + len + j;
                let s0 = out[di];
                let s1 = out[base + len + j];
                let wi = window[(len as isize + i) as usize];
                let wj = window[len + j];
                out[di] = s0 * wj - s1 * wi;
                out[dj] = s0 * wi + s1 * wj;
                i += 1;
                j = j.wrapping_sub(1);
            }
            ch.prev_block_len = self.subframe_len;
        }
    }

    fn decode_subframe(&mut self, sh: &Shared, gb: &mut GetBits) -> Result<(), ()> {
        let mut offset = self.samples_per_frame as i32;
        let mut subframe_len = self.samples_per_frame as i32;
        let nb = self.nb_channels as usize;
        let mut total_samples = self.samples_per_frame as i32 * self.nb_channels as i32;
        let mut transmit_coeffs = false;

        self.subframe_offset = gb.get_bits_count();

        for i in 0..nb {
            self.channel[i].grouped = 0;
            if offset > self.channel[i].decoded_samples as i32 {
                offset = self.channel[i].decoded_samples as i32;
                subframe_len = self.channel[i].subframe_len[self.channel[i].cur_subframe as usize] as i32;
            }
        }

        self.channels_for_cur_subframe = 0;
        for i in 0..nb {
            let cur_subframe = self.channel[i].cur_subframe as usize;
            total_samples -= self.channel[i].decoded_samples as i32;
            if offset == self.channel[i].decoded_samples as i32
                && subframe_len == self.channel[i].subframe_len[cur_subframe] as i32
            {
                total_samples -= self.channel[i].subframe_len[cur_subframe] as i32;
                self.channel[i].decoded_samples =
                    self.channel[i].decoded_samples.wrapping_add(self.channel[i].subframe_len[cur_subframe]);
                self.channel_indexes_for_cur_subframe[self.channels_for_cur_subframe as usize] = i as i8;
                self.channels_for_cur_subframe += 1;
            }
        }

        if total_samples == 0 {
            self.parsed_all_subframes = 1;
        }

        self.table_idx = av_log2((self.samples_per_frame as i32 / subframe_len) as u32) as u8;
        self.num_bands = self.num_sfb[self.table_idx as usize];
        // cur_sfb_offsets = sfb_offsets[table_idx]
        let _cur_subwoofer_cutoff = self.subwoofer_cutoffs[self.table_idx as usize];

        offset += (self.samples_per_frame as i32) >> 1;

        for i in 0..self.channels_for_cur_subframe as usize {
            let c = self.channel_indexes_for_cur_subframe[i] as usize;
            self.channel[c].coeffs = offset as usize;
        }

        self.subframe_len = subframe_len as i16;
        self.esc_len = (av_log2((self.subframe_len as i32 - 1) as u32) + 1) as i8;

        if gb.get_bits1() != 0 {
            let mut num_fill_bits = gb.get_bits(2) as i32;
            if num_fill_bits == 0 {
                let len = gb.get_bits(4);
                num_fill_bits = gb.get_bitsz(len) as i32 + 1;
            }
            if num_fill_bits >= 0 {
                if gb.get_bits_count() + num_fill_bits > self.num_saved_bits {
                    self.errors.0 += 1;
                    return Err(());
                }
                gb.skip_bits_long(num_fill_bits);
            }
        }

        if gb.get_bits1() != 0 {
            // avpriv_request_sample "Reserved bit"
            return Err(());
        }

        if self.decode_channel_transform(gb).is_err() {
            return Err(());
        }

        for i in 0..self.channels_for_cur_subframe as usize {
            let c = self.channel_indexes_for_cur_subframe[i] as usize;
            self.channel[c].transmit_coefs = gb.get_bits1() as u8;
            if self.channel[c].transmit_coefs != 0 {
                transmit_coeffs = true;
            }
        }

        assert!(self.subframe_len as usize <= WMAPRO_BLOCK_MAX_SIZE);
        if transmit_coeffs {
            let mut quant_step: i32 = (90 * self.bits_per_sample as i32) >> 4;
            self.transmit_num_vec_coeffs = gb.get_bits1() as i8;
            if self.transmit_num_vec_coeffs != 0 {
                let num_bits = (av_log2(((self.subframe_len as i32 + 3) / 4) as u32) + 1) as u32;
                for i in 0..self.channels_for_cur_subframe as usize {
                    let c = self.channel_indexes_for_cur_subframe[i] as usize;
                    let num_vec_coeffs = (gb.get_bits(num_bits) << 2) as i32;
                    if num_vec_coeffs > self.subframe_len as i32 {
                        self.errors.0 += 1;
                        return Err(());
                    }
                    assert!(num_vec_coeffs as usize + offset as usize <= self.channel[c].out.len());
                    self.channel[c].num_vec_coeffs = num_vec_coeffs as u16;
                }
            } else {
                for i in 0..self.channels_for_cur_subframe as usize {
                    let c = self.channel_indexes_for_cur_subframe[i] as usize;
                    self.channel[c].num_vec_coeffs = self.subframe_len as u16;
                }
            }
            let mut step = gb.get_sbits(6);
            quant_step += step;
            if step == -32 || step == 31 {
                let sign: i32 = (step == 31) as i32 - 1;
                let mut quant = 0i32;
                while gb.get_bits_count() + 5 < self.num_saved_bits && {
                    step = gb.get_bits(5) as i32;
                    step == 31
                } {
                    quant += 31;
                }
                quant_step += ((quant + step) ^ sign) - sign;
            }
            // quant_step < 0: AV_LOG_DEBUG only

            if self.channels_for_cur_subframe == 1 {
                let c = self.channel_indexes_for_cur_subframe[0] as usize;
                self.channel[c].quant_step = quant_step;
            } else {
                let modifier_len = gb.get_bits(3);
                for i in 0..self.channels_for_cur_subframe as usize {
                    let c = self.channel_indexes_for_cur_subframe[i] as usize;
                    self.channel[c].quant_step = quant_step;
                    if gb.get_bits1() != 0 {
                        if modifier_len != 0 {
                            self.channel[c].quant_step += gb.get_bits(modifier_len) as i32 + 1;
                        } else {
                            self.channel[c].quant_step += 1;
                        }
                    }
                }
            }

            if self.decode_scale_factors(sh, gb).is_err() {
                return Err(());
            }
        }

        for i in 0..self.channels_for_cur_subframe as usize {
            let c = self.channel_indexes_for_cur_subframe[i] as usize;
            if self.channel[c].transmit_coefs != 0 && gb.get_bits_count() < self.num_saved_bits {
                let _ = self.decode_coeffs(sh, gb, c);
            } else {
                let co = self.channel[c].coeffs;
                for v in &mut self.channel[c].out[co..co + subframe_len as usize] {
                    *v = 0.0;
                }
            }
        }

        if transmit_coeffs {
            let mdct = &sh.mdct[(av_log2(subframe_len as u32) - WMAPRO_BLOCK_MIN_BITS as i32) as usize
                + (WMAPRO_BLOCK_MIN_BITS + 1) as usize
                - MDCT_MIN_BITS as usize];
            self.inverse_channel_transform();
            let cur = self.table_idx as usize;
            for i in 0..self.channels_for_cur_subframe as usize {
                let c = self.channel_indexes_for_cur_subframe[i] as usize;
                if c as i32 == self.lfe_channel as i32 {
                    unreachable!("XMA has no LFE channel mask");
                }
                for b in 0..self.num_bands as usize {
                    let end = (self.sfb_offsets[cur][b + 1] as i32).min(self.subframe_len as i32);
                    let ch = &self.channel[c];
                    let sf = ch.saved_scale_factors[ch.scale_factors][b];
                    let exp = ch.quant_step - (ch.max_scale_factor - sf) * ch.scale_factor_step as i32;
                    let quant = ff_exp10(exp as f64 / 20.0) as f32;
                    let start = self.sfb_offsets[cur][b] as i32;
                    let co = ch.coeffs;
                    if end - start > 0 {
                        let (s, e) = (start as usize, end as usize);
                        vector_fmul_scalar(&mut self.tmp[s..e], &ch.out[co + s..co + e], quant, e - s);
                    }
                }
                let co = self.channel[c].coeffs;
                let n2 = subframe_len as usize;
                let tmp = std::mem::take(&mut self.tmp);
                mdct.imdct_half(&sh.tables, &mut self.channel[c].out[co..co + n2], &tmp);
                self.tmp = tmp;
            }
        }

        self.wmapro_window(sh);

        for i in 0..self.channels_for_cur_subframe as usize {
            let c = self.channel_indexes_for_cur_subframe[i] as usize;
            if self.channel[c].cur_subframe >= self.channel[c].num_subframes {
                self.errors.0 += 1;
                return Err(());
            }
            self.channel[c].cur_subframe += 1;
        }
        Ok(())
    }
}

/// ff_wma_get_large_val
fn ff_wma_get_large_val(gb: &mut GetBits) -> u32 {
    let mut n_bits = 8u32;
    if gb.get_bits1() != 0 {
        n_bits += 8;
        if gb.get_bits1() != 0 {
            n_bits += 8;
            if gb.get_bits1() != 0 {
                n_bits += 7;
            }
        }
    }
    gb.get_bits_long(n_bits)
}

/// ff_wma_run_level_decode with version = 1 (wmapro).
fn ff_wma_run_level_decode(
    gb: &mut GetBits,
    vlc: &[[i16; 2]],
    level_table: &[f32],
    run_table: &[u16],
    ptr: &mut [f32],
    mut offset: i32,
    num_coefs: i32,
    block_len: i32,
    frame_len_bits: u32,
    errors: &mut AvErrors,
) -> i32 {
    let coef_mask = (block_len - 1) as u32;
    while offset < num_coefs {
        let code = gb.get_vlc2(vlc, VLCBITS, WMA_VLCMAX);
        if code > 1 {
            offset += run_table[code as usize] as i32;
            let sign = gb.get_bits1().wrapping_sub(1);
            ptr[(offset as u32 & coef_mask) as usize] =
                f32::from_bits(level_table[code as usize].to_bits() ^ (sign & 0x80000000));
        } else if code == 1 {
            break;
        } else {
            let level = ff_wma_get_large_val(gb) as i32;
            if gb.get_bits1() != 0 {
                if gb.get_bits1() != 0 {
                    if gb.get_bits1() != 0 {
                        errors.0 += 1; // "broken escape sequence"
                        return -1;
                    } else {
                        offset += gb.get_bits(frame_len_bits) as i32 + 4;
                    }
                } else {
                    offset += gb.get_bits(2) as i32 + 1;
                }
            }
            let sign = gb.get_bits1() as i32 - 1;
            ptr[(offset as u32 & coef_mask) as usize] = ((level ^ sign).wrapping_sub(sign)) as f32;
        }
        offset += 1;
    }
    if offset > num_coefs {
        errors.0 += 1; // "overflow in spectral RLE, ignoring"
        return -1;
    }
    0
}

#[cfg(test)]
mod tests {
    #[test]
    fn exp10_matches_known_values() {
        // fastexp2 is an approximation; just make sure it is sane.
        let v = super::ff_exp10(1.0);
        assert!((v - 10.0).abs() < 0.01, "{v}");
    }
}

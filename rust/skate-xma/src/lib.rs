//! skate-xma: exact decoder for Skate 3's EA-XMA (SNR codec 3) audio.
//!
//! Reproduces the reference `xmadec --continuous` decode bit for bit: the EA
//! block / item / packet extraction, the continuous XMA frame walk across
//! packet payloads, the `xmaframes` packets it builds, FFmpeg 4.4's WMA Pro /
//! XMA frame decoder as the rexglue-sdk (Xenia) fork compiles it (pure C
//! paths, clang -O2 -ffp-model=strict), and the context's ConvertFrame
//! (x 32767, cvtps2dq, saturating pack). See docs/AEMS.md "Exact XMA
//! decoding".
//!
//! Two things to know:
//! - The reference keeps one codec context per run and `xmaframes` has no
//!   flush callback, so a sound's first frame overlaps the previous sound's
//!   last one when both have the same rate and channel count. [`Decoder`]
//!   reproduces that (one per bank, jobs in table order);
//!   [`decode_snr_sample`] / [`decode_snr`] start fresh.
//! - After a packet's last frame the walk continues from the next packet
//!   after the one that frame *starts* in (XMA semantics). The reference
//!   tool used the one the frame *ends* in, which gives the same frames for
//!   every bank sample but halves the standalone grain/wheel streams.
//!
//! The libm-derived tables (sine windows, FFT cosines, IMDCT sines) are
//! baked in (`dsptables_baked.rs`), so output does not depend on the host
//! C runtime; everything else is IEEE single/double arithmetic in FFmpeg's
//! operation order (the fork's `fastexp2` and `cosfast` included).
//!
//! # License
//!
//! This crate is a port of FFmpeg code and is therefore licensed
//! LGPL-2.1-or-later (see the FFmpeg COPYING.LGPLv2.1 text). It is kept a
//! separate crate and binary so the rest of SkateV is not affected.
//!
//! Ported from FFmpeg 4.4 (libavcodec 58.134) as vendored in rexglue-sdk
//! `thirdparty/FFmpeg` (Xenia's fork):
//! - `libavcodec/wmaprodec.c` (decode_init for XMAFRAMES, xmaframes_decode_packet,
//!   save_bits, decode_frame, decode_tilehdr, decode_subframe_length,
//!   decode_channel_transform, decode_coeffs, decode_scale_factors,
//!   inverse_channel_transform, wmapro_window, decode_subframe) -> `wmapro.rs`
//! - `libavcodec/wmaprodata.h` (Huffman/run/level tables) -> `wmaprodata.rs`
//! - `libavcodec/wma.c` (ff_wma_run_level_decode, ff_wma_get_large_val) -> `wmapro.rs`
//! - `libavcodec/get_bits.h` (safe big-endian bit reader, get_vlc2) -> `getbits.rs`
//! - `libavcodec/bitstream.c` (ff_init_vlc_sparse / build_table) -> `vlc.rs`
//! - `libavcodec/fft_template.c`, `fft-internal.h` (float split-radix FFT, cos
//!   tables, revtab) and `libavcodec/mdct_template.c` (ff_mdct_init with the
//!   fork's cosfast, ff_imdct_half_c) -> `mdct.rs`, `dsptables.rs`
//! - `libavcodec/sinewin_tablegen.h` (sine windows) -> `dsptables.rs`
//! - `libavutil/float_dsp.c` (vector_fmul_scalar_c, vector_fmul_window_c) and
//!   `libavutil/ffmath.h` (ff_exp10 = the fork's fastexp2) -> `wmapro.rs`
//!
//! Copyright (c) 2007 Baptiste Coudurier, Benjamin Larsson, Ulion;
//! (c) 2008-2011 Sascha Sommer, Benjamin Larsson; (c) 2008 Loren Merritt;
//! (c) 2002 Fabrice Bellard; and the other FFmpeg authors. FFmpeg is free
//! software; you can redistribute it and/or modify it under the terms of the
//! GNU Lesser General Public License as published by the Free Software
//! Foundation; either version 2.1 of the License, or (at your option) any
//! later version. It is distributed WITHOUT ANY WARRANTY; without even the
//! implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
//!
//! The EA layer (`ea.rs`) follows SkateV's own xmadec tool and rexglue-sdk's
//! xma_context.cpp / BitStream (Xenia, BSD).

// Float code ported statement by statement from FFmpeg: the literals keep the C
// source's digits, index loops mirror it, and the transform butterflies take
// FFmpeg's argument lists.
#![allow(
    clippy::excessive_precision,
    clippy::approx_constant,
    clippy::needless_range_loop,
    clippy::too_many_arguments
)]

mod dsptables;
mod dsptables_baked;
mod ea;
mod getbits;
mod mdct;
mod vlc;
mod wmapro;
mod wmaprodata;

use std::fmt;
use std::sync::OnceLock;

pub use ea::{Packet, SAMPLES_PER_FRAME};

pub const VERSION_LINE: &str = concat!("skate-xma ", env!("CARGO_PKG_VERSION"), " (FFmpeg 4.4 xmaframes port)");

fn shared() -> &'static wmapro::Shared {
    static S: OnceLock<wmapro::Shared> = OnceLock::new();
    S.get_or_init(wmapro::Shared::new)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// More than 2 channels (several XMA streams): not handled, as in xmadec.
    MultiStream(u32),
    /// Not an EA SNR codec-3 (EA-XMA) header.
    NotXma(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::MultiStream(c) => write!(f, "{c} channels (multi-stream) not handled"),
            Error::NotXma(s) => write!(f, "not EA-XMA: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// Decoded PCM16 (interleaved when stereo), whole 512-sample frames from the
/// sound's first frame, as the XMA context writes its output ring.
#[derive(Debug, Clone)]
pub struct Pcm {
    pub samples: Vec<i16>,
    pub channels: u16,
    /// From the first item's XMA rate register (24000/32000/44100/48000).
    pub sample_rate: u32,
    pub frames: usize,
    /// Error-level messages FFmpeg would have logged for this sound.
    pub av_errors: u32,
}

impl Pcm {
    /// The `.xma16` layout: big-endian PCM16.
    pub fn to_be_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.samples.len() * 2);
        for s in &self.samples {
            v.extend_from_slice(&s.to_be_bytes());
        }
        v
    }
}

const ID_TO_SAMPLE_RATE: [u32; 4] = [24000, 32000, 44100, 48000];

/// A decoder that keeps one `xmaframes` context across sounds exactly as
/// `xmadec --continuous` does within one invocation: the context is only
/// re-created when the sample rate or channel count changes, and the
/// xmaframes codec has no flush callback, so the first frame of a sound
/// overlaps with the previous sound's last frame. Use this (one instance
/// per bank, jobs in table order) to reproduce the reference files bit for
/// bit; use [`decode_snr_sample`] for a fresh-state decode.
#[derive(Default)]
pub struct Decoder {
    ctx: ea::Context,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// One xmadec job: `data_offset` is the first EA block after the SNR
    /// header, `samples`/`channels` from the header.
    pub fn decode_sample(&mut self, file: &[u8], data_offset: usize, samples: u32, channels: u32) -> Result<Pcm, Error> {
        if channels > 2 {
            return Err(Error::MultiStream(channels));
        }
        let mut rate_id = -1i32;
        let pkts = ea::packets(file, data_offset, samples, &mut rate_id);
        let rate_id = if rate_id >= 0 { rate_id } else { 3 };
        let stereo = channels == 2;
        let mut out = Vec::new();
        let r = ea::decode_continuous(&mut self.ctx, shared(), &pkts, stereo, rate_id, &mut out);
        Ok(Pcm {
            samples: out,
            channels: if stereo { 2 } else { 1 },
            sample_rate: ID_TO_SAMPLE_RATE[rate_id.min(3) as usize],
            frames: r.frames,
            av_errors: r.av_errors,
        })
    }
}

/// Decode one bank sample with a fresh decoder (no state carried over from
/// another sound). `file` is the whole .abk, `data_offset` the first EA
/// block after the SNR header (see `tools/prepare-skate-audio.py::jobs_for`). The output rate is the
/// stream's XMA rate register, reported in [`Pcm::sample_rate`].
pub fn decode_snr_sample(file: &[u8], data_offset: usize, samples: u32, channels: u32) -> Result<Pcm, Error> {
    Decoder::new().decode_sample(file, data_offset, samples, channels)
}

/// An EA SNR header (EAAudioCore v1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnrHeader {
    pub version: u8,
    pub codec: u8,
    pub channels: u32,
    pub sample_rate: u32,
    /// 0 RAM, 1 streamed, 2 gigasample
    pub kind: u8,
    pub looped: bool,
    pub samples: u32,
    pub loop_start: Option<u32>,
    /// Size of the header (offset of the first EA block from the header).
    pub header_size: usize,
}

impl SnrHeader {
    pub fn parse(b: &[u8]) -> Result<Self, Error> {
        if b.len() < 8 {
            return Err(Error::NotXma("short header".into()));
        }
        let h1 = ea::be32(b, 0);
        let h2 = ea::be32(b, 4);
        let version = (h1 >> 28) as u8;
        let codec = ((h1 >> 24) & 0xF) as u8;
        let kind = (h2 >> 30) as u8;
        let looped = (h2 >> 29) & 1 != 0;
        let mut header_size = 8;
        let mut loop_start = None;
        if looped {
            if b.len() < 12 {
                return Err(Error::NotXma("short header".into()));
            }
            loop_start = Some(ea::be32(b, 8));
            header_size += 4;
            if kind == 1 {
                // streamed + looped: loop offset follows
                header_size += 4;
            }
        }
        let h = SnrHeader {
            version,
            codec,
            channels: ((h1 >> 18) & 0x3F) + 1,
            sample_rate: h1 & 0x3FFFF,
            kind,
            looped,
            samples: h2 & 0x1FFF_FFFF,
            loop_start,
            header_size,
        };
        if version != 0 || codec != 3 {
            return Err(Error::NotXma(format!("version {version} codec {codec}")));
        }
        Ok(h)
    }
}

/// Decode a standalone EA SNR stream (header + EA blocks in one buffer, e.g.
/// `raw/wheels/*.snr` or a `.grain` file from the u32 BE offset at byte 0)
/// with a fresh decoder, the same way as bank samples.
pub fn decode_snr(snr: &[u8]) -> Result<(SnrHeader, Pcm), Error> {
    let h = SnrHeader::parse(snr)?;
    let pcm = decode_snr_sample(snr, h.header_size, h.samples, h.channels)?;
    Ok((h, pcm))
}

// EA-XMA (SNR codec 3) packet extraction and the continuous XMA frame walk,
// ported from SkateV's xmadec tool (Blocks, Packets, DecodeContinuous,
// ConvertFrame), which itself follows rexglue-sdk's xma_context.cpp (Xenia,
// BSD) and rex::stream::BitStream.

use crate::wmapro::{PacketError, Shared, XmaFrames};

pub const BYTES_PER_PACKET: usize = 2048;
pub type Packet = [u8; BYTES_PER_PACKET];
const BITS_PER_FRAME_HEADER: usize = 15;
const MAX_FRAME_LENGTH: u32 = 0x7FFF;
pub const SAMPLES_PER_FRAME: usize = 512;
const ID_TO_SAMPLE_RATE: [i32; 4] = [24000, 32000, 44100, 48000];

#[inline]
pub(crate) fn be32(f: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([f[at], f[at + 1], f[at + 2], f[at + 3]])
}

/// xma::GetPacketFrameOffset: first frame offset in bits (incl. the 32-bit
/// packet header).
#[inline]
fn packet_frame_offset(p: &[u8]) -> usize {
    let val = (((p[0] as u32 & 0x3) << 13) | ((p[1] as u32) << 5) | ((p[2] as u32) >> 3)) as u16;
    val as usize + 32
}

pub struct Block {
    pub packets: Vec<Packet>,
}

/// xmadec Blocks(): EA blocks ([u32 flags|size][u32 samples] then items
/// [u32 size<<2|rate][bytes]); each item's bytes are whole 2048-byte packets
/// except a trimmed last one, zero-padded here. `rate_id` gets the first
/// item's rate register.
pub fn blocks(file: &[u8], mut at: usize, samples: u32, rate_id: &mut i32) -> Vec<Block> {
    let mut out = Vec::new();
    let mut seen: u32 = 0;
    while at + 8 <= file.len() && seen < samples {
        let hdr = be32(file, at);
        let size = (hdr & 0xFFFFFF) as usize;
        out.push(Block { packets: Vec::new() });
        let pk = &mut out.last_mut().unwrap().packets;
        seen = seen.wrapping_add(be32(file, at + 4));
        let mut item = at + 8;
        while item + 4 <= at + size && item + 4 <= file.len() {
            let word = be32(file, item);
            let len = (word >> 2) as usize;
            if len < 4 {
                break;
            }
            if *rate_id < 0 {
                *rate_id = (word & 3) as i32;
            }
            let mut p = item + 4;
            while p < item + len {
                let mut pkt = [0u8; BYTES_PER_PACKET];
                let n = BYTES_PER_PACKET.min(item + len - p);
                let avail = file.len().saturating_sub(p).min(n);
                pkt[..avail].copy_from_slice(&file[p..p + avail]);
                pk.push(pkt);
                p += BYTES_PER_PACKET;
            }
            item += len;
        }
        if size == 0 || (hdr & 0x8000_0000) != 0 {
            break;
        }
        at += size;
    }
    out
}

/// xmadec Packets(): the packets of every block, once.
pub fn packets(file: &[u8], at: usize, samples: u32, rate_id: &mut i32) -> Vec<Packet> {
    blocks(file, at, samples, rate_id).into_iter().flat_map(|b| b.packets).collect()
}

/// BitStream::Peek on a big-endian byte stream (at least 8 bytes readable
/// at `offset_bits >> 3`).
#[inline]
fn peek(buf: &[u8], offset_bits: usize, num_bits: usize) -> u64 {
    let ob = offset_bits >> 3;
    let rel = offset_bits - (ob << 3);
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[ob..ob + 8]);
    let bits = u64::from_be_bytes(b) >> (64 - (rel + num_bits));
    bits & ((1u64 << num_bits) - 1)
}

/// BitStream::Copy(dest, num_bits) from `offset_bits`; returns the bit
/// offset of the copied bits in dest[0].
fn bitstream_copy(buf: &[u8], offset_bits: usize, dest: &mut [u8], num_bits: usize) -> usize {
    let mut off = offset_bits;
    let offset_bytes = off >> 3;
    let rel = off - (offset_bytes << 3);
    let mut bits_left = num_bits;
    let mut out = 0usize;
    if rel != 0 {
        let bits = peek(buf, off, 8 - rel);
        let clear_mask = !((1u8 << rel) - 1);
        dest[out] &= clear_mask;
        dest[out] |= bits as u8;
        bits_left -= 8 - rel;
        off += 8 - rel;
        out += 1;
    }
    if bits_left >= 8 {
        let n = bits_left / 8;
        dest[out..out + n].copy_from_slice(&buf[offset_bytes + out..offset_bytes + out + n]);
        out += n;
        off += n * 8;
        bits_left -= n * 8;
    }
    if bits_left != 0 {
        let mut bits = peek(buf, off, bits_left);
        bits <<= 8 - bits_left;
        let clear_mask = ((1u16 << bits_left) - 1) as u8;
        dest[out] &= clear_mask;
        dest[out] |= bits as u8;
    }
    rel
}

/// cvtps2dq (round to nearest even; out of range or NaN -> 0x80000000).
#[inline(always)]
fn cvtps2dq(v: f32) -> i32 {
    if v.is_nan() || v >= 2147483648.0 || v < -2147483648.0 {
        i32::MIN
    } else {
        v.round_ties_even() as i32
    }
}

#[inline(always)]
fn packs(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// xma_context.cpp ConvertFrame: x 32767, cvtps2dq, saturating pack,
/// interleaved when stereo.
fn convert_frame(ch0: &[f32], ch1: Option<&[f32]>, out: &mut Vec<i16>) {
    const SCALE: f32 = ((1 << 15) - 1) as f32;
    match ch1 {
        Some(ch1) => {
            for (a, b) in ch0[..SAMPLES_PER_FRAME].iter().zip(&ch1[..SAMPLES_PER_FRAME]) {
                out.push(packs(cvtps2dq(a * SCALE)));
                out.push(packs(cvtps2dq(b * SCALE)));
            }
        }
        None => {
            for a in &ch0[..SAMPLES_PER_FRAME] {
                out.push(packs(cvtps2dq(a * SCALE)));
            }
        }
    }
}

/// The xmadec Context as far as --continuous uses it: one AVCodecContext
/// that is only re-created when rate/channels change (the xmaframes codec
/// has no flush callback, so its state carries over between sounds).
#[derive(Default)]
pub struct Context {
    dec: Option<XmaFrames>,
}

impl Context {
    /// PrepareDecoder
    fn prepare(&mut self, rate_id: i32, two: bool) -> &mut XmaFrames {
        let sample_rate = ID_TO_SAMPLE_RATE[rate_id.min(3) as usize];
        let channels = if two { 2 } else { 1 };
        let reuse = matches!(&self.dec, Some(d) if d.sample_rate == sample_rate && d.channels == channels);
        if !reuse {
            self.dec = Some(XmaFrames::new(sample_rate, channels).expect("xmaframes init"));
        }
        self.dec.as_mut().unwrap()
    }
}

pub struct ContinuousResult {
    pub frames: usize,
    pub av_errors: u32,
    /// the walk stopped on a frame FFmpeg rejected
    pub stopped_on_error: bool,
}

/// xmadec DecodeContinuous: frames walked across the packets' payloads
/// (bytes 4..2048 of each packet, concatenated), one decoder, xmaframes
/// packets built with the padding header byte.
pub fn decode_continuous(
    ctx: &mut Context,
    sh: &Shared,
    pkts: &[Packet],
    stereo: bool,
    rate_id: i32,
    out: &mut Vec<i16>,
) -> ContinuousResult {
    const PAYLOAD_BITS: usize = (BYTES_PER_PACKET - 4) * 8;
    let mut stream: Vec<u8> = Vec::with_capacity(pkts.len() * (BYTES_PER_PACKET - 4) + 16);
    for p in pkts {
        stream.extend_from_slice(&p[4..]);
    }
    stream.resize(stream.len() + 16, 0);
    let total_bits = pkts.len() * PAYLOAD_BITS;
    let dec = ctx.prepare(rate_id, stereo);
    let errors_before = dec.errors.0;
    let mut res = ContinuousResult { frames: 0, av_errors: 0, stopped_on_error: false };
    if pkts.is_empty() {
        return res;
    }
    // xma_frame: 1 + 4096 bytes (+ slack for the bit reader's lookahead,
    // which reads zeros there as it does in xmadec's zero-filled buffer).
    let mut xma_frame = vec![0u8; 1 + 4096 + 16];
    let mut pos = packet_frame_offset(&pkts[0]) - 32;
    while pos + BITS_PER_FRAME_HEADER <= total_bits {
        let len = peek(&stream, pos, BITS_PER_FRAME_HEADER) as u32;
        if len == 0 || len >= MAX_FRAME_LENGTH || len < BITS_PER_FRAME_HEADER as u32 + 1 || pos + len as usize > total_bits {
            break;
        }
        let len = len as usize;
        xma_frame.fill(0);
        let padding_start = bitstream_copy(&stream, pos, &mut xma_frame[1..], len);
        let size = 1 + ((padding_start + len) / 8) + if !(padding_start + len).is_multiple_of(8) { 1 } else { 0 };
        let padding_end = size * 8 - (8 + padding_start + len);
        xma_frame[0] = (((padding_start & 7) << 5) | ((padding_end & 7) << 2)) as u8;
        match dec.decode_packet(sh, &xma_frame, size) {
            Err(PacketError::InvalidData) => {
                res.stopped_on_error = true;
                break;
            }
            Ok(Some(frame)) => {
                let ch1 = if stereo { Some(&frame.ch[1][..]) } else { None };
                convert_frame(&frame.ch[0], ch1, out);
                res.frames += 1;
            }
            Ok(None) => {}
        }
        let more_in_packet = peek(&stream, pos + len - 1, 1) != 0;
        if more_in_packet {
            pos += len;
        } else {
            // The packet after the one the frame starts in (a last frame
            // may straddle into it; its frame offset points past the tail),
            // skipping packets in which no frame starts.
            let mut k = pos / PAYLOAD_BITS + 1;
            while k < pkts.len() && packet_frame_offset(&pkts[k]) - 32 >= PAYLOAD_BITS {
                k += 1;
            }
            if k >= pkts.len() {
                break;
            }
            pos = k * PAYLOAD_BITS + packet_frame_offset(&pkts[k]) - 32;
        }
    }
    res.av_errors = dec.errors.0 - errors_before;
    res
}

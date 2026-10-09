//! Decode a Xbox 360 XEX2 executable to its in-memory base image.
//!
//!     xex_image <default.xex> <out image.bin>
//!
//! Handles the retail-encrypted (AES-128-CBC, zero IV) payload with "none",
//! "basic" (zero-fill) or "normal" (chained blocks of 32 KB LZX frames,
//! SHA-1 verified per block) compression. Prints `base=` and `size=` lines.
//! Input is the user's own disc executable; the output stays local and is
//! never shipped.
use std::{env, fs};

use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, KeyInit};
use anyhow::{Result, anyhow, bail, ensure};
use lzxd::{Lzxd, WindowSize};
use sha1::{Digest, Sha1};

/// The Xbox 360 retail XEX format key. This is the console's public file
/// format constant (documented by every XEX tool), not a game or GTA key.
const RETAIL_KEY: [u8; 16] = [
    0x20, 0xB1, 0x85, 0xA5, 0x9D, 0x28, 0xFD, 0xC3, 0x40, 0x58, 0x3F, 0xBB, 0x08, 0x96, 0xBF, 0x91,
];
const FRAME: usize = 0x8000;

fn be32(d: &[u8], at: usize) -> Result<u32> {
    let b = d.get(at..at + 4).ok_or_else(|| anyhow!("truncated XEX at {at:#x}"))?;
    Ok(u32::from_be_bytes(b.try_into()?))
}

fn be16(d: &[u8], at: usize) -> Result<u16> {
    let b = d.get(at..at + 2).ok_or_else(|| anyhow!("truncated XEX at {at:#x}"))?;
    Ok(u16::from_be_bytes(b.try_into()?))
}

#[derive(Debug)]
pub struct Image {
    pub base: u32,
    pub data: Vec<u8>,
}

/// AES-128-CBC with a zero IV, in place; `data.len()` must be a multiple of 16.
/// A single block with a zero IV is plain AES, which unwraps the session key.
fn cbc_decrypt(key: &[u8; 16], data: &mut [u8]) {
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut prev = [0u8; 16];
    for block in data.chunks_exact_mut(16) {
        let cipher_text: [u8; 16] = block.try_into().unwrap();
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
        block.iter_mut().zip(prev).for_each(|(b, p)| *b ^= p);
        prev = cipher_text;
    }
}

fn window(size: u32) -> Result<WindowSize> {
    Ok(match size {
        0x8000 => WindowSize::KB32,
        0x10000 => WindowSize::KB64,
        0x20000 => WindowSize::KB128,
        0x40000 => WindowSize::KB256,
        0x80000 => WindowSize::KB512,
        0x100000 => WindowSize::MB1,
        0x200000 => WindowSize::MB2,
        0x400000 => WindowSize::MB4,
        _ => bail!("unsupported LZX window size {size:#x}"),
    })
}

pub fn decode(xex: &[u8]) -> Result<Image> {
    ensure!(xex.get(..4) == Some(b"XEX2"), "not an XEX2 file");
    let pe_offset = be32(xex, 8)? as usize;
    let security = be32(xex, 16)? as usize;
    let image_size = be32(xex, security + 4)? as usize;
    let load_address = be32(xex, security + 0x110)?;
    let mut session = [0u8; 16];
    session.copy_from_slice(xex.get(security + 0x150..security + 0x160).ok_or_else(|| anyhow!("truncated XEX security info"))?);

    // Optional headers: (id, value) pairs. Only these two are needed, and the
    // file format info (0x3FF) value is an offset to its data.
    let (mut format, mut base) = (None, None);
    for i in 0..be32(xex, 20)? as usize {
        let (id, value) = (be32(xex, 24 + i * 8)?, be32(xex, 28 + i * 8)?);
        match id {
            0x0001_0201 => base = Some(value),
            0x0000_03FF => format = Some(value as usize),
            _ => {}
        }
    }
    let format = format.ok_or_else(|| anyhow!("no file format info"))?;
    ensure!(base.unwrap_or(load_address) == load_address, "base address headers disagree");
    let info_size = be32(xex, format)? as usize;
    let (encryption, compression) = (be16(xex, format + 4)?, be16(xex, format + 6)?);
    ensure!(encryption <= 1, "unsupported encryption type {encryption}");

    let mut payload = xex.get(pe_offset..).ok_or_else(|| anyhow!("bad PE data offset"))?.to_vec();
    payload.truncate(payload.len() / 16 * 16);
    if encryption == 1 {
        cbc_decrypt(&RETAIL_KEY, &mut session);
        cbc_decrypt(&session, &mut payload);
    }

    let mut out = Vec::with_capacity(image_size);
    match compression {
        0 => out.extend_from_slice(&payload),
        1 => {
            // (data size, zero size) pairs after the 8 byte info header.
            let mut at = 0;
            for entry in 0..info_size.saturating_sub(8) / 8 {
                let data = be32(xex, format + 8 + entry * 8)? as usize;
                let zero = be32(xex, format + 12 + entry * 8)? as usize;
                out.extend_from_slice(payload.get(at..at + data).ok_or_else(|| anyhow!("basic block past payload"))?);
                at += data;
                out.resize(out.len() + zero, 0);
            }
        }
        2 => {
            // window size, then the first block's size and SHA-1.
            let mut lzx = Lzxd::new(window(be32(xex, format + 8)?)?);
            let mut size = be32(xex, format + 12)? as usize;
            let mut hash = xex.get(format + 16..format + 36).ok_or_else(|| anyhow!("truncated XEX"))?.to_vec();
            let mut at = 0;
            while size != 0 {
                let block = payload.get(at..at + size).ok_or_else(|| anyhow!("compressed block past payload"))?;
                ensure!(Sha1::digest(block).as_slice() == hash, "block hash mismatch at payload {at:#x}");
                // Each block starts with the next block's size and hash, then
                // u16-prefixed LZX chunks (one 32 KB frame each) ended by a zero.
                let next_size = be32(block, 0)? as usize;
                let next_hash = block.get(4..24).ok_or_else(|| anyhow!("short block"))?.to_vec();
                let mut p = 24;
                loop {
                    let chunk = be16(block, p)? as usize;
                    p += 2;
                    if chunk == 0 {
                        break;
                    }
                    let want = FRAME.min(image_size.saturating_sub(out.len()));
                    let data = block.get(p..p + chunk).ok_or_else(|| anyhow!("short chunk"))?;
                    out.extend_from_slice(lzx.decompress_next(data, want).map_err(|e| anyhow!("LZX: {e}"))?);
                    p += chunk;
                }
                (size, hash, at) = (next_size, next_hash, at + size);
            }
        }
        c => bail!("unsupported compression type {c}"),
    }
    ensure!(out.len() >= image_size, "decoded {} of {image_size} image bytes", out.len());
    out.truncate(image_size);
    Ok(Image { base: load_address, data: out })
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    ensure!(args.len() == 3, "usage: xex_image <default.xex> <out image.bin>");
    let image = decode(&fs::read(&args[1])?)?;
    fs::write(&args[2], &image.data)?;
    println!("base={:#010x}\nsize={}", image.base, image.data.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncrypt;

    fn encrypt(key: &[u8; 16], data: &mut [u8]) {
        let cipher = Aes128::new(GenericArray::from_slice(key));
        let mut prev = [0u8; 16];
        for block in data.chunks_exact_mut(16) {
            block.iter_mut().zip(prev).for_each(|(b, p)| *b ^= p);
            cipher.encrypt_block(GenericArray::from_mut_slice(block));
            prev.copy_from_slice(block);
        }
    }

    /// Synthetic XEX2: one optional header (format info at 0x40), security
    /// info at 0x100, payload at 0x400.
    fn synthetic(compression: u16, info: &[u8], payload: &[u8], image_size: u32) -> Vec<u8> {
        let session = [7u8; 16];
        let mut wrapped = session;
        encrypt(&RETAIL_KEY, &mut wrapped);
        let mut enc = payload.to_vec();
        encrypt(&session, &mut enc);
        let mut x = vec![0u8; 0x400];
        x[..4].copy_from_slice(b"XEX2");
        x[8..12].copy_from_slice(&0x400u32.to_be_bytes());
        x[16..20].copy_from_slice(&0x100u32.to_be_bytes());
        x[20..24].copy_from_slice(&1u32.to_be_bytes());
        x[24..28].copy_from_slice(&0x3FFu32.to_be_bytes());
        x[28..32].copy_from_slice(&0x40u32.to_be_bytes());
        x[0x40..0x44].copy_from_slice(&(8 + info.len() as u32).to_be_bytes());
        x[0x44..0x46].copy_from_slice(&1u16.to_be_bytes());
        x[0x46..0x48].copy_from_slice(&compression.to_be_bytes());
        x[0x48..0x48 + info.len()].copy_from_slice(info);
        x[0x104..0x108].copy_from_slice(&image_size.to_be_bytes());
        x[0x210..0x214].copy_from_slice(&0x8200_0000u32.to_be_bytes());
        x[0x250..0x260].copy_from_slice(&wrapped);
        x.extend_from_slice(&enc);
        x
    }

    #[test]
    fn basic_compression_zero_fills() {
        let mut payload = [0u8; 32];
        payload[..5].copy_from_slice(b"abcde");
        let mut info = Vec::new();
        for v in [3u32, 2, 2, 1] {
            info.extend_from_slice(&v.to_be_bytes());
        }
        let image = decode(&synthetic(1, &info, &payload, 8)).unwrap();
        assert_eq!(image.base, 0x8200_0000);
        assert_eq!(image.data, b"abc\0\0de\0");
    }

    #[test]
    fn normal_compression_rejects_bad_block_hash() {
        let mut info = Vec::new();
        info.extend_from_slice(&0x20000u32.to_be_bytes());
        info.extend_from_slice(&32u32.to_be_bytes());
        info.extend_from_slice(&[0u8; 20]);
        let err = decode(&synthetic(2, &info, &[0u8; 32], 16)).unwrap_err();
        assert!(err.to_string().contains("hash"), "{err}");
    }
}

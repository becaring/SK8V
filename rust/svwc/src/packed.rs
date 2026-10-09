//! Packed tile records, shared by SVWC 3, SVGJ 4 and SVSD 2: the header and
//! tile table are unchanged, then
//!
//! ```text
//! repeat tile count (table order): u64 offset after this index, u32 packed bytes
//! repeat tile count: the tile's records as one zstd frame
//! ```
//!

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom, Write};

/// zstd level: ~4x on collision records, decompression is level-independent.
const LEVEL: i32 = 9;

/// Where one tile's records are: one zstd frame of `packed` bytes at `at`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Block {
    pub at: u64,
    pub packed: u32,
    pub count: u32,
}

pub(crate) fn tile_of(v: f32, size: f32) -> i32 {
    (v / size).floor() as i32
}

pub(crate) fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

pub(crate) type Tiles = HashMap<(i32, i32), Block>;

/// A tile table: (tile, first record, record count) per tile.
pub(crate) type TileTable = Vec<((i32, i32), u32, u32)>;

/// The SVWC prologue after the magic: version, tile size, tile count, record
/// count, then the tile table and its block index. Returns the tile size,
/// record count and each tile's block. `raw` is the older version whose
/// `record_bytes` records follow the table unpacked. `fix` ends the
/// wrong-version error ("rebuild the cache").
pub(crate) fn read_tiles<R: Read + Seek>(
    r: &mut R,
    magic: [u8; 4],
    version: u32,
    raw: (u32, usize),
    kind: &str,
    fix: &str,
) -> io::Result<(f32, u32, Tiles)> {
    let bad = |m: String| io::Error::new(io::ErrorKind::InvalidData, m);
    let mut found = [0; 4];
    r.read_exact(&mut found)?;
    if found != magic {
        return Err(bad(format!("not an {kind}")));
    }
    let got = read_u32(r)?;
    if got != version && got != raw.0 {
        let name = String::from_utf8_lossy(&magic);
        return Err(bad(format!("{name} version {got}, expected {version}; {fix}")));
    }
    let tile_size = f32::from_bits(read_u32(r)?);
    let tile_count = read_u32(r)?;
    let record_count = read_u32(r)?;
    if !(tile_size.is_finite() && tile_size > 0.0) {
        return Err(bad("invalid tile size".into()));
    }
    let table = read_table(r, tile_count, record_count)?;
    let blocks = if got == raw.0 {
        let at = r.stream_position()?;
        table.iter().map(|&(_, first, count)| Block { at: at + first as u64 * raw.1 as u64, packed: 0, count }).collect()
    } else {
        read_index(r, &table.iter().map(|t| t.2).collect::<Vec<_>>())?
    };
    Ok((tile_size, record_count, table.iter().map(|t| t.0).zip(blocks).collect()))
}

pub(crate) fn read_table<R: Read>(r: &mut R, tile_count: u32, record_count: u32) -> io::Result<TileTable> {
    let mut table = Vec::with_capacity(tile_count as usize);
    for _ in 0..tile_count {
        let tx = read_u32(r)? as i32;
        let ty = read_u32(r)? as i32;
        let first = read_u32(r)?;
        let count = read_u32(r)?;
        if first as u64 + count as u64 > record_count as u64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "tile range outside record table"));
        }
        table.push(((tx, ty), first, count));
    }
    Ok(table)
}

/// Writes the block index and frames for raw per-tile record bytes.
pub(crate) fn write<W: Write>(out: &mut W, tiles: &[Vec<u8>]) -> io::Result<()> {
    write_frames(out, &compress(tiles)?)
}

/// One zstd frame per tile, on every core.
pub(crate) fn compress(tiles: &[Vec<u8>]) -> io::Result<Vec<Vec<u8>>> {
    compress_with(tiles, LEVEL)
}

/// As [`compress`] at zstd level `level`.
pub(crate) fn compress_with(tiles: &[Vec<u8>], level: i32) -> io::Result<Vec<Vec<u8>>> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = tiles.len().div_ceil(threads).max(1);
    Ok(std::thread::scope(|s| {
        let jobs: Vec<_> = tiles
            .chunks(chunk)
            .map(|c| s.spawn(move || c.iter().map(|t| zstd::bulk::compress(t, level)).collect::<io::Result<Vec<_>>>()))
            .collect();
        jobs.into_iter().map(|j| j.join().expect("zstd worker")).collect::<io::Result<Vec<_>>>()
    })?
    .into_iter()
    .flatten()
    .collect())
}

pub(crate) fn write_frames<W: Write>(out: &mut W, frames: &[Vec<u8>]) -> io::Result<()> {
    let mut at = 0u64;
    for f in frames {
        let len = u32::try_from(f.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "tile block over 4 GB"))?;
        out.write_all(&at.to_le_bytes())?;
        out.write_all(&len.to_le_bytes())?;
        at += f.len() as u64;
    }
    for f in frames {
        out.write_all(f)?;
    }
    Ok(())
}

/// A header section (SVGJ joint maps) as `u32 packed bytes` and one zstd frame.
pub(crate) fn write_section<W: Write>(out: &mut W, raw: &[u8]) -> io::Result<()> {
    let frame = zstd::bulk::compress(raw, LEVEL)?;
    let len = u32::try_from(frame.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "section over 4 GB"))?;
    out.write_all(&len.to_le_bytes())?;
    out.write_all(&frame)
}

pub(crate) fn read_section<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let mut frame = vec![0u8; u32::from_le_bytes(len) as usize];
    r.read_exact(&mut frame)?;
    zstd::stream::decode_all(&frame[..])
}

/// Reads the block index after a tile table of `counts` (table order).
pub(crate) fn read_index<R: Read + Seek>(r: &mut R, counts: &[u32]) -> io::Result<Vec<Block>> {
    let mut index = Vec::with_capacity(counts.len());
    for &count in counts {
        let mut b = [0u8; 12];
        r.read_exact(&mut b)?;
        index.push((u64::from_le_bytes(b[0..8].try_into().unwrap()), u32::from_le_bytes(b[8..12].try_into().unwrap()), count));
    }
    let base = r.stream_position()?;
    Ok(index.into_iter().map(|(at, packed, count)| Block { at: base + at, packed, count }).collect())
}

/// A tile's raw record bytes into `buf` (`packed == 0`: stored unpacked).
pub(crate) fn read<R: Read + Seek>(r: &mut R, b: Block, record_bytes: usize, buf: &mut Vec<u8>) -> io::Result<()> {
    let raw = b.count as usize * record_bytes;
    r.seek(SeekFrom::Start(b.at))?;
    if b.packed == 0 {
        buf.resize(raw, 0);
        return r.read_exact(buf);
    }
    let mut frame = vec![0u8; b.packed as usize];
    r.read_exact(&mut frame)?;
    *buf = zstd::bulk::decompress(&frame, raw)?;
    if buf.len() != raw {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "tile block size mismatch"));
    }
    Ok(())
}

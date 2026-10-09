//! SkateV ground joint sidecar (`SVGJ`), version 5: the visual ground GTA
//! draws over walkable collision, with each triangle's own texture
//! coordinates and a relief map of its texture (`docs/MATERIAL-RECOVERY.md`,
//! `tools/material-recovery/relief_map.py`): every measured groove, raised
//! line and pit with its depth. Seam sounds follow the relief a player sees,
//! through the same UVs the GPU uses. Collision is untouched.
//!
//! Version 5 is the distributable form: shared, fixed-point vertices coded
//! as deltas (about 3-4 bytes a triangle against 64), no triangles under
//! [`MIN_AREA`], zstd 19 per 64 m tile. Version 4 (64-byte records)
//! is still read (`examples/pack_ground.rs` converts it).
//!
//! ```text
//! 4 bytes  magic "SVGJ"
//! u32      version = 5
//! f32      tile size (metres, XY; a triangle lives in its centroid's tile)
//! u32      joint map count
//! u32      tile count
//! u32      triangle count
//! u32 packed bytes, then the maps below as one zstd frame:
//! repeat map count:
//!   u16 width, u16 height, u8 roughness (roughness.py rank 1..=255, 128
//!   the median texture; 0 unknown), u8 flags (bit 0: a visibility plane
//!   follows), then height rows of ceil(width / 8) bytes;
//!   texel (x, y) is bit x % 8 of byte y * stride + x / 8 (1 = relief).
//!   Row 0 is v = 0 (Direct3D texture order). Then one byte per set bit in
//!   raster order: kind << 6 | level, kind 1 groove / 2 raised / 3 pit,
//!   level 1..=63 depth (63 = relief_map.py DEPTH_FULL or deeper). Then,
//!   with the flag, a plane of the same shape: 1 = the texel is visible
//!   (a decal overlay's alpha); no plane = everywhere.
//! repeat tile count (sorted by tx, ty):
//!   i32 tx, i32 ty, u32 packed bytes, u32 raw bytes
//! then each tile's zstd frame, back to back. A tile's raw block:
//!   varint vertex count, varint triangle count,
//!   five columns of zigzag varint deltas per vertex: x, y (1/256 m from
//!   the tile corner), z (cm), u, v (1/2048),
//!   per triangle three corners as zigzag varint (index - next unused
//!   vertex; vertices are numbered by first use), the map as a zigzag
//!   varint delta, then one byte per triangle of seam type, then of layer.
//! ```
//! All values are little-endian. Version 4 (maps without the flags byte;
//! zstd per-tile blocks of fixed 64-byte records: 3 x f32 xyz, 3 x f32 uv,
//! u16 map, u8 seam, u8 0) is read as layer 0; older versions are rejected.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::packed::{read_u32, tile_of};

pub const MAGIC: [u8; 4] = *b"SVGJ";
pub const VERSION: u32 = 5;
pub const TRIANGLE_BYTES: usize = 64;

/// Relief texel kinds (`value() >> 6`).
pub const GROOVE: u8 = 1;
pub const RAISED: u8 = 2;
pub const PIT: u8 = 3;

/// A texture's relief (set bits) with each set texel's `kind << 6 | level`
/// byte, tiling. `rank` (derived) counts the set bits before each row.
#[derive(Clone, Debug, PartialEq)]
pub struct JointMap {
    pub width: u16,
    pub height: u16,
    pub bits: Vec<u8>,
    pub values: Vec<u8>,
    /// Fine-scale grain of the whole texture, 1..=255 (128 median; 0 unknown).
    pub roughness: u8,
    /// A decal overlay's alpha: the same shape as `bits`, 1 = visible.
    /// `None`: visible everywhere.
    pub visible: Option<Vec<u8>>,
    rank: Vec<u32>,
}

impl JointMap {
    /// A map whose every set texel is a full-depth groove.
    pub fn from_mask(width: u16, height: u16, joint: impl Fn(usize, usize) -> bool) -> Self {
        Self::from_values(width, height, |x, y| if joint(x, y) { GROOVE << 6 | 63 } else { 0 })
    }

    /// A bit plane in the shape of `bits` (a visibility plane), set where `on`.
    pub fn pack_mask(width: u16, height: u16, on: impl Fn(usize, usize) -> bool) -> Vec<u8> {
        let stride = (width as usize).div_ceil(8);
        let mut bits = vec![0u8; stride * height as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                if on(x, y) {
                    bits[y * stride + x / 8] |= 1 << (x % 8);
                }
            }
        }
        bits
    }

    /// A map from per-texel bytes (0 = flat).
    pub fn from_values(width: u16, height: u16, value: impl Fn(usize, usize) -> u8) -> Self {
        let stride = (width as usize).div_ceil(8);
        let mut bits = vec![0u8; stride * height as usize];
        let mut values = Vec::new();
        for y in 0..height as usize {
            for x in 0..width as usize {
                let b = value(x, y);
                if b != 0 {
                    bits[y * stride + x / 8] |= 1 << (x % 8);
                    values.push(b);
                }
            }
        }
        Self::new(width, height, bits, values).expect("consistent map")
    }

    /// Checks sizes (one value per set bit) and derives the row ranks.
    pub fn new(width: u16, height: u16, bits: Vec<u8>, values: Vec<u8>) -> Option<Self> {
        let stride = (width as usize).div_ceil(8);
        if width == 0 || height == 0 || bits.len() != stride * height as usize {
            return None;
        }
        let mut rank = Vec::with_capacity(height as usize);
        let mut n = 0u32;
        for row in bits.chunks_exact(stride) {
            rank.push(n);
            n += row.iter().map(|b| b.count_ones()).sum::<u32>();
        }
        (values.len() == n as usize).then_some(Self { width, height, bits, values, roughness: 0, visible: None, rank })
    }

    fn stride(&self) -> usize {
        (self.width as usize).div_ceil(8)
    }

    /// Whether the texel at UV `(u, v)` is relief, wrapping as the texture tiles.
    pub fn at(&self, u: f32, v: f32) -> bool {
        self.value(u, v) != 0
    }

    /// The texel's `kind << 6 | level` byte (0 = flat).
    pub fn value(&self, u: f32, v: f32) -> u8 {
        let (w, h) = (self.width as f32, self.height as f32);
        let x = ((u.rem_euclid(1.0) * w) as usize).min(self.width as usize - 1);
        let y = ((v.rem_euclid(1.0) * h) as usize).min(self.height as usize - 1);
        let row = &self.bits[y * self.stride()..(y + 1) * self.stride()];
        if row[x / 8] >> (x % 8) & 1 == 0 {
            return 0;
        }
        let before = row[..x / 8].iter().map(|b| b.count_ones()).sum::<u32>() + (row[x / 8] & ((1u16 << (x % 8)) - 1) as u8).count_ones();
        self.values[(self.rank[y] + before) as usize]
    }

    /// Whether the texel at UV `(u, v)` shows (an overlay's alpha; always
    /// true without a visibility plane).
    pub fn visible_at(&self, u: f32, v: f32) -> bool {
        let Some(plane) = &self.visible else { return true };
        let (w, h) = (self.width as f32, self.height as f32);
        let x = ((u.rem_euclid(1.0) * w) as usize).min(self.width as usize - 1);
        let y = ((v.rem_euclid(1.0) * h) as usize).min(self.height as usize - 1);
        plane[y * self.stride() + x / 8] >> (x % 8) & 1 == 1
    }

    /// Texels per UV unit along each axis (the finest step a path needs).
    pub fn texels(&self) -> f32 {
        self.width.max(self.height) as f32
    }

    /// Joint lines take their deepest level: each 8-connected relief
    /// component spanning at least `min_extent` of the texture's shorter side
    /// gets that component's maximum level on every texel (kinds kept). A
    /// joint has one depth; the measured per-texel strength along it is
    /// noise (tile grout measures 6..8 median, 28 at its deepest), while
    /// short specks keep their own weak level for the runtime depth gate.
    pub fn level_lines(&mut self, min_extent: f32) {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut grid = vec![0u8; w * h];
        let mut i = 0;
        for y in 0..h {
            for x in 0..w {
                if self.bits[y * self.stride() + x / 8] >> (x % 8) & 1 == 1 {
                    grid[y * w + x] = self.values[i];
                    i += 1;
                }
            }
        }
        let span = (min_extent * w.min(h) as f32).ceil() as usize;
        let mut seen = vec![false; w * h];
        let (mut stack, mut members) = (Vec::new(), Vec::new());
        for start in 0..w * h {
            if grid[start] == 0 || seen[start] {
                continue;
            }
            seen[start] = true;
            stack.push(start);
            members.clear();
            let (mut x0, mut x1, mut y0, mut y1, mut deepest) = (w, 0, h, 0, 0u8);
            while let Some(p) = stack.pop() {
                let (x, y) = (p % w, p / w);
                (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
                deepest = deepest.max(grid[p] & 63);
                members.push(p);
                for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let q = ny as usize * w + nx as usize;
                    if grid[q] != 0 && !seen[q] {
                        seen[q] = true;
                        stack.push(q);
                    }
                }
            }
            if (x1 - x0 + 1).max(y1 - y0 + 1) >= span {
                for &p in &members {
                    grid[p] = grid[p] & 0xC0 | deepest;
                }
            }
        }
        self.values = grid.into_iter().filter(|&v| v != 0).collect();
    }
}

/// One visual ground triangle: positions (GTA metres), texture coordinates,
/// the map it shows and what it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundTri {
    pub p: [[f32; 3]; 3],
    pub uv: [[f32; 2]; 3],
    pub map: u16,
    /// Skate seam type (1..=15) that picks the click sounds.
    pub seam: u8,
    /// 0: GTA's opaque ground. 1: a decal-pass overlay: it shows only where
    /// its map's `visible` plane is set, and the ground under it shows
    /// elsewhere.
    pub layer: u8,
}

/// Triangles smaller than this (m^2) are not stored: 27 % of the map's
/// ground triangles are under 0.05 m^2 (a 30 cm x 30 cm right triangle is
/// 0.045) and carry 0.6 % of its area (a wheel never has to resolve them,
/// and they are a quarter of the file; 0.1 saved 17 MB more of 162 and
/// cost 0.3 points of coverage on the acceptance areas).
pub const MIN_AREA: f32 = 0.05;

/// Fixed-point steps: positions 1/256 m, height 1 cm, texture coordinates
/// 1/2048 (a quarter texel on the 512 maps, which are the largest).
const XY_STEP: f32 = 256.0;
const Z_STEP: f32 = 100.0;
const UV_STEP: f32 = 2048.0;
/// Quantised values beyond this are corrupt, not ground.
const LIMIT: f32 = 1.0e9;

fn area(p: &[[f32; 3]; 3]) -> f32 {
    let e1: [f32; 3] = std::array::from_fn(|i| p[1][i] - p[0][i]);
    let e2: [f32; 3] = std::array::from_fn(|i| p[2][i] - p[0][i]);
    let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
    0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
}

fn put_var(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn zig(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzig(v: u64) -> i64 {
    (v >> 1) as i64 ^ -((v & 1) as i64)
}

struct Cursor8<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor8<'_> {
    fn var(&mut self) -> io::Result<u64> {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let byte = *self.b.get(self.at).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ground tile block ends early"))?;
            self.at += 1;
            v |= ((byte & 0x7F) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
            if shift > 63 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "ground tile varint too long"));
            }
        }
    }
    fn signed(&mut self) -> io::Result<i64> {
        self.var().map(unzig)
    }
    fn bytes(&mut self, n: usize) -> io::Result<&[u8]> {
        let s = self.b.get(self.at..self.at + n).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ground tile block ends early"))?;
        self.at += n;
        Ok(s)
    }
}

/// One tile's triangles as a block: vertices (shared between triangles,
/// numbered in order of first use) as delta-coded fixed-point columns, then
/// triangle corners relative to the next unused vertex, maps, seams, layers.
/// (Block, triangles kept).
pub fn encode_tile(tris: &[&GroundTri], tile: (i32, i32), tile_size: f32) -> (Vec<u8>, usize) {
    let (block, n, _) = encode_tile_sized(tris, tile, tile_size);
    (block, n)
}

/// As [`encode_tile`], plus the raw bytes of each part of the block: the
/// five vertex columns, corners, maps, seams, layers (diagnostics).
pub fn encode_tile_sized(tris: &[&GroundTri], tile: (i32, i32), tile_size: f32) -> (Vec<u8>, usize, [usize; 10]) {
    let (ox, oy) = (tile.0 as f32 * tile_size, tile.1 as f32 * tile_size);
    let mut index: HashMap<[i32; 5], u32> = HashMap::new();
    let mut verts: Vec<[i32; 5]> = Vec::new();
    let mut corners: Vec<[u32; 3]> = Vec::new();
    let mut seen: std::collections::HashSet<([u32; 3], u16, u8)> = std::collections::HashSet::new();
    let (mut maps, mut seams, mut layers) = (Vec::new(), Vec::new(), Vec::new());
    for t in tris {
        let mut ids = [0u32; 3];
        for (k, id) in ids.iter_mut().enumerate() {
            let q = [
                ((t.p[k][0] - ox) * XY_STEP).round() as i32,
                ((t.p[k][1] - oy) * XY_STEP).round() as i32,
                (t.p[k][2] * Z_STEP).round() as i32,
                (t.uv[k][0] * UV_STEP).round() as i32,
                (t.uv[k][1] * UV_STEP).round() as i32,
            ];
            *id = *index.entry(q).or_insert_with(|| {
                verts.push(q);
                verts.len() as u32 - 1
            });
        }
        let mut key = ids;
        key.sort_unstable();
        if ids[0] == ids[1] || ids[1] == ids[2] || ids[0] == ids[2] || !seen.insert((key, t.map, t.layer)) {
            continue;
        }
        corners.push(ids);
        maps.push(t.map);
        seams.push(t.seam);
        layers.push(t.layer);
    }
    // Vertices in order of first use, so corners mostly say "the next one".
    let mut order = vec![u32::MAX; verts.len()];
    let mut next = 0u32;
    for c in &corners {
        for &v in c {
            if order[v as usize] == u32::MAX {
                order[v as usize] = next;
                next += 1;
            }
        }
    }
    let mut sorted = vec![[0i32; 5]; next as usize];
    for (v, &o) in order.iter().enumerate() {
        if o != u32::MAX {
            sorted[o as usize] = verts[v];
        }
    }
    let mut out = Vec::new();
    let mut sizes = [0usize; 10];
    put_var(&mut out, sorted.len() as u64);
    put_var(&mut out, corners.len() as u64);
    for col in 0..5 {
        let before = out.len();
        let mut prev = 0i64;
        for v in &sorted {
            put_var(&mut out, zig(v[col] as i64 - prev));
            prev = v[col] as i64;
        }
        sizes[col] = out.len() - before;
    }
    let before = out.len();
    let mut fresh = 0i64;
    for c in &corners {
        for &v in c {
            let o = order[v as usize] as i64;
            put_var(&mut out, zig(o - fresh));
            fresh = fresh.max(o + 1);
        }
    }
    sizes[5] = out.len() - before;
    let before = out.len();
    let mut prev = 0i64;
    for &m in &maps {
        put_var(&mut out, zig(m as i64 - prev));
        prev = m as i64;
    }
    sizes[6] = out.len() - before;
    out.extend_from_slice(&seams);
    out.extend_from_slice(&layers);
    sizes[7] = seams.len();
    sizes[8] = layers.len();
    sizes[9] = sorted.len();
    (out, corners.len(), sizes)
}

fn decode_tile(raw: &[u8], tile: (i32, i32), tile_size: f32, out: &mut Vec<GroundTri>, maps: usize) -> io::Result<()> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    let (ox, oy) = (tile.0 as f32 * tile_size, tile.1 as f32 * tile_size);
    let mut c = Cursor8 { b: raw, at: 0 };
    let (nv, nt) = (c.var()? as usize, c.var()? as usize);
    if nv > raw.len() || nt > raw.len() {
        return Err(bad("ground tile block counts exceed its size"));
    }
    let mut verts = vec![[0i64; 5]; nv];
    for col in 0..5 {
        let mut prev = 0i64;
        for v in verts.iter_mut() {
            prev += c.signed()?;
            v[col] = prev;
        }
    }
    let mut corners = Vec::with_capacity(nt);
    let mut fresh = 0i64;
    for _ in 0..nt {
        let mut ids = [0usize; 3];
        for id in ids.iter_mut() {
            let o = fresh + c.signed()?;
            if o < 0 || o as usize >= nv {
                return Err(bad("ground tile corner outside its vertices"));
            }
            *id = o as usize;
            fresh = fresh.max(o + 1);
        }
        corners.push(ids);
    }
    let mut map_ids = Vec::with_capacity(nt);
    let mut prev = 0i64;
    for _ in 0..nt {
        prev += c.signed()?;
        map_ids.push(prev);
    }
    let seams = c.bytes(nt)?.to_vec();
    let layers = c.bytes(nt)?.to_vec();
    let pos = |v: &[i64; 5]| [ox + v[0] as f32 / XY_STEP, oy + v[1] as f32 / XY_STEP, v[2] as f32 / Z_STEP];
    let uv = |v: &[i64; 5]| [v[3] as f32 / UV_STEP, v[4] as f32 / UV_STEP];
    for (i, ids) in corners.iter().enumerate() {
        if map_ids[i] < 0 || map_ids[i] as usize >= maps {
            continue;
        }
        out.push(GroundTri {
            p: ids.map(|k| pos(&verts[k])),
            uv: ids.map(|k| uv(&verts[k])),
            map: map_ids[i] as u16,
            seam: seams[i],
            layer: layers[i],
        });
    }
    Ok(())
}

/// Whether `write` keeps `t`: a real map, finite and in-range numbers, at
/// least [`MIN_AREA`].
pub fn keeps(t: &GroundTri, maps: usize) -> bool {
    let numbers = t.p.iter().flatten().chain(t.uv.iter().flatten());
    (t.map as usize) < maps && numbers.clone().all(|x| x.is_finite() && x.abs() < LIMIT / UV_STEP) && area(&t.p) >= MIN_AREA
}

/// The tile a triangle lives in (its centroid's).
pub fn tile_of_tri(t: &GroundTri, tile_size: f32) -> (i32, i32) {
    let c = [(t.p[0][0] + t.p[1][0] + t.p[2][0]) / 3.0, (t.p[0][1] + t.p[1][1] + t.p[2][1]) / 3.0];
    (tile_of(c[0], tile_size), tile_of(c[1], tile_size))
}

/// Writes `tris` (those under [`MIN_AREA`], with a bad map or with
/// non-finite or out-of-range numbers are left out, as are repeats) and
/// returns how many were kept.
pub fn write<W: Write>(out: &mut W, maps: &[JointMap], tris: &[GroundTri], tile_size: f32) -> io::Result<usize> {
    if !(tile_size.is_finite() && tile_size > 0.0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "tile size must be positive"));
    }
    let mut tiles: BTreeMap<(i32, i32), Vec<&GroundTri>> = BTreeMap::new();
    for t in tris.iter().filter(|t| keeps(t, maps.len())) {
        tiles.entry(tile_of_tri(t, tile_size)).or_default().push(t);
    }
    let encoded: Vec<((i32, i32), Vec<u8>, usize)> = tiles.iter().map(|(&tile, members)| {
        let (block, n) = encode_tile(members, tile, tile_size);
        (tile, block, n)
    }).collect();
    write_encoded(out, maps, &encoded, tile_size)
}

/// The writer under [`write`], for callers that encode tile by tile
/// (`encode_tile`; tiles sorted by (tx, ty)): returns the triangle count.
pub fn write_encoded<W: Write>(out: &mut W, maps: &[JointMap], tiles: &[((i32, i32), Vec<u8>, usize)], tile_size: f32) -> io::Result<usize> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidInput, m.to_string());
    let kept: usize = tiles.iter().map(|e| e.2).sum();
    let blocks: Vec<Vec<u8>> = tiles.iter().map(|e| e.1.clone()).collect();
    let u32_of = |n: usize| u32::try_from(n).map_err(|_| bad("sidecar exceeds u32 limits"));
    out.write_all(&MAGIC)?;
    out.write_all(&VERSION.to_le_bytes())?;
    out.write_all(&tile_size.to_le_bytes())?;
    out.write_all(&u32_of(maps.len())?.to_le_bytes())?;
    out.write_all(&u32_of(tiles.len())?.to_le_bytes())?;
    out.write_all(&u32_of(kept)?.to_le_bytes())?;
    let mut raw = Vec::new();
    for m in maps {
        let visible = m.visible.as_ref();
        if JointMap::new(m.width, m.height, m.bits.clone(), m.values.clone()).is_none() || visible.is_some_and(|v| v.len() != m.bits.len()) {
            return Err(bad("malformed joint map"));
        }
        raw.extend_from_slice(&m.width.to_le_bytes());
        raw.extend_from_slice(&m.height.to_le_bytes());
        raw.push(m.roughness);
        raw.push(visible.is_some() as u8);
        raw.extend_from_slice(&m.bits);
        raw.extend_from_slice(&m.values);
        if let Some(v) = visible {
            raw.extend_from_slice(v);
        }
    }
    crate::packed::write_section(out, &raw)?;
    // Tile table: tile, packed bytes, raw bytes (offsets follow from the sizes).
    let frames: Vec<Vec<u8>> = crate::packed::compress_with(&blocks, 19)?;
    for ((tile, _, _), (frame, block)) in tiles.iter().zip(frames.iter().zip(&blocks)) {
        for v in [tile.0 as u32, tile.1 as u32, u32_of(frame.len())?, u32_of(block.len())?] {
            out.write_all(&v.to_le_bytes())?;
        }
    }
    for f in &frames {
        out.write_all(f)?;
    }
    Ok(kept)
}

/// Where one tile's triangles are in the file.
#[derive(Clone, Copy)]
enum Tile {
    /// Version 4: fixed 64-byte records, one zstd frame per tile.
    Records(crate::packed::Block),
    /// Version 5: `at`, packed bytes, raw bytes.
    Coded(u64, u32, u32),
}

/// An open sidecar: joint maps and the tile table are resident, triangles
/// are read on demand.
pub struct Ground<R> {
    reader: R,
    pub tile_size: f32,
    pub maps: Vec<JointMap>,
    /// Maps in the file (`maps` may be taken by the caller).
    pub map_count: usize,
    pub triangle_count: u32,
    tiles: HashMap<(i32, i32), Tile>,
}

impl Ground<BufReader<File>> {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::from_reader(BufReader::new(File::open(path)?))
    }
}

impl<R: Read + Seek> Ground<R> {
    pub fn from_reader(mut reader: R) -> io::Result<Self> {
        let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
        let mut magic = [0; 4];
        reader.read_exact(&mut magic)?;
        if magic != MAGIC {
            return Err(bad("not an SVGJ ground sidecar"));
        }
        let version = read_u32(&mut reader)?;
        if !(4..=VERSION).contains(&version) {
            return Err(bad(&format!("SVGJ version {version}, expected {VERSION}; rebake the ground")));
        }
        let tile_size = f32::from_bits(read_u32(&mut reader)?);
        let map_count = read_u32(&mut reader)?;
        let tile_count = read_u32(&mut reader)?;
        let triangle_count = read_u32(&mut reader)?;
        if !(tile_size.is_finite() && tile_size > 0.0) {
            return Err(bad("invalid tile size"));
        }
        let section = crate::packed::read_section(&mut reader)?;
        let mut maps_from = io::Cursor::new(section);
        let mut maps = Vec::with_capacity(map_count as usize);
        for _ in 0..map_count {
            let mut wh = [0u8; 5];
            maps_from.read_exact(&mut wh)?;
            let (width, height) = (u16::from_le_bytes([wh[0], wh[1]]), u16::from_le_bytes([wh[2], wh[3]]));
            if width == 0 || height == 0 {
                return Err(bad("empty joint map"));
            }
            let mut flags = [0u8; 1];
            if version >= 5 {
                maps_from.read_exact(&mut flags)?;
            }
            let mut bits = vec![0u8; (width as usize).div_ceil(8) * height as usize];
            maps_from.read_exact(&mut bits)?;
            let mut values = vec![0u8; bits.iter().map(|b| b.count_ones() as usize).sum()];
            maps_from.read_exact(&mut values)?;
            let visible = if flags[0] & 1 == 1 {
                let mut v = vec![0u8; bits.len()];
                maps_from.read_exact(&mut v)?;
                Some(v)
            } else {
                None
            };
            let mut map = JointMap::new(width, height, bits, values).ok_or_else(|| bad("malformed joint map"))?;
            map.roughness = wh[4];
            map.visible = visible;
            maps.push(map);
        }
        let mut tiles = HashMap::with_capacity(tile_count as usize);
        if version >= 5 {
            let mut table = Vec::with_capacity(tile_count as usize);
            for _ in 0..tile_count {
                let tx = read_u32(&mut reader)? as i32;
                let ty = read_u32(&mut reader)? as i32;
                table.push(((tx, ty), read_u32(&mut reader)?, read_u32(&mut reader)?));
            }
            let mut at = reader.stream_position()?;
            for (tile, packed, raw) in table {
                tiles.insert(tile, Tile::Coded(at, packed, raw));
                at += packed as u64;
            }
        } else {
            let table = crate::packed::read_table(&mut reader, tile_count, triangle_count)?;
            let blocks = crate::packed::read_index(&mut reader, &table.iter().map(|t| t.2).collect::<Vec<_>>())?;
            tiles.extend(table.iter().map(|t| t.0).zip(blocks.into_iter().map(Tile::Records)));
        }
        Ok(Self { reader, tile_size, map_count: maps.len(), maps, triangle_count, tiles })
    }

    /// The tiles in the file.
    pub fn tile_keys(&self) -> Vec<(i32, i32)> {
        let mut keys: Vec<_> = self.tiles.keys().copied().collect();
        keys.sort_unstable();
        keys
    }

    /// The triangles whose centroid lies in `tile` (none for a missing
    /// tile; triangles naming a missing map are dropped).
    pub fn read_tile(&mut self, tile: (i32, i32)) -> io::Result<Vec<GroundTri>> {
        let mut out = Vec::new();
        let Some(&entry) = self.tiles.get(&tile) else { return Ok(out) };
        let mut buf = Vec::new();
        match entry {
            Tile::Coded(at, packed, raw) => {
                self.reader.seek(SeekFrom::Start(at))?;
                let mut frame = vec![0u8; packed as usize];
                self.reader.read_exact(&mut frame)?;
                buf = zstd::bulk::decompress(&frame, raw as usize)?;
                decode_tile(&buf, tile, self.tile_size, &mut out, self.map_count)?;
            }
            Tile::Records(block) => {
                crate::packed::read(&mut self.reader, block, TRIANGLE_BYTES, &mut buf)?;
                for rec in buf.as_chunks::<TRIANGLE_BYTES>().0 {
                    let f = |i: usize| f32::from_le_bytes(rec[4 * i..4 * i + 4].try_into().unwrap());
                    let t = GroundTri {
                        p: std::array::from_fn(|v| std::array::from_fn(|k| f(3 * v + k))),
                        uv: std::array::from_fn(|v| std::array::from_fn(|k| f(9 + 2 * v + k))),
                        map: u16::from_le_bytes([rec[60], rec[61]]),
                        seam: rec[62],
                        layer: 0,
                    };
                    if (t.map as usize) < self.map_count {
                        out.push(t);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Triangles whose centroid lies in a tile overlapping the XY square of
    /// half-size `radius` around `center`. Triangles naming a missing map
    /// are dropped.
    pub fn query(&mut self, center: [f32; 2], radius: f32) -> io::Result<Vec<GroundTri>> {
        let s = self.tile_size;
        let mut out = Vec::new();
        for tx in tile_of(center[0] - radius, s)..=tile_of(center[0] + radius, s) {
            for ty in tile_of(center[1] - radius, s)..=tile_of(center[1] + radius, s) {
                out.extend(self.read_tile((tx, ty))?);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_maps_and_triangles_by_tile() {
        // Joints on column 0 and row 2 of a 10 x 4 map.
        let map = JointMap::from_mask(10, 4, |x, y| x == 0 || y == 2);
        let t = |x: f32| GroundTri {
            p: [[x, 0.0, 1.0], [x + 1.0, 0.0, 1.0], [x, 1.0, 1.0]],
            uv: [[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]],
            map: 0,
            seam: 12,
            layer: 0,
        };
        let bad_map = GroundTri { map: 5, ..t(2.0) };
        let mut bytes = Vec::new();
        assert_eq!(write(&mut bytes, std::slice::from_ref(&map), &[t(1.0), t(300.0), bad_map], 64.0).unwrap(), 2);
        let mut g = Ground::from_reader(Cursor::new(bytes)).unwrap();
        assert_eq!(g.maps, vec![map.clone()]);
        assert_eq!(g.query([0.0, 0.0], 10.0).unwrap(), vec![t(1.0)]);
        assert_eq!(g.query([300.0, 0.0], 10.0).unwrap(), vec![t(300.0)]);
        assert!(map.at(0.01, 0.1) && map.at(1.03, 7.1), "column 0 wraps");
        assert!(map.at(0.55, 0.6) && map.at(-0.45, -0.4), "row 2, negative UVs wrap");
        assert!(!map.at(0.55, 0.1));
    }

    #[test]
    fn lines_take_their_deepest_level() {
        // A full-width groove of mixed depth and a lone speck.
        let mut map = JointMap::from_values(10, 4, |x, y| match (x, y) {
            (_, 1) => GROOVE << 6 | (x as u8 + 1),
            (5, 3) => PIT << 6 | 4,
            _ => 0,
        });
        map.level_lines(0.5);
        assert_eq!(map.value(0.05, 0.3), GROOVE << 6 | 10, "the line, at its deepest");
        assert_eq!(map.value(0.55, 0.8), PIT << 6 | 4, "the speck keeps its own level");
    }

    #[test]
    fn values_follow_their_texels() {
        // Depth grows along x on row 1 (bits straddle a byte); a pit at (9, 3).
        let mut map = JointMap::from_values(10, 4, |x, y| match (x, y) {
            (_, 1) => GROOVE << 6 | (x as u8 + 1),
            (9, 3) => PIT << 6 | 40,
            _ => 0,
        });
        map.roughness = 200;
        let t = GroundTri { p: [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], uv: [[0.0; 2]; 3], map: 0, seam: 11, layer: 0 };
        let mut bytes = Vec::new();
        write(&mut bytes, &[map.clone()], &[t], 64.0).unwrap();
        let g = Ground::from_reader(Cursor::new(bytes)).unwrap();
        for x in 0..10u8 {
            assert_eq!(g.maps[0].value((x as f32 + 0.5) / 10.0, 0.3), GROOVE << 6 | (x + 1));
        }
        assert_eq!(g.maps[0].value(0.95, 0.9), PIT << 6 | 40);
        assert_eq!(g.maps[0].value(0.85, 0.9), 0);
        assert_eq!(g.maps[0].roughness, 200);
        assert!(JointMap::new(10, 4, map.bits.clone(), vec![1]).is_none(), "one value per set bit");
    }

    #[test]
    fn v5_shares_vertices_drops_specks_and_keeps_layers() {
        let mut overlay = JointMap::from_mask(16, 4, |x, _| x == 3);
        overlay.visible = Some(vec![0b0000_1111, 0, 0b0000_1111, 0, 0b0000_1111, 0, 0b0000_1111, 0]);
        let plain = JointMap::from_mask(8, 8, |x, y| x == y);
        // A quad of two triangles sharing an edge, the same quad again as
        // an overlay, a speck, and an exact repeat.
        let quad = |layer: u8, map: u16, z: f32| {
            [
                GroundTri { p: [[1.0, 1.0, z], [3.0, 1.0, z], [1.0, 3.0, z]], uv: [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]], map, seam: 11, layer },
                GroundTri { p: [[3.0, 1.0, z], [3.0, 3.0, z], [1.0, 3.0, z]], uv: [[4.0, 0.0], [4.0, 4.0], [0.0, 4.0]], map, seam: 11, layer },
            ]
        };
        let base = quad(0, 1, 5.0);
        let over = quad(1, 0, 5.01);
        let speck = GroundTri { p: [[7.0, 7.0, 5.0], [7.05, 7.0, 5.0], [7.0, 7.05, 5.0]], uv: [[0.0; 2]; 3], map: 1, seam: 11, layer: 0 };
        let all = [base[0], base[1], over[0], over[1], speck, base[0]];
        let mut bytes = Vec::new();
        assert_eq!(write(&mut bytes, &[overlay.clone(), plain.clone()], &all, 64.0).unwrap(), 4, "speck and repeat left out");
        let mut g = Ground::from_reader(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(g.maps[0].visible, overlay.visible);
        assert!(g.maps[0].visible_at(0.05, 0.1) && !g.maps[0].visible_at(0.55, 0.1) && g.maps[1].visible_at(0.9, 0.9));
        let mut got = g.query([2.0, 2.0], 10.0).unwrap();
        let key = |t: &GroundTri| (t.layer, t.p[0][0] as i32, t.p[1][1] as i32);
        got.sort_by_key(key);
        let mut want = vec![base[0], base[1], over[0], over[1]];
        want.sort_by_key(key);
        assert_eq!(got.len(), 4);
        for (a, b) in got.iter().zip(&want) {
            assert_eq!((a.layer, a.map, a.seam), (b.layer, b.map, b.seam));
            for k in 0..3 {
                assert!(a.p[k].iter().zip(&b.p[k]).all(|(x, y)| (x - y).abs() < 0.01), "{:?} vs {:?}", a.p, b.p);
                assert!(a.uv[k].iter().zip(&b.uv[k]).all(|(x, y)| (x - y).abs() < 0.0002));
            }
        }
        // 4 triangles, 4 shared corners per layer: smaller than the raw records.
        assert!(bytes.len() < 4 * TRIANGLE_BYTES + 300, "{}", bytes.len());
    }

    #[test]
    fn rejects_wrong_magic_and_version() {
        assert!(Ground::from_reader(Cursor::new(b"SVSD\x01\0\0\0".to_vec())).is_err());
        assert!(Ground::from_reader(Cursor::new(b"SVGJ\x02\0\0\0".to_vec())).is_err());
    }
}

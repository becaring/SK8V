//! SkateV surface sidecar (`SVSD`), version 2: per collision triangle sound
//! surface recovered offline from GTA's visual materials
//! (`docs/MATERIAL-RECOVERY.md`, `tools/material-recovery`). It sits next to
//! an SVWC cache and refers to its triangles by the exact vertex bits, so it
//! never changes collision geometry or physics bytes.
//!
//! ```text
//! 4 bytes  magic "SVSD"
//! u32      version = 2
//! f32      tile size (metres, XY; a record lives in its centroid's tile)
//! u32      tile count
//! u32      record count
//! repeat tile count (sorted by tx, ty):
//!   i32 tx, i32 ty, u32 first record, u32 record count
//! block index and one zstd frame per tile (`packed.rs`) of its records:
//!   u64    key: FNV-1a 64 over the triangle's nine f32 vertex bit patterns
//!   u8     Skate audio surface row (0-based AudioSurfaceMap row), 0xFF = keep
//!   u8     Skate seam type (1..=15), 0 = none, 0xFF = keep
//!   u8     line families (0..=2)
//!   u8     reserved
//!   2 x { f32 nx, f32 ny, f32 spacing, f32 phase }
//! ```
//!
//! A line family is the set of world lines `n . p = phase + k * spacing`
//! (GTA XY, metres, `n` a unit vector); clicks are counted on these lines
//! instead of the seam type's authored grid. All values are little-endian.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, Write};
use std::path::Path;

use crate::packed::tile_of;

pub const MAGIC: [u8; 4] = *b"SVSD";
pub const VERSION: u32 = 2;
pub const KEEP: u8 = 0xFF;
pub const RECORD_BYTES: u64 = 44;

/// World lines `n . p = phase + k * spacing` (GTA XY).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Family {
    pub normal: [f32; 2],
    pub spacing: f32,
    pub phase: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub key: u64,
    /// Skate audio row (0-based), or `KEEP`.
    pub row: u8,
    /// Skate seam type, 0 for none, or `KEEP`.
    pub seam: u8,
    pub families: [Option<Family>; 2],
}

/// The key of an SVWC triangle (FNV-1a 64 over its vertex bits).
pub fn key(v: &[[f32; 3]; 3]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for x in v.iter().flatten() {
        for b in x.to_bits().to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Writes `(centroid XY, surface)` records as an SVSD sidecar.
pub fn write<W: Write>(out: &mut W, records: &[([f32; 2], Surface)], tile_size: f32) -> io::Result<usize> {
    if !(tile_size.is_finite() && tile_size > 0.0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "tile size must be positive"));
    }
    let mut tiles: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
    for (i, (c, _)) in records.iter().enumerate() {
        if c.iter().all(|x| x.is_finite()) {
            tiles.entry((tile_of(c[0], tile_size), tile_of(c[1], tile_size))).or_default().push(i);
        }
    }
    let count: usize = tiles.values().map(Vec::len).sum();
    let too_big = || io::Error::new(io::ErrorKind::InvalidInput, "sidecar exceeds u32 limits");
    out.write_all(&MAGIC)?;
    out.write_all(&VERSION.to_le_bytes())?;
    out.write_all(&tile_size.to_le_bytes())?;
    out.write_all(&u32::try_from(tiles.len()).map_err(|_| too_big())?.to_le_bytes())?;
    out.write_all(&u32::try_from(count).map_err(|_| too_big())?.to_le_bytes())?;
    let mut first = 0u32;
    for (&(tx, ty), members) in &tiles {
        for v in [tx as u32, ty as u32, first, members.len() as u32] {
            out.write_all(&v.to_le_bytes())?;
        }
        first += members.len() as u32;
    }
    let blocks: Vec<Vec<u8>> = tiles.values().map(|members| {
        let mut b = Vec::with_capacity(members.len() * RECORD_BYTES as usize);
        for &i in members {
            let s = &records[i].1;
            b.extend_from_slice(&s.key.to_le_bytes());
            let n = s.families.iter().flatten().count() as u8;
            b.extend_from_slice(&[s.row, s.seam, n, 0]);
            let mut fams = s.families.iter().flatten();
            for _ in 0..2 {
                let f = fams.next().copied().unwrap_or(Family { normal: [0.0; 2], spacing: 0.0, phase: 0.0 });
                for x in [f.normal[0], f.normal[1], f.spacing, f.phase] {
                    b.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
        b
    }).collect();
    crate::packed::write(out, &blocks)?;
    Ok(count)
}

fn parse(rec: &[u8]) -> Surface {
    let f = |i: usize| f32::from_le_bytes(rec[i..i + 4].try_into().unwrap());
    let n = rec[10];
    let fam = |k: usize| {
        let at = 12 + 16 * k;
        let fam = Family { normal: [f(at), f(at + 4)], spacing: f(at + 8), phase: f(at + 12) };
        // Only well-formed families: a bad record never reaches the clicks.
        (k < n as usize && fam.spacing.is_finite() && fam.spacing > 0.01 && fam.phase.is_finite()
            && (fam.normal[0].hypot(fam.normal[1]) - 1.0).abs() < 1e-3)
            .then_some(fam)
    };
    Surface {
        key: u64::from_le_bytes(rec[0..8].try_into().unwrap()),
        row: rec[8],
        seam: rec[9],
        families: [fam(0), fam(1)],
    }
}

/// An open sidecar: the tile table is resident, records are read on demand.
pub struct Sidecar<R> {
    reader: R,
    pub tile_size: f32,
    pub record_count: u32,
    tiles: HashMap<(i32, i32), crate::packed::Block>,
}

impl Sidecar<BufReader<File>> {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::from_reader(BufReader::new(File::open(path)?))
    }
}

impl<R: Read + Seek> Sidecar<R> {
    pub fn from_reader(mut reader: R) -> io::Result<Self> {
        let (tile_size, record_count, tiles) =
            crate::packed::read_tiles(&mut reader, MAGIC, VERSION, (1, RECORD_BYTES as usize), "SVSD surface sidecar", "rebake the surfaces")?;
        Ok(Self { reader, tile_size, record_count, tiles })
    }

    /// Records whose triangle centroid lies in a tile overlapping the XY
    /// square of half-size `radius` around `center`, by key.
    pub fn query(&mut self, center: [f32; 2], radius: f32) -> io::Result<HashMap<u64, Surface>> {
        let s = self.tile_size;
        let mut out = HashMap::new();
        let mut buf = Vec::new();
        for tx in tile_of(center[0] - radius, s)..=tile_of(center[0] + radius, s) {
            for ty in tile_of(center[1] - radius, s)..=tile_of(center[1] + radius, s) {
                let Some(&block) = self.tiles.get(&(tx, ty)) else { continue };
                crate::packed::read(&mut self.reader, block, RECORD_BYTES as usize, &mut buf)?;
                for rec in buf.as_chunks::<{ RECORD_BYTES as usize }>().0 {
                    let r = parse(rec);
                    out.insert(r.key, r);
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn tri(x: f32) -> [[f32; 3]; 3] {
        [[x, 0.0, 1.0], [x + 1.0, 0.0, 1.0], [x, 1.0, 1.0]]
    }

    #[test]
    fn roundtrip_by_triangle_key_and_tile() {
        let fam = Family { normal: [0.6, 0.8], spacing: 1.5, phase: 0.25 };
        let a = Surface { key: key(&tri(1.0)), row: 3, seam: 11, families: [Some(fam), None] };
        let b = Surface { key: key(&tri(300.0)), row: KEEP, seam: KEEP, families: [None, None] };
        let mut bytes = Vec::new();
        assert_eq!(write(&mut bytes, &[([1.3, 0.3], a), ([300.3, 0.3], b)], 64.0).unwrap(), 2);
        let mut s = Sidecar::from_reader(Cursor::new(bytes)).unwrap();
        let near = s.query([0.0, 0.0], 10.0).unwrap();
        assert_eq!(near.len(), 1);
        assert_eq!(near[&a.key], a);
        assert_eq!(s.query([300.0, 0.0], 10.0).unwrap()[&b.key], b);
    }

    #[test]
    fn key_follows_exact_vertex_bits() {
        assert_eq!(key(&tri(1.0)), key(&tri(1.0)));
        assert_ne!(key(&tri(1.0)), key(&tri(1.0 + f32::EPSILON)));
    }

    #[test]
    fn malformed_families_are_dropped_on_read() {
        let bad = Family { normal: [1.0, 1.0], spacing: 1.0, phase: 0.0 };
        let tiny = Family { normal: [1.0, 0.0], spacing: 0.0, phase: 0.0 };
        let s = Surface { key: 7, row: 2, seam: 3, families: [Some(bad), Some(tiny)] };
        let mut bytes = Vec::new();
        write(&mut bytes, &[([0.0, 0.0], s)], 64.0).unwrap();
        let got = Sidecar::from_reader(Cursor::new(bytes)).unwrap().query([0.0, 0.0], 1.0).unwrap();
        assert_eq!(got[&7].families, [None, None]);
    }

    #[test]
    fn rejects_wrong_magic_and_version() {
        assert!(Sidecar::from_reader(Cursor::new(b"SVSD   ".to_vec())).is_err());
        assert!(Sidecar::from_reader(Cursor::new(b"SVWC\x01\0\0\0".to_vec())).is_err());
    }
}

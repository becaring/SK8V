//! SkateV world cache (`SVWC`), version 3: project-owned, tiled triangle soup in
//! GTA world space (metres, Z up). The runtime loads only the tiles around the
//! skater; the whole Los Santos collision never has to be resident.
//!
//! ```text
//! 4 bytes  magic "SVWC"
//! u32      version = 3
//! f32      tile size (metres, XY)
//! u32      tile count
//! u32      record count
//! repeat tile count (sorted by tx, ty):
//!   i32 tx, i32 ty, u32 first record, u32 record count
//! block index and one zstd frame per tile (`packed.rs`) of its records:
//!   9*f32  vertices a, b, c (x, y, z)
//!   u8     GTA material id, 3*u8 reserved
//! ```
//!
//! A triangle is stored in every tile its XY bounding box overlaps, so a query
//! never misses a large triangle; `query` removes the duplicates.
//! All values are little-endian.

pub mod bounds;
pub mod clean;
pub mod ground;
pub mod surfaces;
mod packed;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, Write};
use std::path::Path;

use packed::tile_of;

pub const MAGIC: [u8; 4] = *b"SVWC";
pub const VERSION: u32 = 3;
pub const DEFAULT_TILE_SIZE: f32 = 64.0;
pub const RECORD_BYTES: u64 = 40;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tri {
    pub v: [[f32; 3]; 3],
    pub material: u8,
}

impl Tri {
    fn key(&self) -> [u32; 9] {
        let mut k = [0; 9];
        for (i, p) in self.v.iter().flatten().enumerate() {
            k[i] = p.to_bits();
        }
        k
    }
}

/// Writes `tris` as an SVWC cache. Non-finite triangles are dropped.
pub fn write<W: Write>(out: &mut W, tris: &[Tri], tile_size: f32) -> io::Result<usize> {
    if !(tile_size.is_finite() && tile_size > 0.0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tile size must be positive",
        ));
    }
    let mut tiles: BTreeMap<(i32, i32), Vec<u32>> = BTreeMap::new();
    for (i, t) in tris.iter().enumerate() {
        if !t.v.iter().flatten().all(|x| x.is_finite()) {
            continue;
        }
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in &t.v {
            for a in 0..2 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
        for tx in tile_of(min[0], tile_size)..=tile_of(max[0], tile_size) {
            for ty in tile_of(min[1], tile_size)..=tile_of(max[1], tile_size) {
                tiles.entry((tx, ty)).or_default().push(i as u32);
            }
        }
    }
    let records: usize = tiles.values().map(Vec::len).sum();
    let too_big = || io::Error::new(io::ErrorKind::InvalidInput, "cache exceeds u32 limits");
    let tile_count = u32::try_from(tiles.len()).map_err(|_| too_big())?;
    let record_count = u32::try_from(records).map_err(|_| too_big())?;

    out.write_all(&MAGIC)?;
    out.write_all(&VERSION.to_le_bytes())?;
    out.write_all(&tile_size.to_le_bytes())?;
    out.write_all(&tile_count.to_le_bytes())?;
    out.write_all(&record_count.to_le_bytes())?;
    let mut first = 0u32;
    for (&(tx, ty), members) in &tiles {
        out.write_all(&tx.to_le_bytes())?;
        out.write_all(&ty.to_le_bytes())?;
        out.write_all(&first.to_le_bytes())?;
        out.write_all(&(members.len() as u32).to_le_bytes())?;
        first += members.len() as u32;
    }
    let blocks: Vec<Vec<u8>> = tiles.values().map(|members| {
        let mut b = Vec::with_capacity(members.len() * RECORD_BYTES as usize);
        for &i in members {
            let t = &tris[i as usize];
            for x in t.v.iter().flatten() {
                b.extend_from_slice(&x.to_le_bytes());
            }
            b.extend_from_slice(&[t.material, 0, 0, 0]);
        }
        b
    }).collect();
    packed::write(out, &blocks)?;
    Ok(records)
}

/// An open cache: the tile table is resident, records are read on demand.
pub struct Cache<R> {
    reader: R,
    pub tile_size: f32,
    pub record_count: u32,
    tiles: HashMap<(i32, i32), packed::Block>,
}

impl Cache<BufReader<File>> {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::from_reader(BufReader::new(File::open(path)?))
    }
}

impl<R: Read + Seek> Cache<R> {
    pub fn from_reader(mut reader: R) -> io::Result<Self> {
        let (tile_size, record_count, tiles) =
            // ponytail: v2 (raw) stays readable because the installed
            // los-santos-packfix cache and its prop models are v2; drop after a rebuild.
            packed::read_tiles(&mut reader, MAGIC, VERSION, (2, RECORD_BYTES as usize), "SVWC cache", "rebuild the cache")?;
        Ok(Self { reader, tile_size, record_count, tiles })
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Unique triangles in every tile overlapping the XY square of half-size
    /// `radius` around `center`.
    pub fn query(&mut self, center: [f32; 2], radius: f32) -> io::Result<Vec<Tri>> {
        let s = self.tile_size;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut buf = Vec::new();
        for tx in tile_of(center[0] - radius, s)..=tile_of(center[0] + radius, s) {
            for ty in tile_of(center[1] - radius, s)..=tile_of(center[1] + radius, s) {
                let Some(&block) = self.tiles.get(&(tx, ty)) else {
                    continue;
                };
                packed::read(&mut self.reader, block, RECORD_BYTES as usize, &mut buf)?;
                for rec in buf.as_chunks::<{ RECORD_BYTES as usize }>().0 {
                    let f =
                        |i: usize| f32::from_le_bytes(rec[i * 4..i * 4 + 4].try_into().unwrap());
                    let t = Tri {
                        v: [[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]],
                        material: rec[36],
                    };
                    if seen.insert(t.key()) {
                        out.push(t);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Reads a small model cache without assuming an origin or bounding radius.
    /// Tile duplication is removed in file order, just as with spatial queries.
    pub fn read_all(&mut self) -> io::Result<Vec<Tri>> {
        let mut blocks: Vec<packed::Block> = self.tiles.values().copied().collect();
        blocks.sort_by_key(|b| b.at);
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut buf = Vec::new();
        for block in blocks {
            packed::read(&mut self.reader, block, RECORD_BYTES as usize, &mut buf)?;
            for rec in buf.as_chunks::<{ RECORD_BYTES as usize }>().0 {
                let f = |i: usize| f32::from_le_bytes(rec[i * 4..i * 4 + 4].try_into().unwrap());
                let t = Tri { v: [[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]], material: rec[36] };
                if !t.v.iter().flatten().all(|v| v.is_finite()) {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "nonfinite model triangle"));
                }
                if seen.insert(t.key()) { out.push(t); }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn tri(x: f32, y: f32, size: f32, material: u8) -> Tri {
        Tri {
            v: [[x, y, 1.0], [x + size, y, 1.0], [x, y + size, 1.0]],
            material,
        }
    }

    fn roundtrip(tris: &[Tri], tile: f32) -> Cache<Cursor<Vec<u8>>> {
        let mut bytes = Vec::new();
        write(&mut bytes, tris, tile).unwrap();
        Cache::from_reader(Cursor::new(bytes)).unwrap()
    }

    #[test]
    fn header_layout_is_stable() {
        let mut bytes = Vec::new();
        let records = write(&mut bytes, &[tri(1.0, 1.0, 1.0, 7)], 64.0).unwrap();
        assert_eq!(records, 1);
        assert_eq!(&bytes[0..4], b"SVWC");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 3);
        assert_eq!(f32::from_le_bytes(bytes[8..12].try_into().unwrap()), 64.0);
        assert_eq!(roundtrip(&[tri(1.0, 1.0, 1.0, 7)], 64.0).read_all().unwrap(), vec![tri(1.0, 1.0, 1.0, 7)]);
    }

    #[test]
    fn packs_smaller_than_raw() {
        let tris: Vec<Tri> = (0..2000).map(|i| tri((i % 50) as f32, (i / 50) as f32, 1.0, 3)).collect();
        let mut bytes = Vec::new();
        write(&mut bytes, &tris, 64.0).unwrap();
        assert!((bytes.len() as u64) < 2000 * RECORD_BYTES / 2, "{} bytes", bytes.len());
        assert_eq!(roundtrip(&tris, 64.0).query([25.0, 20.0], 64.0).unwrap().len(), 2000);
    }

    #[test]
    fn query_returns_only_nearby_tiles_and_preserves_data() {
        let near = tri(10.0, 10.0, 2.0, 3);
        let far = tri(1000.0, -1000.0, 2.0, 4);
        let mut cache = roundtrip(&[near, far], 64.0);
        assert_eq!(cache.query([0.0, 0.0], 50.0).unwrap(), vec![near]);
        assert_eq!(cache.query([1000.0, -1000.0], 10.0).unwrap(), vec![far]);
        assert!(cache.query([5000.0, 5000.0], 10.0).unwrap().is_empty());
    }

    #[test]
    fn large_triangle_spanning_tiles_is_found_once_from_any_tile() {
        let big = tri(-100.0, -100.0, 300.0, 1);
        let mut cache = roundtrip(&[big], 64.0);
        assert!(cache.tile_count() > 4);
        assert_eq!(cache.query([-90.0, -90.0], 1.0).unwrap(), vec![big]);
        assert_eq!(cache.query([0.0, 0.0], 200.0).unwrap(), vec![big]);
    }

    #[test]
    fn negative_coordinates_floor_into_correct_tiles() {
        let t = tri(-0.5, -0.5, 0.25, 0);
        let mut cache = roundtrip(&[t], 64.0);
        assert_eq!(cache.query([-1.0, -1.0], 0.5).unwrap(), vec![t]);
        assert!(cache.query([32.0, 32.0], 0.5).unwrap().is_empty());
    }

    #[test]
    fn rejects_wrong_magic_and_version() {
        assert!(Cache::from_reader(Cursor::new(b"NOPE\x02\0\0\0".to_vec())).is_err());
        let mut bytes = Vec::new();
        write(&mut bytes, &[], 64.0).unwrap();
        bytes[4] = 1;
        let err = Cache::from_reader(Cursor::new(bytes)).err().unwrap();
        assert!(err.to_string().contains("rebuild"), "{err}");
    }
}

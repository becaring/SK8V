//! Recovered sound surfaces (`<cache>.svsd`, docs/MATERIAL-RECOVERY.md).
//!
//! The sidecar names, per SVWC collision triangle, the Skate audio row and
//! seam type recovered from GTA's visual materials, and the world lines of the
//! visible joints. Area builds apply the row/seam to the triangle's audio tag
//! (sound only: physics bits stay untouched) and publish the joint lines of
//! the area, which the audio tick looks up under each wheel so seam clicks
//! land on the joints the player sees (docs/DECISIONS.md, 2026-10-04).
//! Without a sidecar everything stays on `materials::surface`.

use crate::grid::{weights, XyGrid};
use bevy_math::Vec3;
use skate_aems::live::frame::{Field, Frame, SeamFamily, SeamGrid};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use svwc::surfaces::{Sidecar, Surface, KEEP};

/// The sidecar beside an SVWC cache.
pub fn sidecar_path(cache: &Path) -> PathBuf {
    cache.with_extension("svsd")
}

/// An open sidecar (absent or unreadable: no recovery, logged by status).
pub struct Recovered {
    sidecar: Option<Sidecar<std::io::BufReader<std::fs::File>>>,
    status: String,
}

impl Recovered {
    /// Never fails: a missing or bad sidecar leaves GTA's material mapping.
    pub fn open(cache: &Path) -> Self {
        let path = sidecar_path(cache);
        if !path.exists() {
            return Self { sidecar: None, status: format!("no recovered surfaces ({} absent)", path.display()) };
        }
        match Sidecar::open(&path) {
            Ok(s) => {
                let status = format!("recovered surfaces {} ({} records)", path.display(), s.record_count);
                Self { sidecar: Some(s), status }
            }
            Err(e) => Self { sidecar: None, status: format!("recovered surfaces ignored: {}: {e}", path.display()) },
        }
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn none() -> Self {
        Self { sidecar: None, status: "no recovered surfaces".into() }
    }

    pub fn is_loaded(&self) -> bool {
        self.sidecar.is_some()
    }

    /// Records for triangles around `center` (one tile of margin, since a
    /// record lives in its centroid's tile).
    pub fn around(&mut self, center: [f32; 2], radius: f32) -> Result<HashMap<u64, Surface>, String> {
        match &mut self.sidecar {
            None => Ok(HashMap::new()),
            Some(s) => {
                let margin = s.tile_size;
                s.query(center, radius + margin).map_err(|e| format!("surface sidecar read: {e}"))
            }
        }
    }
}

/// The audio tag of a collision triangle: GTA's material mapping, with the
/// recovered row and seam type when the sidecar has them.
pub fn tag(material: u8, recovered: Option<&Surface>) -> u32 {
    let mut s = crate::materials::surface(material);
    if let Some(r) = recovered {
        if r.row != KEEP && r.row < 95 {
            s.audio = r.row + 1;
        }
        if r.seam != KEEP && r.seam <= 15 {
            s.seam = r.seam;
        }
    }
    s.packed()
}

/// A collision triangle carrying recovered joint lines (GTA space).
#[derive(Clone, Debug)]
struct GridTri {
    v: [[f32; 3]; 3],
    families: [Option<svwc::surfaces::Family>; 2],
}

/// The current area's joint lines, bucketed by 4 m XY cells.
#[derive(Default, Debug)]
pub struct GridIndex {
    tris: Vec<GridTri>,
    cells: XyGrid,
}

const CELL: f32 = 4.0;
/// Wheel centre height above the ground it may be standing on.
const DZ: f32 = 0.5;

impl GridIndex {
    pub fn build(tris: impl Iterator<Item = ([[f32; 3]; 3], [Option<svwc::surfaces::Family>; 2])>) -> Self {
        let mut g = GridIndex { tris: Vec::new(), cells: XyGrid::new(CELL) };
        for (v, families) in tris {
            if families.iter().all(Option::is_none) {
                continue;
            }
            let id = g.tris.len() as u32;
            let lo = |a: usize| v.iter().map(|p| p[a]).fold(f32::MAX, f32::min);
            let hi = |a: usize| v.iter().map(|p| p[a]).fold(f32::MIN, f32::max);
            g.cells.insert(id, Vec3::new(lo(0), lo(1), 0.0), Vec3::new(hi(0), hi(1), 0.0));
            g.tris.push(GridTri { v, families });
        }
        g
    }

    pub fn len(&self) -> usize {
        self.tris.len()
    }

    /// The joint lines under a GTA-space point: the highest grid triangle
    /// whose XY footprint holds the point, at or up to `DZ` below it.
    pub fn at(&self, p: Vec3) -> Option<[Option<svwc::surfaces::Family>; 2]> {
        let mut best: Option<(f32, u32)> = None;
        for &id in self.cells.at(p.x, p.y) {
            let t = &self.tris[id as usize];
            let Some(z) = height(t, p) else { continue };
            let dz = p.z - z;
            if (-0.1..=DZ).contains(&dz) && best.is_none_or(|b| dz < b.0) {
                best = Some((dz, id));
            }
        }
        best.map(|(_, id)| self.tris[id as usize].families)
    }
}

fn height(t: &GridTri, p: Vec3) -> Option<f32> {
    let [a, b, c] = t.v;
    let [l0, l1, l2] = weights(&t.v, p.x, p.y, 1e-9, 1e-4)?;
    Some(l0 * a[2] + l1 * b[2] + l2 * c[2])
}

fn current() -> &'static RwLock<Arc<GridIndex>> {
    static SLOT: OnceLock<RwLock<Arc<GridIndex>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(Arc::new(GridIndex::default())))
}

/// Makes `index` the grid the audio tick reads (the latest built area).
pub fn publish(index: GridIndex) {
    if let Ok(mut g) = current().write() {
        *g = Arc::new(index);
    }
}

/// Skate-space line family for GTA lines `n . p = phase (mod spacing)`:
/// Skate (x, z) = GTA (x, -y), so `n . p = nx * x - ny * z`.
fn to_skate(f: svwc::surfaces::Family) -> SeamFamily {
    SeamFamily { a: f.normal[0], b: -f.normal[1], spacing: f.spacing, phase: f.phase }
}

/// Fills `frame.seam_grids` from the published grid under each wheel (wheel
/// centres at skater `+384 + 16 * w`, Skate space).
pub fn apply_to_frame(frame: &mut Frame) {
    let Ok(index) = current().read().map(|g| Arc::clone(&g)) else { return };
    if index.len() == 0 {
        frame.seam_grids = [None; 4];
        return;
    }
    let f32_at = |off: usize| {
        frame.fields.iter().rev().find_map(|&(o, f)| match f {
            Field::F32(v) if o == off => Some(v),
            _ => None,
        })
    };
    let mut grids = [None; 4];
    for (w, g) in grids.iter_mut().enumerate() {
        let base = 384 + 16 * w;
        let (Some(x), Some(y), Some(z)) = (f32_at(base), f32_at(base + 4), f32_at(base + 8)) else { continue };
        let p = crate::coords::from_skate(Vec3::new(x, y, z));
        *g = index.at(p).map(|fams| SeamGrid { families: fams.map(|f| f.map(to_skate)) });
    }
    frame.seam_grids = grids;
}

#[cfg(test)]
mod tests {
    use super::*;
    use svwc::surfaces::Family;

    fn floor(x: f32, y: f32, z: f32, size: f32) -> [[f32; 3]; 3] {
        [[x, y, z], [x + size, y, z], [x, y + size, z]]
    }

    #[test]
    fn tag_applies_recovered_row_and_seam_and_keeps_physics_bits() {
        let r = Surface { key: 0, row: 3, seam: 11, families: [None, None] };
        let t = tag(1, Some(&r));
        assert_eq!(t & 0x7f, 4, "row 3 is audio id 4");
        assert_eq!((t >> 12) & 15, 11);
        assert_eq!(crate::world::packed_surface(t), 0, "physics surface untouched");
        let keep = Surface { key: 0, row: KEEP, seam: KEEP, families: [None, None] };
        assert_eq!(tag(1, Some(&keep)), crate::materials::surface(1).packed());
        assert_eq!(tag(1, None), crate::materials::surface(1).packed());
    }

    #[test]
    fn wheel_finds_the_grid_of_the_floor_it_stands_on() {
        let f = Family { normal: [1.0, 0.0], spacing: 2.0, phase: 0.5 };
        let g = GridIndex::build(
            [(floor(0.0, 0.0, 10.0, 8.0), [Some(f), None]), (floor(0.0, 0.0, 14.0, 8.0), [None, Some(f)])].into_iter(),
        );
        assert_eq!(g.at(Vec3::new(1.0, 1.0, 10.03)).unwrap()[0], Some(f));
        assert_eq!(g.at(Vec3::new(1.0, 1.0, 14.03)).unwrap()[1], Some(f), "upper deck, not the floor below");
        assert!(g.at(Vec3::new(1.0, 1.0, 12.0)).is_none(), "airborne between decks");
        assert!(g.at(Vec3::new(20.0, 1.0, 10.0)).is_none());
    }

    #[test]
    fn frame_gets_skate_space_lines_under_each_wheel() {
        let f = Family { normal: [0.0, 1.0], spacing: 1.0, phase: 0.25 };
        publish(GridIndex::build([(floor(-5.0, -5.0, 0.0, 20.0), [Some(f), None])].into_iter()));
        // Wheel 0 at GTA (1, 2, 0.03) = Skate (1, 0.03, -2); wheel 1 away.
        let mut frame = Frame {
            fields: vec![(384, Field::F32(1.0)), (388, Field::F32(0.03)), (392, Field::F32(-2.0)),
                         (400, Field::F32(500.0)), (404, Field::F32(0.0)), (408, Field::F32(0.0))],
            ..Default::default()
        };
        apply_to_frame(&mut frame);
        let s = frame.seam_grids[0].unwrap().families[0].unwrap();
        // GTA n.p = y = 2 must equal Skate a*x + b*z.
        assert!((s.a * 1.0 + s.b * -2.0 - 2.0).abs() < 1e-6);
        assert_eq!(frame.seam_grids[1], None);
        publish(GridIndex::default());
    }
}

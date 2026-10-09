//! Texture-space seam joints and cracks (`crackmaps.svgj` beside the cache, docs/MATERIAL-RECOVERY.md).
//!
//! The sidecar holds the visual ground GTA draws over walkable collision,
//! with each triangle's own texture coordinates and a joint map of its
//! texture measured from GTA's pixels. Area builds publish the ground around
//! the skater; every audio tick each wheel's path since the last tick is
//! walked in short world steps, each step mapped through the triangle under
//! it to its relief map, and entering a groove, raised line or pit is one
//! crossing: a `Frame::seam_events` entry with where in the tick it
//! happened, its depth and kind, played directly (skate-aems
//! `seam_voices`, docs/DECISIONS.md 2026-10-05). Sounds land on the relief
//! the player sees, along curves and across triangle and UV seams. With a
//! sidecar, `seam_hits` stay `Some(false)`: Class_Seams never clicks on its
//! own (no seam type's evenly spaced grid); without one the wheels keep
//! those grids.

use crate::grid::{weights, XyGrid};
use bevy_math::Vec3;
use skate_aems::live::frame::{Field, Frame, SeamEvent};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use svwc::ground::{Ground, GroundTri, JointMap};

/// The crack maps: one fixed name beside the cache, whatever the cache is called.
pub const FILE_NAME: &str = "crackmaps.svgj";

pub fn sidecar_path(cache: &Path) -> PathBuf {
    cache.with_file_name(FILE_NAME)
}

/// An open ground sidecar (absent or unreadable: no texture joints).
pub struct GroundSource {
    ground: Option<Ground<std::io::BufReader<std::fs::File>>>,
    maps: Arc<Vec<JointMap>>,
    status: String,
}

impl GroundSource {
    /// Never fails: a missing or bad sidecar leaves the authored grids.
    pub fn open(cache: &Path) -> Self {
        let path = sidecar_path(cache);
        if !path.exists() {
            return Self::none(format!("no ground joints ({} absent)", path.display()));
        }
        match Ground::open(&path) {
            Ok(mut g) => {
                let status = format!("ground relief {} ({} triangles, {} relief maps)", path.display(), g.triangle_count, g.maps.len());
                // One resident copy (~100 MB on the whole map): the index shares it.
                let maps = Arc::new(std::mem::take(&mut g.maps));
                Self { ground: Some(g), maps, status }
            }
            Err(e) => Self::none(format!("ground joints ignored: {}: {e}", path.display())),
        }
    }

    fn none(status: String) -> Self {
        Self { ground: None, maps: Arc::new(Vec::new()), status }
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    /// The ground index around `center` (one tile of margin, since a
    /// triangle lives in its centroid's tile).
    pub fn around(&mut self, center: [f32; 2], radius: f32) -> Result<Option<GroundIndex>, String> {
        let Some(g) = &mut self.ground else { return Ok(None) };
        let margin = g.tile_size;
        let tris = g.query(center, radius + margin).map_err(|e| format!("ground sidecar read: {e}"))?;
        Ok(Some(GroundIndex::build(tris, Arc::clone(&self.maps))))
    }
}

const CELL: f32 = 2.0;
/// A wheel stands on ground from 0.1 m below its point to `DZ` above.
const DZ: f32 = 0.5;
/// Path step (metres): under half a joint's width on the finest maps.
const STEP: f32 = 0.005;
/// Longest path walked per tick; beyond it (a teleport) the wheel restarts.
const MAX_PATH: f32 = 1.5;
/// Joint entries closer than this along a path are one joint (chips on a
/// joint's edge).
const DEBOUNCE: f32 = 0.05;
/// Game frame the wheel paths span (seconds).
// ponytail: fixed 60 Hz; pass the frame's dt if the game tick ever varies.
const FRAME_DT: f32 = 1.0 / 60.0;

/// The published ground, bucketed by `CELL` XY cells.
#[derive(Default)]
pub struct GroundIndex {
    tris: Vec<GroundTri>,
    maps: Arc<Vec<JointMap>>,
    cells: XyGrid,
}

impl GroundIndex {
    pub fn build(tris: Vec<GroundTri>, maps: Arc<Vec<JointMap>>) -> Self {
        let mut cells = XyGrid::new(CELL);
        for (id, t) in tris.iter().enumerate() {
            let lo = |a: usize| t.p.iter().map(|p| p[a]).fold(f32::MAX, f32::min);
            let hi = |a: usize| t.p.iter().map(|p| p[a]).fold(f32::MIN, f32::max);
            if hi(0) - lo(0) > 500.0 || hi(1) - lo(1) > 500.0 {
                continue;
            }
            cells.insert(id as u32, Vec3::new(lo(0), lo(1), 0.0), Vec3::new(hi(0), hi(1), 0.0));
        }
        Self { tris, maps, cells }
    }

    pub fn len(&self) -> usize {
        self.tris.len()
    }

    /// The roughness rank (1..=255, 0 unknown) of triangle `id`'s texture.
    pub fn roughness(&self, id: u32) -> u8 {
        self.tris.get(id as usize).map_or(0, |t| self.maps[t.map as usize].roughness)
    }

    fn on(&self, id: u32, p: Vec3) -> Option<(f32, u8)> {
        let t = &self.tris[id as usize];
        let l = weights(&t.p, p.x, p.y, 1e-12, 1e-5)?;
        let z = l[0] * t.p[0][2] + l[1] * t.p[1][2] + l[2] * t.p[2][2];
        let dz = p.z - z;
        if !(-0.1..=DZ).contains(&dz) {
            return None;
        }
        let u = l[0] * t.uv[0][0] + l[1] * t.uv[1][0] + l[2] * t.uv[2][0];
        let v = l[0] * t.uv[0][1] + l[1] * t.uv[1][1] + l[2] * t.uv[2][1];
        // A decal overlay holds the point only where its alpha shows.
        let map = &self.maps[t.map as usize];
        map.visible_at(u, v).then(|| (dz, map.value(u, v)))
    }

    /// The ground under a GTA point: (triangle, relief byte, 0 = flat). The triangle the
    /// wheel was on is kept while it still holds the point, so coplanar
    /// overlapping meshes never swap mid-slab; otherwise the highest.
    pub fn at(&self, p: Vec3, prefer: Option<u32>) -> Option<(u32, u8)> {
        if let Some(id) = prefer.filter(|id| (*id as usize) < self.tris.len()) {
            if let Some((_, j)) = self.on(id, p) {
                return Some((id, j));
            }
        }
        let mut best: Option<(f32, u32, u8)> = None;
        for &id in self.cells.at(p.x, p.y) {
            if let Some((dz, j)) = self.on(id, p) {
                if best.is_none_or(|b| dz < b.0) {
                    best = Some((dz, id, j));
                }
            }
        }
        best.map(|(_, id, j)| (id, j))
    }
}

/// One wheel's trace state between ticks.
#[derive(Clone, Copy, Default)]
struct Wheel {
    last: Option<(Vec3, u32, u8)>,
    /// Path length since the last relief exit.
    since: f32,
    /// Inside relief: (path length inside, deepest byte seen).
    inside: Option<(f32, u8)>,
}

/// A wheel leaving relief it rolled across: where along this tick's path
/// (0..=1) it hit the far side, the deepest relief byte, and the path
/// length inside (metres) -- what a wheel can drop into.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Crossing {
    at: f32,
    value: u8,
    width: f32,
}

#[derive(Default)]
struct Tracker {
    index: Option<Arc<GroundIndex>>,
    wheels: [Wheel; 4],
}

impl Tracker {
    /// Walks wheel `w` to `p`: the relief it rolled across (`None` only
    /// without a published ground). A crossing completes where the wheel
    /// leaves the relief; one that is still open waits for a later tick.
    fn step(&mut self, w: usize, p: Vec3) -> Option<Vec<Crossing>> {
        let index = Arc::clone(self.index.as_ref()?);
        let wheel = &mut self.wheels[w];
        let Some((from, tri, value)) = wheel.last.filter(|(f, ..)| f.distance(p) <= MAX_PATH) else {
            wheel.last = index.at(p, None).map(|(t, v)| (p, t, v));
            wheel.since = f32::MAX;
            wheel.inside = None;
            return Some(Vec::new());
        };
        let length = from.distance(p);
        let steps = ((length / STEP).ceil() as usize).max(1);
        let ds = length / steps as f32;
        let (mut tri, mut value) = (Some(tri), value);
        let mut crossed = Vec::new();
        for k in 1..=steps {
            let f = k as f32 / steps as f32;
            wheel.since += ds;
            match index.at(from.lerp(p, f), tri) {
                Some((t, v)) => {
                    if v != 0 && value == 0 && wheel.since >= DEBOUNCE {
                        wheel.inside = Some((0.0, v));
                    }
                    if let Some((width, deepest)) = wheel.inside {
                        if v != 0 {
                            let d = if v & 63 > deepest & 63 { v } else { deepest };
                            wheel.inside = Some((width + ds, d));
                        } else {
                            crossed.push(Crossing { at: f, value: deepest, width });
                            wheel.inside = None;
                            wheel.since = 0.0;
                        }
                    }
                    tri = Some(t);
                    value = v;
                }
                // Off the mapped ground: the next ground starts fresh.
                None => {
                    tri = None;
                    value = u8::MAX;
                    wheel.inside = None;
                }
            }
        }
        wheel.last = tri.map(|t| (p, t, value));
        if wheel.last.is_none() {
            wheel.since = f32::MAX;
        }
        Some(crossed)
    }
}

fn published() -> &'static RwLock<Option<Arc<GroundIndex>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<GroundIndex>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

fn tracker() -> &'static Mutex<Tracker> {
    static SLOT: OnceLock<Mutex<Tracker>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Tracker::default()))
}

/// Makes `index` the ground the audio tick reads (the latest built area);
/// `None` when there is no ground sidecar.
pub fn publish(index: Option<GroundIndex>) {
    if let Ok(mut g) = published().write() {
        *g = index.map(Arc::new);
    }
}

/// Skate surface rows (`materials.rs`) of loose ground and vegetation.
const LOOSE_ROWS: [u32; 2] = [7, 9];

/// Fills `frame.seam_events` by walking each wheel (skater `+384 + 16 * w`,
/// Skate space) over the published ground since the last tick.
pub fn apply_to_frame(frame: &mut Frame) {
    let Ok(index) = published().read().map(|g| g.clone()) else { return };
    let Ok(mut t) = tracker().lock() else { return };
    let surface = |w: usize| {
        frame.fields.iter().rev().find_map(|&(o, f)| match f {
            Field::F32(v) if o == 620 + 4 * w => Some(v as u32),
            Field::U32(v) if o == 620 + 4 * w => Some(v),
            _ => None,
        })
    };
    let loose = (0..4).filter(|&w| surface(w).is_some_and(|s| LOOSE_ROWS.contains(&s))).count();
    frame.loose_ground = loose as f32 / 4.0;
    if !t.index.as_ref().zip(index.as_ref()).is_some_and(|(a, b)| Arc::ptr_eq(a, b)) {
        // A new area: triangle ids changed, every wheel restarts.
        t.index = index;
        t.wheels = [Wheel::default(); 4];
    }
    if t.index.is_none() {
        frame.seam_hits = [None; 4];
        return;
    }
    let field = |off: usize| {
        frame.fields.iter().rev().find_map(|&(o, f)| match f {
            Field::F32(v) if o == off => Some(v),
            Field::U32(v) if o == off => Some(v as f32),
            _ => None,
        })
    };
    let speed = field(208).unwrap_or(0.0).abs();
    let mut rough = (0.0, 0);
    for w in 0..4 {
        let base = 384 + 16 * w;
        let crossed = match (field(base), field(base + 4), field(base + 8)) {
            (Some(x), Some(y), Some(z)) => t.step(w, crate::coords::from_skate(Vec3::new(x, y, z))),
            _ => None,
        };
        frame.seam_hits[w] = crossed.as_ref().map(|_| false);
        let surface = field(620 + 4 * w).unwrap_or(0.0) as u32;
        // Loose ground and vegetation keep Skate's own sound: a terrain blend
        // can show grass or sand over a hard layer's relief and grain.
        if LOOSE_ROWS.contains(&surface) {
            continue;
        }
        if let Some((_, tri, _)) = t.wheels[w].last {
            let r = t.index.as_ref().map_or(0, |i| i.roughness(tri));
            if r > 0 {
                rough = (rough.0 + r as f32 / 255.0, rough.1 + 1);
            }
        }
        for c in crossed.into_iter().flatten() {
            frame.seam_events.push(SeamEvent {
                wheel: w as u8,
                at: c.at * FRAME_DT,
                level: (c.value & 63) as f32 / 63.0,
                kind: c.value >> 6,
                width: c.width,
                speed,
                surface,
            });
        }
    }
    frame.roughness = (rough.1 > 0).then(|| rough.0 / rough.1 as f32);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4 m square of ground at z = 0, mapped one UV unit per metre along x
    /// (two triangles with different vertex order), joints on texture
    /// column 0 of a 100-texel map: lines every metre at x = 0, 1, 2, 3.
    fn slab() -> GroundIndex {
        let map = JointMap::from_mask(100, 4, |x, _| x == 0);
        let tri = |p: [[f32; 2]; 3]| GroundTri {
            p: p.map(|q| [q[0], q[1], 0.0]),
            uv: p.map(|q| [q[0], q[1] * 0.25]),
            map: 0,
            seam: 11,
            layer: 0,
        };
        GroundIndex::build(vec![tri([[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]]), tri([[0.0, 0.0], [4.0, 4.0], [0.0, 4.0]])], Arc::new(vec![map]))
    }

    fn tracker_on(index: GroundIndex) -> Tracker {
        Tracker { index: Some(Arc::new(index)), wheels: Default::default() }
    }

    #[test]
    fn an_overlay_holds_a_point_only_where_its_alpha_shows() {
        // Base: a 2 m square with no relief. Overlay 2 cm above it, a crack on
        // every texel, visible only on the left half (x < 1).
        let mut over = JointMap::from_mask(8, 8, |_, _| true);
        over.visible = Some(JointMap::pack_mask(8, 8, |x, _| x < 4));
        let tri = |z: f32, map: u16, layer: u8| GroundTri {
            p: [[0.0, 0.0, z], [2.0, 0.0, z], [0.0, 2.0, z]],
            uv: [[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]],
            map,
            seam: 11,
            layer,
        };
        let plain = JointMap::from_mask(8, 8, |_, _| false);
        let g = GroundIndex::build(vec![tri(0.0, 0, 0), tri(0.02, 1, 1)], Arc::new(vec![plain, over]));
        let p = |x: f32| Vec3::new(x, 0.3, 0.1);
        assert_eq!(g.at(p(0.25), None).map(|(id, j)| (id, j != 0)), Some((1, true)), "under the visible half the overlay wins");
        assert_eq!(g.at(p(0.75), None).map(|(id, _)| id), Some(0), "where its alpha is clear the base holds the point");
    }

    #[test]
    fn one_crossing_per_visible_joint_across_triangle_edges() {
        let mut t = tracker_on(slab());
        let mut hits = 0;
        let hit = |c: Option<Vec<Crossing>>| c.is_some_and(|c| !c.is_empty());
        // Diagonal run from (0.5, 0.2) to (3.5, 3.6) in 0.3 m ticks: crosses
        // the triangle edge and the joints at x = 1, 2, 3 once each.
        assert!(!hit(t.step(0, Vec3::new(0.5, 0.2, 0.05))));
        for k in 1..=10 {
            let f = k as f32 / 10.0;
            if hit(t.step(0, Vec3::new(0.5 + 3.0 * f, 0.2 + 3.4 * f, 0.05))) {
                hits += 1;
            }
        }
        assert_eq!(hits, 3);
    }

    #[test]
    fn fast_ticks_and_standing_on_a_joint() {
        let mut t = tracker_on(slab());
        t.step(1, Vec3::new(1.0, 1.0, 0.0));
        // Starting on the joint is no crossing; a long tick still finds the
        // joint it passes; airborne wheels fall back; landing never clicks.
        assert_eq!(t.step(1, Vec3::new(1.4, 1.0, 0.0)), Some(vec![]));
        let c = t.step(1, Vec3::new(2.6, 1.0, 0.0)).unwrap();
        assert_eq!(c.len(), 1, "x = 2 in one 1.2 m tick");
        assert!((c[0].at - 0.61 / 1.2).abs() < 0.01, "at its far side along the path: {c:?}");
        assert_eq!(c[0].value, svwc::ground::GROOVE << 6 | 63);
        assert!((c[0].width - 0.01).abs() < 0.006, "one 1 cm texel wide: {c:?}");
        assert_eq!(t.step(1, Vec3::new(2.6, 1.0, 3.0)), Some(vec![]), "airborne");
        assert_eq!(t.step(1, Vec3::new(3.5, 1.0, 0.0)), Some(vec![]), "landing restarts, no click");
    }

    #[test]
    fn ground_without_measured_joints_never_clicks() {
        let mut t = tracker_on(slab());
        assert_eq!(t.step(2, Vec3::new(4.5, 1.0, 0.0)), Some(vec![]), "just off the mapped ground");
        assert_eq!(t.step(3, Vec3::new(40.0, 40.0, 0.0)), Some(vec![]), "far from it: no authored grid either");
        assert_eq!(Tracker::default().step(0, Vec3::ZERO), None, "no sidecar: authored grids");
    }
}

/// Owned: area query cost on the baked sidecar (dense downtown, airport,
/// Vespucci). `SKATEV_CACHE=<cache.svwc> cargo test --release owned_ground -- --ignored --nocapture`
#[cfg(test)]
mod owned_ground {
    use super::*;

    #[test]
    #[ignore]
    fn owned_ground_area_query_is_cheap() {
        let cache = std::env::var("SKATEV_CACHE").expect("SKATEV_CACHE");
        let mut g = GroundSource::open(Path::new(&cache));
        println!("{}", g.status());
        for c in [[-250.0, -850.0], [-1340.0, -2700.0], [-1250.0, -1250.0]] {
            let t = std::time::Instant::now();
            let index = g.around(c, 128.0).unwrap().expect("sidecar");
            println!("{c:?}: {} triangles in {:?}", index.len(), t.elapsed());
            assert!(t.elapsed().as_secs_f32() < 1.0);
        }
        // The owner's spots: airport apron, boardwalk, brick circle candidates.
        for c in [[-1340.0, -2700.0], [-1250.0, -1250.0], [-1335.0, -1215.0], [-1245.0, -1140.0]] {
            let index = g.around(c, 6.0).unwrap().unwrap();
            let mut seams = std::collections::BTreeMap::new();
            for t in &index.tris {
                let cx = (t.p[0][0] + t.p[1][0] + t.p[2][0]) / 3.0 - c[0];
                let cy = (t.p[0][1] + t.p[1][1] + t.p[2][1]) / 3.0 - c[1];
                if cx.abs() < 6.0 && cy.abs() < 6.0 {
                    *seams.entry((t.seam, t.map)).or_insert(0) += 1;
                }
            }
            println!("{c:?} seam/map within 6 m: {seams:?}");
        }
    }
}

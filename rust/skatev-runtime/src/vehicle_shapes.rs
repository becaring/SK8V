//! Vehicle shapes from probes (contributed by GTAxSkate). SK8V's extracted
//! model bounds miss many vehicles (most DLC cars), which then stand in as
//! their bounding box: no roof to ride, no gap to hippie jump under. Such a
//! model is surveyed once by the host with ray probes straight down its own
//! up axis over a grid of its footprint (`sv_vehicle_survey`): the heights
//! make a roof, hood and trunk to ride on, walled down to the wheels, which
//! dynamic.rs then uses instead of the box.

use bevy_math::{Vec2, Vec3};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// Spacing of the survey grid, metres.
pub const SPACING: f32 = 0.3;
/// The gap left under every vehicle, metres: a board (with its wheels about
/// 0.1 m high) rolls on underneath while its rider hippie jumps the car.
pub const UNDER: f32 = 0.2;
/// Probes per model, at most (a long truck is surveyed more coarsely).
const MAX_PROBES: usize = 500;

/// A model's body in its own space (x right, y forward, z up): its top
/// heights on a grid over the bounding box's footprint.
#[derive(Clone, Debug)]
pub struct Shape {
    pub min: Vec3,
    pub max: Vec3,
    nx: usize,
    ny: usize,
    step: Vec2,
    heights: Vec<Option<f32>>,
}

impl Shape {
    /// True for [Shape::boxed]'s flat box (no probe met the body).
    pub fn is_box(&self) -> bool {
        self.nx == 2 && self.ny == 2 && self.heights.iter().all(|h| *h == Some(self.max.z))
    }

    /// The bounding box as the body: a flat top at its height.
    pub fn boxed(min: Vec3, max: Vec3) -> Self {
        let size = (max - min).truncate().max(Vec2::splat(0.1));
        Self { min, max, nx: 2, ny: 2, step: size, heights: vec![Some(max.z); 4] }
    }

    fn at(&self, i: usize, j: usize) -> Option<f32> {
        self.heights.get(j * self.nx + i).copied().flatten()
    }

    fn point(&self, i: usize, j: usize, z: f32) -> Vec3 {
        Vec3::new(self.min.x + i as f32 * self.step.x, self.min.y + j as f32 * self.step.y, z)
    }

    /// The top's height above `p` (own space), if the body is there.
    pub fn top_at(&self, p: Vec2) -> Option<f32> {
        let u = (p.x - self.min.x) / self.step.x;
        let v = (p.y - self.min.y) / self.step.y;
        if u < -0.25 || v < -0.25 || u > (self.nx - 1) as f32 + 0.25 || v > (self.ny - 1) as f32 + 0.25 {
            return None;
        }
        let i = (u.round().max(0.) as usize).min(self.nx - 1);
        let j = (v.round().max(0.) as usize).min(self.ny - 1);
        self.at(i, j)
    }

    /// Closes holes in the top: probes through the windows or the open top
    /// of a convertible, or onto the driver, meet no body, and a walker
    /// climbing over the door's edge stood inside the car. A missed cell
    /// with body on both sides of it (along its row or its column) takes the
    /// higher of the two nearest heights, so nothing steps down into it.
    pub fn fill_holes(&mut self) {
        let before = self.heights.clone();
        let at = |i: usize, j: usize| before[j * self.nx + i];
        for j in 0..self.ny {
            for i in 0..self.nx {
                if at(i, j).is_some() {
                    continue;
                }
                let left = (0..i).rev().find_map(|k| at(k, j));
                let right = (i + 1..self.nx).find_map(|k| at(k, j));
                let back = (0..j).rev().find_map(|k| at(i, k));
                let front = (j + 1..self.ny).find_map(|k| at(i, k));
                let across = left.zip(right).map(|(a, b)| a.max(b));
                let along = back.zip(front).map(|(a, b)| a.max(b));
                if let Some(fill) = match (across, along) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                } {
                    self.heights[j * self.nx + i] = Some(fill);
                }
            }
        }
    }

    /// Collision triangles (counterclockwise seen from outside) in own space:
    /// the top, and walls down to [UNDER] above the bottom of the box.
    pub fn triangles(&self) -> Vec<[Vec3; 3]> {
        let mut out = Vec::new();
        let present = |i: isize, j: isize| -> bool {
            i >= 0
                && j >= 0
                && (i as usize) + 1 < self.nx
                && (j as usize) + 1 < self.ny
                && [(0, 0), (1, 0), (0, 1), (1, 1)].iter().all(|(a, b)| self.at(i as usize + a, j as usize + b).is_some())
        };
        let bottom = self.min.z + UNDER;
        for j in 0..self.ny.saturating_sub(1) {
            for i in 0..self.nx.saturating_sub(1) {
                if !present(i as isize, j as isize) {
                    continue;
                }
                let h = |a: usize, b: usize| self.at(i + a, j + b).unwrap_or(self.max.z);
                let (p00, p10, p11, p01) =
                    (self.point(i, j, h(0, 0)), self.point(i + 1, j, h(1, 0)), self.point(i + 1, j + 1, h(1, 1)), self.point(i, j + 1, h(0, 1)));
                out.push([p00, p10, p11]);
                out.push([p00, p11, p01]);
                // Walls where the body ends, facing out.
                let (ii, jj) = (i as isize, j as isize);
                for (neighbour, a, b, facing) in [
                    ((ii, jj - 1), p00, p10, Vec2::NEG_Y),
                    ((ii + 1, jj), p10, p11, Vec2::X),
                    ((ii, jj + 1), p11, p01, Vec2::Y),
                    ((ii - 1, jj), p01, p00, Vec2::NEG_X),
                ] {
                    if present(neighbour.0, neighbour.1) {
                        continue;
                    }
                    let (a0, b0) = (Vec3::new(a.x, a.y, bottom), Vec3::new(b.x, b.y, bottom));
                    let quad = [a0, b0, b, a];
                    let normal = (quad[1] - quad[0]).cross(quad[2] - quad[0]);
                    if normal.truncate().dot(facing) >= 0. {
                        out.push([quad[0], quad[1], quad[2]]);
                        out.push([quad[0], quad[2], quad[3]]);
                    } else {
                        out.push([quad[0], quad[2], quad[1]]);
                        out.push([quad[0], quad[3], quad[2]]);
                    }
                }
            }
        }
        out
    }

    /// Grind rails along the top's edges (own space): down each side over
    /// hood, roof and trunk, and across the front and the back, split where
    /// the top steps more than a hand's height.
    pub fn rails(&self) -> Vec<Vec<Vec3>> {
        const SPLIT: f32 = 0.15;
        const MIN_LENGTH: f32 = 0.8;
        let mut rails = Vec::new();
        let mut finish = |rail: &mut Vec<Vec3>| {
            let length: f32 = rail.windows(2).map(|w| w[0].distance(w[1])).sum();
            if length >= MIN_LENGTH {
                rails.push(std::mem::take(rail));
            }
            rail.clear();
        };
        // Each edge: the outermost top point of every row (or column).
        let edges: [(usize, usize, bool, bool); 4] = [
            (self.ny, self.nx, false, false), // left side, along the length
            (self.ny, self.nx, false, true),  // right side
            (self.nx, self.ny, true, false),  // back, across
            (self.nx, self.ny, true, true),   // front
        ];
        for (rows, cells, across, far) in edges {
            let mut rail = Vec::new();
            for row in 0..rows {
                let mut order: Vec<usize> = (0..cells).collect();
                if far {
                    order.reverse();
                }
                let edge = order.into_iter().find_map(|cell| {
                    let (i, j) = if across { (row, cell) } else { (cell, row) };
                    self.at(i, j).map(|h| self.point(i, j, h))
                });
                match edge {
                    Some(p) if rail.last().is_none_or(|q: &Vec3| (q.z - p.z).abs() <= SPLIT) => rail.push(p),
                    Some(p) => {
                        finish(&mut rail);
                        rail.push(p);
                    }
                    None => finish(&mut rail),
                }
            }
            finish(&mut rail);
        }
        rails
    }
}

/// A model's survey, a few probes a frame.
#[derive(Clone, Debug)]
pub struct Survey {
    shape: Shape,
    next: usize,
    hits: usize,
}

impl Survey {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        let size = (max - min).truncate().max(Vec2::splat(0.2));
        let mut spacing = SPACING;
        while ((size.x / spacing).ceil() as usize + 1) * ((size.y / spacing).ceil() as usize + 1) > MAX_PROBES {
            spacing *= 1.25;
        }
        let nx = (size.x / spacing).ceil() as usize + 1;
        let ny = (size.y / spacing).ceil() as usize + 1;
        let step = Vec2::new(size.x / (nx - 1) as f32, size.y / (ny - 1) as f32);
        Self { shape: Shape { min, max, nx, ny, step, heights: vec![None; nx * ny] }, next: 0, hits: 0 }
    }

    pub fn done(&self) -> bool {
        self.next >= self.shape.heights.len()
    }

    /// The next probe: from above the box to its bottom, in own space.
    pub fn next_probe(&self) -> Option<(Vec3, Vec3)> {
        (!self.done()).then(|| {
            let (i, j) = (self.next % self.shape.nx, self.next / self.shape.nx);
            // Just inside the edges, so the outermost probes meet the body.
            let p = self.shape.point(i, j, 0.);
            let inset = |v: f32, lo: f32, hi: f32| v.clamp(lo + 0.03, hi - 0.03);
            let (x, y) = (inset(p.x, self.shape.min.x, self.shape.max.x), inset(p.y, self.shape.min.y, self.shape.max.y));
            (Vec3::new(x, y, self.shape.max.z + 0.5), Vec3::new(x, y, self.shape.min.z))
        })
    }

    /// The last probe's hit height (own space), or none.
    pub fn record(&mut self, height: Option<f32>) {
        if let Some(slot) = self.shape.heights.get_mut(self.next) {
            // Something a hand's width above the bottom: the wheels' lower
            // edges are not a roof.
            *slot = height.filter(|z| *z > self.shape.min.z + 0.15).map(|z| z.min(self.shape.max.z));
            if slot.is_some() {
                self.hits += 1;
            }
        }
        self.next += 1;
    }

    /// The surveyed body, or the box when the probes met too little of it.
    pub fn finish(mut self) -> Shape {
        if self.hits * 4 < self.shape.heights.len() {
            Shape::boxed(self.shape.min, self.shape.max)
        } else {
            self.shape.fill_holes();
            self.shape
        }
    }
}


/// Where SK8V's extracted model bounds are (dynamic.rs `Templates::new`).
pub static TEMPLATE_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Surveyed models' triangles (model space) and the models already asked about.
static SURVEYED: Mutex<Option<HashMap<u32, Arc<Vec<svwc::Tri>>>>> = Mutex::new(None);
static ASKED: Mutex<Option<HashMap<u32, u8>>> = Mutex::new(None);
/// Surveys per model before it stays SK8V's box (one that met too little of
/// the body, as when its collision was not loaded yet, is tried again).
const TRIES: u8 = 8;

/// The surveyed body of `model`, if there is one.
pub fn surveyed(model: u32) -> Option<Arc<Vec<svwc::Tri>>> {
    SURVEYED.lock().ok()?.as_ref()?.get(&model).cloned()
}

/// True for a model that has no extracted bound and no survey yet (up to
/// [TRIES] times while surveys meet too little of it).
pub fn needs_shape(model: u32) -> bool {
    let Some(dir) = TEMPLATE_DIR.get() else { return false };
    let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
    let tries = asked.get_or_insert_with(HashMap::new).entry(model).or_insert(0);
    if *tries >= TRIES || surveyed(model).is_some() {
        return false;
    }
    *tries += 1;
    if dir.join(format!("{model:08x}.svwc")).is_file() {
        *tries = TRIES;
        return false;
    }
    true
}

/// Surveys `model` (its box `min`..`max`, model space) with `probe` (from,
/// to -> where it hits, model space); keeps and returns its triangles.
pub fn survey(model: u32, min: Vec3, max: Vec3, mut probe: impl FnMut(Vec3, Vec3) -> Option<Vec3>) -> usize {
    let mut survey = Survey::new(min, max);
    while let Some((top, bottom)) = survey.next_probe() {
        survey.record(probe(top, bottom).map(|p| p.z));
    }
    let shape = survey.finish();
    if shape.is_box() {
        // Too little met: SK8V's own box stands, and the model is tried again.
        return shape.triangles().len();
    }
    let tris: Vec<svwc::Tri> = shape
        .triangles()
        .into_iter()
        .map(|t| svwc::Tri { v: t.map(|p| p.to_array()), material: 0 })
        .collect();
    let n = tris.len();
    SURVEYED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(model, Arc::new(tris));
    n
}

/// 1 once per vehicle model that has no extracted bound (the host then
/// surveys it with `sv_vehicle_survey`), else 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_vehicle_needs_shape(_rt: *mut c_void, model: u32) -> u32 {
    crate::guard(0, || u32::from(needs_shape(model)))
}

/// A line the host casts against one vehicle, in its model space: from, to
/// (3 floats each); writes where it hits the vehicle and returns 1, else 0.
pub type SvModelProbe = unsafe extern "C" fn(ctx: *mut c_void, from: *const f32, to: *const f32, hit: *mut f32) -> u32;

/// Surveys `model` (bounding box `min`/`max`, 3 floats each, model space)
/// with `probe`; returns the triangles its body got (0: none).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_vehicle_survey(_rt: *mut c_void, model: u32, min: *const f32, max: *const f32,
                                           probe: Option<SvModelProbe>, ctx: *mut c_void) -> u32 {
    crate::guard(0, || {
        let Some(probe) = probe else { return 0 };
        if min.is_null() || max.is_null() {
            return 0;
        }
        let v3 = |p: *const f32| unsafe { Vec3::new(*p, *p.add(1), *p.add(2)) };
        let (lo, hi) = (v3(min), v3(max));
        if !lo.is_finite() || !hi.is_finite() || (hi - lo).min_element() <= 0.05 || (hi - lo).max_element() > 40.0 {
            return 0;
        }
        survey(model, lo, hi, |a, b| {
            let (from, to, mut hit) = (a.to_array(), b.to_array(), [0.0f32; 3]);
            (unsafe { probe(ctx, from.as_ptr(), to.as_ptr(), hit.as_mut_ptr()) } != 0).then(|| Vec3::from(hit))
        }) as u32
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_car_with_a_roof_gets_a_top_and_a_gap_under_it() {
        // A 2 x 4 m car: body 0.5..1.0 m, roof 1.0..1.5 m over its middle.
        let (min, max) = (Vec3::new(-1.0, -2.0, 0.0), Vec3::new(1.0, 2.0, 1.5));
        let n = survey(7, min, max, |a, _| Some(Vec3::new(a.x, a.y, if a.y.abs() < 1.0 { 1.5 } else { 1.0 })));
        assert!(n > 20, "{n}");
        let tris = surveyed(7).unwrap();
        let top = tris.iter().flat_map(|t| t.v).map(|p| p[2]).fold(f32::MIN, f32::max);
        let bottom = tris.iter().flat_map(|t| t.v).map(|p| p[2]).fold(f32::MAX, f32::min);
        assert!((top - 1.5).abs() < 1e-4, "roof {top}");
        assert!((bottom - UNDER).abs() < 1e-4, "walls end {UNDER} above the ground: {bottom}");
    }

    #[test]
    fn a_survey_that_meets_nothing_is_the_box() {
        let n = survey(8, Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 1.0), |_, _| None);
        assert_eq!(n, 2 + 4 * 2, "the box's top and four walls");
    }

    #[test]
    fn a_model_that_met_nothing_is_tried_again_and_then_left_as_the_box() {
        let dir = std::env::temp_dir().join("skatev-no-bounds");
        let _ = TEMPLATE_DIR.set(dir);
        let model = 0x5eed_0001;
        let mut asked = 0;
        while needs_shape(model) {
            asked += 1;
            survey(model, Vec3::new(-1.0, -2.0, 0.0), Vec3::new(1.0, 2.0, 1.5), |_, _| None);
        }
        assert_eq!(asked, TRIES as usize);
        assert!(surveyed(model).is_none(), "SK8V's box stands");
    }
}

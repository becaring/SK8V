//! Ramp toes, and the classification of every other small drop.
//!
//! GTA geometry is kept as built except at a ramp's toe. Three cases, from
//! the edge of a walkable top with ground 1.5..16 cm lower just outside:
//!
//! - **Ramp toe** (the top descends toward the edge faster than the ground:
//!   a garage kicker: 8 m/s -> 3 m/s at a 10 cm plywood
//!   lip). The ramp's own plane continues flush down to the ground with a
//!   tangent fillet, so the only kink is the ramp's own angle; nothing is
//!   steeper than the ramp.
//! - **Sloped surface detail** (the ground keeps falling: a pothole dish, a
//!   rounded kerb). GTA's potholes are shallow dishes (owner cache: median
//!   2.3 cm deep, 7.7 degree walls, `examples/surface_detail.rs`), so the
//!   collision profile is kept and the rider dips and rises over it.
//! - **Real riser** onto level ground (curb, ledge, step). Kept: Skate's
//!   finite wheels (retail radius 3.1 cm) meet it, an ollie clears it. An
//!   earlier 22 degree bevel and a smoothed variant both turned a 7.5 cm curb
//!   into a kicker at 8.75 m/s (`evidence/2026-10-02/surface-detail.md`).
use crate::grid::{height, XyGrid};
use bevy_math::Vec3;
use std::collections::HashMap;

/// Smallest and largest step that gets a bevel (metres).
pub const MIN_STEP: f32 = 0.015;
pub const MAX_STEP: f32 = 0.16;
/// A lower ground falling more than this fraction of the step again within
/// `GRADE_SPAN` is a slope (a pothole dish, a rounded kerb), not a riser.
const SLOPE_FRACTION: f32 = 0.5;
/// A top face this level or flatter can be a curb top.
const TOP_NORMAL_Z: f32 = 0.9;
/// How far outside the edge the lower ground is sampled.
const SAMPLE_OUT: f32 = 0.04;
/// Longest bevel piece; longer edges are split so the ground can vary.
const PIECE: f32 = 1.0;
/// A top descending at least this grade toward its edge (rise per run, about
/// 5.7 degrees) is a ramp toe and is extended in its own plane.
const MIN_TOE_GRADE: f32 = 0.1;
/// Horizontal span over which the lower ground's grade is sampled.
const GRADE_SPAN: f32 = 0.2;

/// Horizontal half-length of the fillet between a toe's plane and the
/// ground. A hard kink turns a 31 m/s board (0.52 m per tick, longer than
/// the 0.37 m garage toe) into the ramp plane in one tick, sinking it
/// 0.52 * sin(15.8 deg) = 14 cm before any contact; spread over `FILLET_SEGMENTS`
/// pieces each tick sees a couple of degrees (owner BackwardsMan runs).
const FILLET: f32 = 0.5;
const FILLET_SEGMENTS: usize = 8;

/// Toe profile from the edge `p` outward: the top's plane, then a smooth
/// (quadratic, tangent at both ends) fillet into the lower ground. `rise` is
/// the edge height above the ground just outside it.
fn toe_profile(p: Vec3, out_xy: bevy_math::Vec2, outward_grade: f32, ground_grade: f32, toe_grade: f32, rise: f32) -> Vec<Vec3> {
    let at = |w: f32, z: f32| {
        let q = p.truncate() + out_xy * w;
        Vec3::new(q.x, q.y, z)
    };
    let plane = |w: f32| p.z + outward_grade * w;
    let ground = |w: f32| p.z - rise + ground_grade * (w - SAMPLE_OUT);
    // Where the plane meets the ground.
    let kink = (rise - ground_grade * SAMPLE_OUT) / toe_grade;
    let (w2, w1) = (kink - FILLET, kink + FILLET);
    let k = at(kink, plane(kink));
    let (t2, t1) = (at(w2, plane(w2)), at(w1, ground(w1)));
    let mut out = Vec::with_capacity(FILLET_SEGMENTS + 2);
    if w2 > 0.0 {
        out.push(at(0.0, p.z));
    }
    for i in 0..=FILLET_SEGMENTS {
        let t = i as f32 / FILLET_SEGMENTS as f32;
        let q = t2 * (1.0 - t) * (1.0 - t) + k * (2.0 * t * (1.0 - t)) + t1 * (t * t);
        out.push(q);
    }
    // The fillet meets the ground tangentially and flush; the imported seam
    // filter treats that coplanar joint as internal.
    out
}

/// Cell of the area grid curbs and plies share (`XyGrid::triangles`).
pub const CELL: f32 = 2.0;

/// Highest upward surface under (x, y) between `below` and `above`.
fn ground(grid: &XyGrid, tris: &[[Vec3; 3]], x: f32, y: f32, below: f32, above: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for &i in grid.at(x, y) {
        let t = &tris[i as usize];
        let [a, b, c] = *t;
        if (b - a).cross(c - a).z <= 0.0 {
            continue;
        }
        let Some(z) = height(t, x, y) else { continue };
        if z >= below && z <= above && best.is_none_or(|b| z > b) {
            best = Some(z);
        }
    }
    best
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BevelCensus {
    pub edges: usize,
    pub bevels: usize,
    /// Riser/underside triangles buried under a bevel and its top.
    pub buried: usize,
    /// Edges over a sloped drop (GTA surface detail kept as built).
    pub slopes: usize,
    /// Real risers onto level ground (curbs, ledges, steps) kept as built.
    pub curbs: usize,
}

/// The solid a bevel piece and the top behind it cover: a vertical slab
/// under the bevel surface (outward) and under the top (inward, `BEHIND`).
struct Cover {
    p0: Vec3,
    p1: Vec3,
    out_xy: bevy_math::Vec2,
    run: f32,
    undercut: f32,
    foot_z: [f32; 2],
    outward_grade: f32,
}

/// How far behind the edge (under its top) a buried face may reach.
const BEHIND: f32 = 0.15;

impl Cover {
    fn covers(&self, v: Vec3) -> bool {
        let along = (self.p1 - self.p0).truncate();
        let len2 = along.length_squared();
        if len2 < 1e-8 {
            return false;
        }
        let u = (v.truncate() - self.p0.truncate()).dot(along) / len2;
        let tol = 0.01 / len2.sqrt();
        if !(-tol..=1.0 + tol).contains(&u) {
            return false;
        }
        let u = u.clamp(0.0, 1.0);
        let edge = self.p0.lerp(self.p1, u);
        let foot = self.foot_z[0] + (self.foot_z[1] - self.foot_z[0]) * u;
        let w = (v.truncate() - edge.truncate()).dot(self.out_xy);
        if w < -BEHIND || w > self.run + 0.01 {
            return false;
        }
        let surface = if w >= 0.0 {
            let top = edge.z - self.undercut;
            top + (foot - top) * (w / self.run).min(1.0)
        } else {
            edge.z + self.outward_grade * w
        };
        v.z <= surface + 0.01 && v.z >= foot - 0.03
    }
}

pub struct Bevels {
    pub triangles: Vec<[Vec3; 3]>,
    /// Source top triangle for each bevel triangle (keeps contact material).
    pub sources: Vec<usize>,
    /// Input triangles buried under a bevel and its top: risers and
    /// undersides a swept board would otherwise hit through a short bevel at
    /// speed (owner BackwardsMan at 31 m/s into a garage kicker).
    pub buried: Vec<usize>,
    pub census: BevelCensus,
}

/// Bevels (GTA space, metres, Z up, counterclockwise from above), their
/// source tops, and the triangles they bury. `grid`: `tris` in `CELL` cells.
pub fn bevels_with_sources(tris: &[[Vec3; 3]], grid: &XyGrid) -> Bevels {
    // Diagnostic A/B switch (harness and in-game): SKATEV_CURB_BEVELS=0.
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *OFF.get_or_init(|| std::env::var("SKATEV_CURB_BEVELS").is_ok_and(|v| v == "0")) {
        return Bevels { triangles: Vec::new(), sources: Vec::new(), buried: Vec::new(), census: BevelCensus::default() };
    }
    let key = |v: Vec3| v.to_array().map(|x| (x * 200.0).round() as i32);
    // Edges of flat tops, once (an edge shared by two tops is interior).
    let mut edges: HashMap<([i32; 3], [i32; 3]), (Vec3, Vec3, Vec3, u32, usize)> = HashMap::new();
    for (source, t) in tris.iter().enumerate() {
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        if n.z < TOP_NORMAL_Z {
            continue;
        }
        let centroid = (t[0] + t[1] + t[2]) / 3.0;
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            let (ka, kb) = (key(a), key(b));
            let k = if ka < kb { (ka, kb) } else { (kb, ka) };
            edges.entry(k).or_insert((a, b, centroid, 0, source)).3 += 1;
        }
    }
    let mut census = BevelCensus::default();
    let mut out = Vec::new();
    let mut sources = Vec::new();
    let mut covers: Vec<Cover> = Vec::new();
    for (a, b, centroid, uses, source) in edges.into_values() {
        if uses != 1 {
            continue;
        }
        let along = b - a;
        let len = along.truncate().length();
        if len < 0.05 || (b.z - a.z).abs() > 0.3 * len {
            continue;
        }
        census.edges += 1;
        let dir = along.truncate() / len;
        let mut out_xy = bevy_math::Vec2::new(dir.y, -dir.x);
        let mid = (a + b) * 0.5;
        if out_xy.dot((centroid - mid).truncate()) > 0.0 {
            out_xy = -out_xy;
        }
        let pieces = (len / PIECE).ceil().max(1.0) as usize;
        for k in 0..pieces {
            let p0 = a + along * (k as f32 / pieces as f32);
            let p1 = a + along * ((k + 1) as f32 / pieces as f32);
            // Lower ground just outside both ends of the piece.
            let below = |p: Vec3| {
                let q = p.truncate() + out_xy * SAMPLE_OUT;
                ground(grid, tris, q.x, q.y, p.z - MAX_STEP - 0.01, p.z - MIN_STEP)
                    .filter(|z| p.z - z <= MAX_STEP)
            };
            let (Some(z0), Some(z1)) = (below(p0), below(p1)) else { continue };
            // Nothing higher may sit just outside (a wall, not a curb).
            let blocked = |p: Vec3| {
                let q = p.truncate() + out_xy * SAMPLE_OUT;
                ground(grid, tris, q.x, q.y, p.z - MIN_STEP + 1e-3, p.z + 0.5).is_some()
            };
            if blocked(p0) || blocked(p1) {
                continue;
            }
            let rise = ((p0.z - z0) + (p1.z - z1)) * 0.5;
            // Grade of the top going outward from this edge (negative: it
            // descends toward the edge, a ramp toe).
            let t = tris[source];
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            let outward_grade = -(n.x * out_xy.x + n.y * out_xy.y) / n.z;
            // Grade of the lower ground going outward (a ramp ply stacked on
            // another ramp descends with it: parallel planes never meet).
            let ground_grade = {
                let mid = (p0 + p1) * 0.5;
                let q0 = mid.truncate() + out_xy * SAMPLE_OUT;
                let q1 = mid.truncate() + out_xy * (SAMPLE_OUT + GRADE_SPAN);
                let z0 = ground(grid, tris, q0.x, q0.y, mid.z - MAX_STEP - 0.01, mid.z - MIN_STEP);
                let z1 = ground(grid, tris, q1.x, q1.y, mid.z - MAX_STEP - 0.3, mid.z - MIN_STEP);
                match (z0, z1) {
                    (Some(a), Some(b)) => (b - a) / GRADE_SPAN,
                    _ => 0.0,
                }
            };
            let toe_grade = ground_grade - outward_grade;
            let toe = toe_grade >= MIN_TOE_GRADE;
            if !toe {
                // Only a ramp's toe is extended. A drop that keeps falling is
                // GTA's sloped surface detail (pothole dish, rounded kerb) and
                // keeps its collision profile; a riser onto level ground is a
                // real curb, ledge or step and stays a real obstacle (Retail:
                // finite wheels meet it; an ollie clears it).
                if -ground_grade * GRADE_SPAN > SLOPE_FRACTION * rise {
                    census.slopes += 1;
                } else {
                    census.curbs += 1;
                }
                continue;
            }
            // The toe ends 1 cm below the ground, like every bevel foot.
            let run = (rise + 0.01) / toe_grade;
            let undercut = 0.0;
            // In the top's own plane: coplanar with the ramp.
            let foot = |p: Vec3| {
                let q = p.truncate() + out_xy * run;
                Vec3::new(q.x, q.y, p.z + outward_grade * run)
            };
            let (f0, f1) = (foot(p0), foot(p1));
            covers.push(Cover { p0, p1, out_xy, run, undercut, foot_z: [f0.z, f1.z], outward_grade });
            // Profile (edge -> ground) at each end of the piece: a plain bevel
            // is one quad; a toe is its plane down to a fillet into the ground.
            let profile = |p: Vec3, f: Vec3| -> Vec<Vec3> {
                toe_profile(p, out_xy, outward_grade, ground_grade, toe_grade, p.z - f.z - 0.01)
            };
            let (s0, s1) = (profile(p0, f0), profile(p1, f1));
            for k in 0..s0.len().min(s1.len()).saturating_sub(1) {
                // Counterclockwise seen from above.
                let mut quad = [s0[k + 1], s1[k + 1], s1[k], s0[k]];
                if (quad[1] - quad[0]).cross(quad[2] - quad[0]).z < 0.0 {
                    quad.reverse();
                }
                for t in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                    // Same degenerate rule as the GTA triangles (world::build_area).
                    if (t[1] - t[0]).cross(t[2] - t[0]).length_squared() > 1e-10 && t.iter().all(|v| v.is_finite()) {
                        out.push(t);
                        sources.push(source);
                    }
                }
            }
            census.bevels += 1;
        }
    }
    // Non-walkable triangles wholly inside the covered solids.
    let mut near = XyGrid::new(CELL);
    for (i, c) in covers.iter().enumerate() {
        let lo = c.p0.min(c.p1).truncate() - bevy_math::Vec2::splat(c.run + BEHIND);
        let hi = c.p0.max(c.p1).truncate() + bevy_math::Vec2::splat(c.run + BEHIND);
        near.insert(i as u32, lo.extend(0.0), hi.extend(0.0));
    }
    let mut buried = Vec::new();
    for (i, t) in tris.iter().enumerate() {
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        if n.z >= TOP_NORMAL_Z {
            continue;
        }
        let covered = |v: Vec3| {
            near.at(v.x, v.y).iter().any(|&k| covers[k as usize].covers(v))
        };
        // Vertices alone are not enough for long faces: sample each edge
        // every 25 cm and the centroid (a side face running up a ramp has
        // its ends under different bevels but its middle exposed).
        let edges_covered = (0..3).all(|e| {
            let (a, b) = (t[e], t[(e + 1) % 3]);
            let steps = ((b - a).length() / 0.25).ceil().max(1.0) as usize;
            (0..=steps).all(|k| covered(a.lerp(b, k as f32 / steps as f32)))
        });
        if edges_covered && covered((t[0] + t[1] + t[2]) / 3.0) {
            buried.push(i);
        }
    }
    census.buried = buried.len();
    Bevels { triangles: out, sources, buried, census }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bevels(tris: &[[Vec3; 3]]) -> Bevels {
        bevels_with_sources(tris, &XyGrid::triangles(CELL, tris))
    }

    fn quad(z: f32, x0: f32, x1: f32) -> [[Vec3; 3]; 2] {
        let (a, b, c, d) = (
            Vec3::new(x0, -2.0, z),
            Vec3::new(x1, -2.0, z),
            Vec3::new(x1, 2.0, z),
            Vec3::new(x0, 2.0, z),
        );
        [[a, b, c], [a, c, d]]
    }

    #[test]
    fn ramp_toe_blends_smoothly_into_the_floor() {
        // Floor at z 0; a 15 degree ramp whose toe edge (x = 0) stands 10 cm
        // above the floor and rises toward +x.
        let grade = 15f32.to_radians().tan();
        let mut tris: Vec<[Vec3; 3]> = Vec::new();
        tris.extend(quad(0.0, -3.0, 0.0));
        let (a, b, c, d) = (
            Vec3::new(0.0, -2.0, 0.1),
            Vec3::new(1.0, -2.0, 0.1 + grade),
            Vec3::new(1.0, 2.0, 0.1 + grade),
            Vec3::new(0.0, 2.0, 0.1),
        );
        tris.extend([[a, b, c], [a, c, d]]);
        let Bevels { triangles: bevel, census, .. } = bevels(&tris);
        assert!(census.bevels >= 1, "{census:?}");
        // Profile at y = 0 from the triangles' vertices on the toe side.
        let ramp = |x: f32| 0.1 + grade * x;
        let mut profile: Vec<(f32, f32)> = bevel
            .iter()
            .flatten()
            .filter(|p| p.y.abs() < 1e-3 || (p.y - 1.0).abs() < 1e-3)
            .map(|p| (p.x, p.z))
            .collect();
        profile.sort_by(|p, q| q.0.total_cmp(&p.0));
        profile.dedup_by(|p, q| (p.0 - q.0).abs() < 1e-4);
        let (first, last) = (profile[0], profile[profile.len() - 1]);
        assert!((first.1 - ramp(first.0)).abs() < 1e-3, "starts on the ramp plane: {first:?}");
        assert!(last.1.abs() < 1e-3, "ends on the floor: {last:?}");
        for &(x, z) in &profile {
            assert!(z >= ramp(x).min(0.0) - 0.011 && z <= ramp(x).max(0.0) + 0.05, "no dip or bump: {x} {z}");
        }
        // Each segment turns by only a few degrees (no hard kink).
        let slopes: Vec<f32> = profile.windows(2).map(|w| ((w[0].1 - w[1].1) / (w[0].0 - w[1].0)).atan().to_degrees()).collect();
        for s in slopes.windows(2) {
            assert!((s[0] - s[1]).abs() < 4.0, "segment kink {s:?}");
        }
        assert!((slopes[0] - 15.0).abs() < 1.0 && slopes[slopes.len() - 1].abs() < 3.0, "tangent at both ends: {slopes:?}");
    }

    #[test]
    fn toe_buries_the_riser_but_not_a_wall() {
        // Road z 0 (x < 0); a 15 degree ramp from x = 0 whose toe lip stands
        // 10 cm (riser at x = 0), and a 1 m wall standing on it at x = 1.
        let g = 15f32.to_radians().tan();
        let mut tris: Vec<[Vec3; 3]> = Vec::new();
        tris.extend(quad(0.0, -3.0, 0.0));
        let (a, b, c, d) = (
            Vec3::new(0.0, -2.0, 0.1),
            Vec3::new(3.0, -2.0, 0.1 + 3.0 * g),
            Vec3::new(3.0, 2.0, 0.1 + 3.0 * g),
            Vec3::new(0.0, 2.0, 0.1),
        );
        tris.push([a, b, c]);
        tris.push([a, c, d]);
        let riser = |x: f32, z0: f32, z1: f32| {
            let (a, b, c, d) = (
                Vec3::new(x, -2.0, z0),
                Vec3::new(x, 2.0, z0),
                Vec3::new(x, 2.0, z1),
                Vec3::new(x, -2.0, z1),
            );
            [[a, b, c], [a, c, d]]
        };
        let curb_riser = tris.len();
        tris.extend(riser(0.0, 0.0, 0.1));
        let wall = tris.len();
        tris.extend(riser(1.0, 0.1 + g, 1.1 + g));
        let b = bevels(&tris);
        assert!(b.census.bevels >= 1);
        assert!(b.buried.contains(&curb_riser) && b.buried.contains(&(curb_riser + 1)), "{:?}", b.buried);
        assert!(!b.buried.contains(&wall) && !b.buried.contains(&(wall + 1)), "a wall is never buried");
    }

    #[test]
    fn curbs_and_ledges_stay_real_risers() {
        // Road at z 0 for x < 0, sidewalk at z 0.075 for x >= 0: no geometry
        // is added in front of a real curb, nor a 40 cm ledge.
        for h in [0.075f32, 0.4] {
            let mut tris: Vec<[Vec3; 3]> = Vec::new();
            tris.extend(quad(0.0, -3.0, 0.0));
            tris.extend(quad(h, 0.0, 3.0));
            let Bevels { triangles: b, census, .. } = bevels(&tris);
            assert!(b.is_empty(), "{h}: {census:?}");
        }
        let mut curb: Vec<[Vec3; 3]> = Vec::new();
        curb.extend(quad(0.0, -3.0, 0.0));
        curb.extend(quad(0.075, 0.0, 3.0));
        assert!(bevels(&curb).census.curbs >= 1);
    }

    #[test]
    fn pothole_dish_keeps_its_profile() {
        // Road at z 0 around a 1 m dish whose 30 degree wall drops to -0.1.
        let g = 30f32.to_radians().tan();
        let w = 0.1 / g;
        let mut tris: Vec<[Vec3; 3]> = Vec::new();
        tris.extend(quad(0.0, -3.0, 0.0));
        let (a, b, c, d) = (Vec3::new(0.0, -2.0, 0.0), Vec3::new(w, -2.0, -0.1), Vec3::new(w, 2.0, -0.1), Vec3::new(0.0, 2.0, 0.0));
        tris.push([a, b, c]);
        tris.push([a, c, d]);
        tris.extend(quad(-0.1, w, 1.0));
        let Bevels { triangles: bev, census, .. } = bevels(&tris);
        assert!(bev.is_empty(), "{census:?}");
        assert!(census.slopes >= 1, "{census:?}");
    }
}

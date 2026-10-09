//! Triangle-soup clean-up shared by the offline bake (`skatev-world-cache`)
//! and the live collision reader (`skatev-runtime::live`): both must leave the
//! same collision behind, so the rules live here once.
use crate::Tri;
use std::collections::HashMap;

/// Soft vegetation GTA authors as volumes or foliage meshes: bush and grass
/// clumps (GRASS_LONG, GRASS, GRASS_SHORT, BUSHES, TWIGS, LEAVES; HAY bales
/// and tree bark stay solid). Peds push through them.
pub fn soft_vegetation(material: u8) -> bool {
    matches!(material, 46..=48 | 50..=52)
}

/// A BUSHES / TWIGS / LEAVES piece GTA's peds cannot pass, by its placed
/// bounds: a hedge (at least 1 m tall and 2.5 m long) or a tree (at least
/// 3 m tall). Smaller clumps are bushes and stay passable.
pub fn hedge(material: u8, lo: [f32; 3], hi: [f32; 3]) -> bool {
    let (h, long) = (hi[2] - lo[2], (hi[0] - lo[0]).max(hi[1] - lo[1]));
    matches!(material, 50..=52) && (h >= 3.0 || (h >= 1.0 && long >= 2.5))
}

/// Removes repeated triangles: every corner within 1 cm of another triangle's
/// (same winding, any rotation of the corners), as GTA places some trees and
/// props twice a few millimetres apart. Reversed copies (two-sided surfaces)
/// stay. Returns how many were removed.
pub fn drop_repeats(tris: &mut Vec<Tri>) -> usize {
    const TOL: f32 = 0.01;
    let centroid = |t: &Tri| -> [f32; 3] { std::array::from_fn(|k| (t.v[0][k] + t.v[1][k] + t.v[2][k]) / 3.0) };
    let same = |a: &Tri, b: &Tri| {
        (0..3).any(|r| (0..3).all(|i| (0..3).all(|k| (a.v[i][k] - b.v[(i + r) % 3][k]).abs() < TOL)))
    };
    let mut blocks: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        let c = centroid(t);
        if c.iter().all(|x| x.is_finite()) {
            blocks.entry(((c[0] / 64.0).floor() as i32, (c[1] / 64.0).floor() as i32)).or_default().push(i as u32);
        }
    }
    let mut keep = vec![true; tris.len()];
    for ids in blocks.values() {
        // Centroids within TOL of each other: the same 2*TOL cell, or the
        // neighbour on the side the centroid is nearer to (8 cells).
        let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        for &i in ids {
            let c = centroid(&tris[i as usize]).map(|x| x / (2.0 * TOL));
            let base = c.map(|x| x.floor() as i32);
            let side = std::array::from_fn::<i32, 3, _>(|k| if c[k] - c[k].floor() < 0.5 { -1 } else { 1 });
            let repeat = (0..8).any(|m| {
                let cell = std::array::from_fn(|k| base[k] + if m >> k & 1 == 1 { side[k] } else { 0 });
                cells.get(&cell).is_some_and(|v| v.iter().any(|&j| same(&tris[i as usize], &tris[j as usize])))
            });
            if repeat {
                keep[i as usize] = false;
            } else {
                cells.entry(base).or_default().push(i);
            }
        }
    }
    let before = tris.len();
    let mut k = 0;
    tris.retain(|_| {
        k += 1;
        keep[k - 1]
    });
    before - tris.len()
}

const CELL: f32 = 2.0;
/// Solid floor at most this far under a foliage triangle's centroid: bushes
/// and hedges (Rockford Hills hedges stand 3-4 m) reach higher than grass,
/// and grass a few metres over a floor is more likely terrain over a tunnel.
fn depth(material: u8) -> f32 {
    if material >= 50 { 5.0 } else { 2.5 }
}
/// Floors closer than this are the same surface (coplanar layers).
const EPS: f32 = 0.02;

fn up(t: &Tri) -> bool {
    let [a, b, c] = t.v;
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    len > 1e-9 && n[2] / len >= 0.5
}

fn cells(t: &Tri) -> impl Iterator<Item = (i32, i32)> {
    let lo = |k: usize| t.v.iter().map(|p| p[k]).fold(f32::MAX, f32::min);
    let hi = |k: usize| t.v.iter().map(|p| p[k]).fold(f32::MIN, f32::max);
    let (x0, x1) = ((lo(0) / CELL).floor() as i32, (hi(0) / CELL).floor() as i32);
    let (y0, y1) = ((lo(1) / CELL).floor() as i32, (hi(1) / CELL).floor() as i32);
    (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
}

/// Height of upward-facing triangle `t` at XY `p`, if `p` lies over it.
pub fn floor_at(t: &Tri, p: [f32; 2]) -> Option<f32> {
    if up(t) { height(t, p) } else { None }
}

fn height(t: &Tri, p: [f32; 2]) -> Option<f32> {
    let [a, b, c] = t.v;
    let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if d.abs() < 1e-12 {
        return None;
    }
    let l0 = ((b[1] - c[1]) * (p[0] - c[0]) + (c[0] - b[0]) * (p[1] - c[1])) / d;
    let l1 = ((c[1] - a[1]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[1] - c[1])) / d;
    let l2 = 1.0 - l0 - l1;
    (l0 >= -1e-4 && l1 >= -1e-4 && l2 >= -1e-4).then(|| l0 * a[2] + l1 * b[2] + l2 * c[2])
}

/// Squared distance from `p` to triangle `t` (Ericson, closest point).
fn dist2(p: [f32; 3], t: &Tri) -> f32 {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let along = |a: [f32; 3], d: [f32; 3], s: f32| [a[0] + d[0] * s, a[1] + d[1] * s, a[2] + d[2] * s];
    let [a, b, c] = t.v;
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    let (va, vb, vc) = (d3 * d6 - d5 * d4, d5 * d2 - d1 * d6, d1 * d4 - d3 * d2);
    let q = if d1 <= 0.0 && d2 <= 0.0 {
        a
    } else if d3 >= 0.0 && d4 <= d3 {
        b
    } else if d6 >= 0.0 && d5 <= d6 {
        c
    } else if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        along(a, ab, d1 / (d1 - d3))
    } else if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        along(a, ac, d2 / (d2 - d6))
    } else if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        along(b, sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6)))
    } else {
        let den = 1.0 / (va + vb + vc);
        along(along(a, ab, vb * den), ac, vc * den)
    };
    let d = sub(p, q);
    dot(d, d)
}

/// Where gaps are: no walkable triangle within this of a point.
const GAP: f32 = 1.0;

/// Fills the walkable collision's gaps from `extra` (GTA's bullet/camera copy,
/// `bounds::solid_to_sight`): an extra triangle is added when 3 of its 4
/// points (centroid and the half-way points to the corners) have no triangle
/// of `tris` within `GAP`. Where the ped collision already has a surface
/// (ramps over stairs, the coarse version of detailed ground) the copy stays
/// out; surfaces it lacks (owner 2026-10-07: a rooftop wall's top) come in.
/// Returns how many were added.
pub fn fill_gaps(tris: &mut Vec<Tri>, extra: Vec<Tri>) -> usize {
    let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        if !t.v.iter().flatten().all(|x| x.is_finite()) {
            continue;
        }
        let span = t.v.iter().map(|p| p[0]).fold(f32::MIN, f32::max) - t.v.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
        let span_y = t.v.iter().map(|p| p[1]).fold(f32::MIN, f32::max) - t.v.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        if span > 2000.0 || span_y > 2000.0 {
            continue;
        }
        for cell in cells(t) {
            grid.entry(cell).or_default().push(i as u32);
        }
    }
    let covered = |p: [f32; 3]| {
        let (x0, x1) = (((p[0] - GAP) / CELL).floor() as i32, ((p[0] + GAP) / CELL).floor() as i32);
        let (y0, y1) = (((p[1] - GAP) / CELL).floor() as i32, ((p[1] + GAP) / CELL).floor() as i32);
        (x0..=x1).flat_map(|x| (y0..=y1).map(move |y| (x, y)))
            .filter_map(|c| grid.get(&c)).flatten()
            .any(|&i| dist2(p, &tris[i as usize]) <= GAP * GAP)
    };
    let half = |a: [f32; 3], b: [f32; 3]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0];
    let gaps: Vec<Tri> = extra
        .into_iter()
        .filter(|t| t.v.iter().flatten().all(|x| x.is_finite()))
        .filter(|t| {
            let c = centroid(t);
            [c, half(c, t.v[0]), half(c, t.v[1]), half(c, t.v[2])].iter().filter(|p| !covered(**p)).count() >= 3
        })
        .collect();
    let n = gaps.len();
    tris.extend(gaps);
    n
}

fn centroid(t: &Tri) -> [f32; 3] {
    std::array::from_fn(|i| (t.v[0][i] + t.v[1][i] + t.v[2][i]) / 3.0)
}

/// Foliage meshes standing on ground (hedges and bush blobs GTA models as
/// triangle meshes with a vegetation material): removes vegetation triangles
/// with a solid upward floor under their centroid within `depth`. A
/// vegetation triangle with nothing solid under it *is* the ground (grassy
/// terrain, shrub-covered hillsides) and stays, as do hedges and trees
/// (`hedge`) and soil caps (`soil`), over each connected foliage mesh.
/// Returns how many were removed.
pub fn drop_over_ground(tris: &mut Vec<Tri>) -> usize {
    let solid = hedges(tris);
    // Only cells under foliage need floors.
    let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for t in tris.iter().filter(|t| soft_vegetation(t.material)) {
        let c = centroid(t);
        grid.entry(((c[0] / CELL).floor() as i32, (c[1] / CELL).floor() as i32)).or_default();
    }
    if grid.is_empty() {
        return 0;
    }
    for (i, t) in tris.iter().enumerate() {
        if soft_vegetation(t.material) || !up(t) || !t.v.iter().flatten().all(|x| x.is_finite()) {
            continue;
        }
        // Skip absurdly large triangles' cell sweep (sea floor, map bounds).
        let span = t.v.iter().map(|p| p[0]).fold(f32::MIN, f32::max) - t.v.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
        let span_y = t.v.iter().map(|p| p[1]).fold(f32::MIN, f32::max) - t.v.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        if span > 2000.0 || span_y > 2000.0 {
            continue;
        }
        for cell in cells(t) {
            if let Some(list) = grid.get_mut(&cell) {
                list.push(i as u32);
            }
        }
    }
    let over: Vec<bool> = tris
        .iter()
        .zip(&solid)
        .map(|(t, &solid)| {
            if !soft_vegetation(t.material) || solid {
                return false;
            }
            let c = centroid(t);
            grid[&((c[0] / CELL).floor() as i32, (c[1] / CELL).floor() as i32)]
                .iter()
                .any(|&i| height(&tris[i as usize], [c[0], c[1]]).is_some_and(|z| z < c[2] - EPS && z >= c[2] - depth(t.material)))
        })
        .collect();
    let before = tris.len();
    let mut k = 0;
    tris.retain(|_| {
        k += 1;
        !over[k - 1]
    });
    before - tris.len()
}

/// Per triangle: part of a hedge or tree mesh (`hedge` over the bounds of
/// its connected foliage triangles; corners joined by identical bits).
/// Grass materials: a grass mesh of upward faces only, 2 m across, is a soil
/// cap (a planter's or raised bed's dirt, owner 2026-10-07), not a clump.
fn soil(material: u8) -> bool {
    matches!(material, 46..=48)
}

fn hedges(tris: &[Tri]) -> Vec<bool> {
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut parent: Vec<usize> = (0..tris.len()).collect();
    let mut corners: HashMap<[u32; 3], usize> = HashMap::new();
    for (i, t) in tris.iter().enumerate().filter(|(_, t)| soft_vegetation(t.material)) {
        for v in t.v {
            let j = *corners.entry(v.map(f32::to_bits)).or_insert(i);
            let (a, b) = (root(&mut parent, i), root(&mut parent, j));
            parent[a] = b;
        }
    }
    // Per mesh: bounds, and whether every triangle faces up.
    let mut bounds: HashMap<usize, ([f32; 3], [f32; 3], bool)> = HashMap::new();
    for (i, t) in tris.iter().enumerate().filter(|(_, t)| soft_vegetation(t.material)) {
        let b = bounds.entry(root(&mut parent, i)).or_insert(([f32::MAX; 3], [f32::MIN; 3], true));
        for v in t.v {
            for k in 0..3 {
                b.0[k] = b.0[k].min(v[k]);
                b.1[k] = b.1[k].max(v[k]);
            }
        }
        b.2 &= up(t);
    }
    (0..tris.len())
        .map(|i| bounds.get(&root(&mut parent, i)).is_some_and(|&(lo, hi, cap)| {
            let m = tris[i].material;
            hedge(m, lo, hi) || (soil(m) && cap && hi[0] - lo[0] >= 2.0 && hi[1] - lo[1] >= 2.0)
        }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(z: f32, m: u8, x0: f32, x1: f32) -> [Tri; 2] {
        [
            Tri { v: [[x0, 0., z], [x1, 0., z], [x1, 4., z]], material: m },
            Tri { v: [[x0, 0., z], [x1, 4., z], [x0, 4., z]], material: m },
        ]
    }

    #[test]
    fn foliage_over_ground_goes_foliage_as_ground_stays() {
        let mut tris = Vec::new();
        tris.extend(quad(0.0, 4, 0.0, 4.0)); // tarmac
        tris.extend(quad(1.2, 50, 1.0, 3.0)); // bush top over it
        // A hedge side face standing on the tarmac.
        tris.push(Tri { v: [[1., 1., 0.], [3., 1., 0.], [3., 1., 1.2]], material: 50 });
        tris.extend(quad(0.0, 47, 10.0, 14.0)); // grass terrain, nothing under it
        tris.extend(quad(6.0, 50, 1.0, 3.0)); // canopy far above the ground (> depth)
        tris.extend(quad(3.5, 47, 1.0, 3.0)); // grass 3.5 m up: terrain over a tunnel
        assert_eq!(drop_over_ground(&mut tris), 3);
        assert_eq!(tris.iter().filter(|t| t.material == 4).count(), 2);
        assert_eq!(tris.iter().filter(|t| t.material == 47).count(), 4, "grass ground stays");
        assert_eq!(tris.iter().filter(|t| t.material == 50).count(), 2, "nothing within reach under it");
    }

    #[test]
    fn hedges_and_trees_stay_over_ground() {
        let mut tris = Vec::new();
        tris.extend(quad(0.0, 4, -10.0, 10.0)); // tarmac
        // A 4 m hedge face, 1.5 m tall: two triangles sharing an edge.
        let (a, b, c, d) = ([0., 1., 0.], [4., 1., 0.], [4., 1., 1.5], [0., 1., 1.5]);
        tris.push(Tri { v: [a, b, c], material: 50 });
        tris.push(Tri { v: [a, c, d], material: 50 });
        // A bush 1.2 m wide and tall: goes.
        tris.push(Tri { v: [[6., 1., 0.], [7.2, 1., 0.], [7.2, 1., 1.2]], material: 50 });
        assert_eq!(drop_over_ground(&mut tris), 1);
        assert_eq!(tris.iter().filter(|t| t.material == 50).count(), 2, "the hedge stays");
        assert!(hedge(52, [0.0; 3], [1.4, 1.4, 4.6]), "a tree");
        assert!(!hedge(47, [0.0; 3], [6.0, 1.0, 1.5]), "grass is never a hedge");
    }

    #[test]
    fn gaps_fill_from_the_bullet_copy_and_covered_ground_does_not() {
        let mut tris: Vec<Tri> = quad(0.0, 4, -10.0, 10.0).to_vec(); // roof
        // The bullet copy: the same roof 5 cm higher (covered, stays out) and
        // a wall top 1.5 m up the walkable collision lacks (comes in).
        let mut extra: Vec<Tri> = quad(0.05, 4, -10.0, 10.0).to_vec();
        extra.extend(quad(1.5, 4, 12.0, 13.0));
        assert_eq!(fill_gaps(&mut tris, extra), 2);
        assert!(tris[2..].iter().all(|t| t.v[0][2] == 1.5), "only the wall top");
        assert!((dist2([0.5, 0.5, 2.0], &tris[0]) - 4.0).abs() < 1e-4, "point over the face");
        assert!((dist2([-11.0, 0.0, 0.0], &tris[0]) - 1.0).abs() < 1e-4, "point beside an edge");
    }

    #[test]
    fn planter_soil_stays_grass_clumps_go() {
        let mut tris = Vec::new();
        tris.extend(quad(0.0, 4, -10.0, 10.0)); // street
        // A planter's dirt: a 3 m upward grass fan 0.8 m above the street.
        let c = [0.0, 2.0, 1.1];
        let ring: Vec<[f32; 3]> = (0..8).map(|k| { let a = k as f32 * std::f32::consts::FRAC_PI_4; [1.5 * a.cos(), 2.0 + 1.5 * a.sin(), 0.8] }).collect();
        for k in 0..8 {
            tris.push(Tri { v: [ring[k], ring[(k + 1) % 8], c], material: 47 });
        }
        // A grass clump 3 m across: an upward top and a side face.
        let (p, q, r) = ([5., 1., 0.5], [8., 1., 0.5], [8., 4., 0.5]);
        tris.push(Tri { v: [p, q, r], material: 47 });
        tris.push(Tri { v: [p, [8., 1., 0.0], q], material: 47 });
        assert_eq!(drop_over_ground(&mut tris), 2);
        assert_eq!(tris.iter().filter(|t| t.material == 47).count(), 8, "the dirt stays");
    }

    #[test]
    fn repeats_go_reversed_copies_stay() {
        let t = |v: [[f32; 3]; 3]| Tri { v, material: 1 };
        let (a, b, c) = ([0., 0., 0.], [1., 0., 0.], [0., 1., 0.]);
        let shift = |v: [[f32; 3]; 3], d: f32| t(v.map(|p| [p[0] + d, p[1] + d, p[2] + d]));
        let mut tris = vec![
            t([a, b, c]),
            t([b, c, [0.0002, 0., 0.]]),
            t([a, c, b]),
            t([a, b, [0., 2., 0.]]),
            shift([a, b, c], 0.004),
            shift([a, b, c], -0.006),
            shift([a, b, c], 0.015),
        ];
        assert_eq!(drop_repeats(&mut tris), 3, "rotated 0.2 mm, +4 mm and -6 mm copies go");
        assert_eq!(tris.len(), 4, "reversed winding, a different triangle and a 15 mm copy stay");
    }
}

//! Interior faces of stacked thin plies.
//!
//! GTA builds many props and kickers from overlapping boxes. The faces
//! between them (a lower sheet's top and front lip under a ramp's top sheet,
//! the overlap of two brush boxes) are unreachable from above, yet a board
//! sweeping 0.5 m per tick at BackwardsMan speed reaches them through the top
//! skin and stops dead (a garage kicker: 28 m/s -> wipeout on
//! a lip 5 cm under the visible ramp). A face is interior when every sampled
//! point of it has a walkable face directly above within `SKIN` metres and
//! clearly above it (`GAP`): no skateboard fits in that space. A real step's
//! riser touches its own top at the edge (gap 0), so it is never removed.
use crate::grid::{height, XyGrid};
use bevy_math::Vec3;
use std::collections::HashMap;

/// Deepest a face may sit under a walkable skin and still be interior.
const SKIN: f32 = 0.12;
/// Every sample must be at least this far below the skin (a riser's top
/// edge touches its own top and keeps the face).
const GAP: f32 = 0.01;
/// A face this level or flatter is walkable (matches curbs::TOP_NORMAL_Z).
const WALKABLE_Z: f32 = 0.9;
/// Edge sampling step.
const STEP: f32 = 0.25;

/// Indices of interior faces among `tris` (GTA space, Z up). `grid`: `tris`
/// in `curbs::CELL` cells.
pub fn interior(tris: &[[Vec3; 3]], grid: &XyGrid) -> Vec<usize> {
    let walkable: Vec<bool> = tris
        .iter()
        .map(|t| {
            let z = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero().z;
            z.is_nan() || z >= WALKABLE_Z
        })
        .collect();
    // Lowest walkable surface strictly above p within the skin depth.
    let skin_above = |p: Vec3, own: usize| -> bool {
        grid.at(p.x, p.y).iter().any(|&i| {
            let i = i as usize;
            if i == own || !walkable[i] {
                return false;
            }
            let Some(z) = height(&tris[i], p.x, p.y) else { return false };
            let gap = z - p.z;
            gap >= GAP && gap <= SKIN
        })
    };
    let mut out = Vec::new();
    for (i, t) in tris.iter().enumerate() {
        let lo = t[0].min(t[1]).min(t[2]);
        let hi = t[0].max(t[1]).max(t[2]);
        // A long road never lies in a ply interior; skip it cheaply. (Height
        // is no filter: a ply runs up a ramp.)
        if (hi - lo).truncate().length() > 8.0 {
            continue;
        }
        let centroid = (t[0] + t[1] + t[2]) / 3.0;
        if !skin_above(centroid, i) {
            continue;
        }
        let edges = (0..3).all(|e| {
            let (a, b) = (t[e], t[(e + 1) % 3]);
            let steps = ((b - a).length() / STEP).ceil().max(1.0) as usize;
            (0..=steps).all(|k| skin_above(a.lerp(b, k as f32 / steps as f32), i))
        });
        if edges {
            out.push(i);
        }
    }
    out
}

/// Largest edge of a face that can be an end cap between abutting boxes
/// (rail, bar or ledge cross-sections). Broad faces (walls, signs) never are.
const CAP_EDGE: f32 = 0.75;
/// Farthest apart (along either normal) two opposing caps may sit.
const CAP_GAP: f32 = 0.03;
/// Opposing normals: dot product at most this (about 135 degrees apart).
const CAP_FACING: f32 = -0.7;
/// Corners are pulled this far toward the centroid before the coverage test.
const CAP_OVERLAP: f32 = 0.02;
/// A cap's own box runs at least this far behind it (a bar, not a plate).
const CAP_DEPTH: f32 = 0.08;

/// Indices of end caps pressed back to back between abutting boxes.
///
/// GTA builds a kinked handrail or ledge from one box per straight run; at
/// each joint the two boxes' end caps face each other a centimetre or so
/// apart, inside the solid. A board meeting the joint at speed touches such
/// a cap and reads a contact facing back along the rail, with the whole
/// speed as closing velocity: Skate's grind post check then wants a runout
/// (a stair handrail bailed at the bottom of every
/// flight). A small face is interior when its own box (the faces sharing its
/// edges) runs more than `CAP_DEPTH` behind it and opposing faces of other
/// such boxes, each within `CAP_GAP` of its plane, together cover its corners
/// (pulled in by `CAP_OVERLAP`) and its centroid; the partners' quads may be
/// split along other diagonals than its own. The two faces of a thin plate
/// are joined by a rim only as deep as the plate, so plates and sheets keep
/// their faces.
pub fn abutting_caps(tris: &[[Vec3; 3]]) -> Vec<usize> {
    const CAP_CELL: f32 = 0.5;
    let key = |p: Vec3| ((p.x / CAP_CELL).floor() as i32, (p.y / CAP_CELL).floor() as i32, (p.z / CAP_CELL).floor() as i32);
    let mut small: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    let mut normals = vec![Vec3::ZERO; tris.len()];
    for (i, t) in tris.iter().enumerate() {
        let longest = (0..3).map(|e| (t[(e + 1) % 3] - t[e]).length()).fold(0.0, f32::max);
        if longest > CAP_EDGE {
            continue;
        }
        normals[i] = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        if normals[i] == Vec3::ZERO {
            continue;
        }
        small.entry(key((t[0] + t[1] + t[2]) / 3.0)).or_default().push(i as u32);
    }
    // Point `c` projected onto `b`'s plane lies inside `b`.
    let inside = |c: Vec3, b: usize| {
        let n = normals[b];
        let [p0, p1, p2] = tris[b];
        let q = c - n * (c - p0).dot(n);
        (0..3).all(|e| {
            let (u, v) = ([p0, p1, p2][e], [p0, p1, p2][(e + 1) % 3]);
            let inward = n.cross(v - u).normalize_or_zero();
            (q - u).dot(inward) >= -CAP_OVERLAP
        })
    };
    let near_plane = |a: usize, b: usize| {
        tris[a].iter().all(|&p| (p - tris[b][0]).dot(normals[b]).abs() <= CAP_GAP)
    };
    // Edge adjacency over positions welded at 1 mm.
    let weld = |p: Vec3| [(p.x * 1000.0).round() as i64, (p.y * 1000.0).round() as i64, (p.z * 1000.0).round() as i64];
    let mut edges: HashMap<([i64; 3], [i64; 3]), Vec<u32>> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        for e in 0..3 {
            let (a, b) = (weld(t[e]), weld(t[(e + 1) % 3]));
            edges.entry(if a < b { (a, b) } else { (b, a) }).or_default().push(i as u32);
        }
    }
    // How far the faces sharing an edge with `i` reach behind it.
    let depth = |i: usize| {
        let mut deepest = 0.0f32;
        for e in 0..3 {
            let (a, b) = (weld(tris[i][e]), weld(tris[i][(e + 1) % 3]));
            for &j in &edges[&if a < b { (a, b) } else { (b, a) }] {
                if j as usize != i {
                    for &v in &tris[j as usize] {
                        deepest = deepest.max(-(v - tris[i][0]).dot(normals[i]));
                    }
                }
            }
        }
        deepest
    };
    let mut found = vec![false; tris.len()];
    for (&(x, y, z), list) in &small {
        for &i in list {
            let i = i as usize;
            if depth(i) <= CAP_DEPTH {
                continue;
            }
            let mut partners = Vec::new();
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(others) = small.get(&(x + dx, y + dy, z + dz)) else { continue };
                        partners.extend(others.iter().map(|&j| j as usize).filter(|&j| {
                            j != i && normals[i].dot(normals[j]) <= CAP_FACING
                                && near_plane(i, j) && near_plane(j, i) && depth(j) > CAP_DEPTH
                        }));
                    }
                }
            }
            if partners.is_empty() {
                continue;
            }
            // Covered by the partners together (two boxes' caps may split
            // along different diagonals): the corners, pulled in by the
            // tolerance, and the centroid.
            let t = tris[i];
            let centroid = (t[0] + t[1] + t[2]) / 3.0;
            let samples = t.map(|p| p + (centroid - p).normalize_or_zero() * CAP_OVERLAP);
            found[i] = samples.iter().chain([&centroid]).all(|&p| partners.iter().any(|&j| inside(p, j)));
        }
    }
    (0..tris.len()).filter(|&i| found[i]).collect()
}

/// Largest closed solid (triangle count) another face can be buried in: bars,
/// panels and boxes, never building shells or rooms.
const SOLID_TRIS: usize = 64;
/// Widest such solid (bounding box diagonal, metres).
const SOLID_SPAN: f32 = 12.0;
/// The part of a face a board can touch: within this of its top.
const REACH: f32 = 0.3;
/// Share of a face's samples that must lie inside the other solid.
const BURIED_SHARE: f32 = 0.85;
/// A sample this close outside the other solid still counts as inside (GTA
/// builds the two panels of a handrail kink a centimetre apart in width).
const BURIED_TOL: f32 = 0.01;
/// The face's centroid must lie at least this deep inside, so a plate lying
/// flush on a box and the box's top under it both stay.
const BURIED_DEPTH: f32 = 0.005;

/// Indices of faces buried inside another small convex solid.
///
/// GTA also builds kinked handrails by running one box into the next: a
/// stair flight's sloped panel runs on 0.4 m into the landing's panel, so the
/// landing panel's end face sits inside the flight's panel with its top edge
/// on the grind line (the exchange staircase's south
/// handrail bailed every grind at the bottom of the first flight; the
/// contact faced back up the rail). A face of a closed solid is buried when
/// its centroid lies `BURIED_DEPTH` inside another closed, convex,
/// outward-facing solid of at most `SOLID_TRIS` triangles, and every sample
/// within `REACH` of its top plus `BURIED_SHARE` of all its samples lie
/// inside it within `BURIED_TOL`.
pub fn buried_faces(tris: &[[Vec3; 3]]) -> Vec<usize> {
    // Solids: components over welded edges where every edge has two faces.
    let weld = |p: Vec3| [(p.x * 1000.0).round() as i64, (p.y * 1000.0).round() as i64, (p.z * 1000.0).round() as i64];
    // (welded edge, face), sorted: the faces of an edge sit together in order.
    type Edge = ([i64; 3], [i64; 3]);
    let mut edges: Vec<(Edge, u32)> = Vec::with_capacity(tris.len() * 3);
    for (i, t) in tris.iter().enumerate() {
        for e in 0..3 {
            let (a, b) = (weld(t[e]), weld(t[(e + 1) % 3]));
            edges.push((if a < b { (a, b) } else { (b, a) }, i as u32));
        }
    }
    edges.sort_unstable();
    let mut root: Vec<u32> = (0..tris.len() as u32).collect();
    fn find(root: &mut [u32], mut i: u32) -> u32 {
        while root[i as usize] != i {
            root[i as usize] = root[root[i as usize] as usize];
            i = root[i as usize];
        }
        i
    }
    let mut open = vec![false; tris.len()];
    for faces in edges.chunk_by(|x, y| x.0 == y.0) {
        if faces.len() != 2 {
            faces.iter().for_each(|&(_, f)| open[f as usize] = true);
        }
        for w in faces.windows(2) {
            let (a, b) = (find(&mut root, w[0].1), find(&mut root, w[1].1));
            root[a as usize] = b;
        }
    }
    let solid_of: Vec<u32> = (0..tris.len() as u32).map(|i| find(&mut root, i)).collect();
    let mut members: HashMap<u32, Vec<u32>> = HashMap::new();
    for (i, &r) in solid_of.iter().enumerate() {
        members.entry(r).or_default().push(i as u32);
    }
    // Each solid as its face planes (unit normal, offset), with its bounds.
    let mut solids: Vec<(Vec<(Vec3, f32)>, Vec3, Vec3)> = Vec::new();
    let mut solid_index: HashMap<u32, usize> = HashMap::new();
    for (&r, list) in &members {
        if list.len() < 4 || list.len() > SOLID_TRIS || list.iter().any(|&f| open[f as usize]) {
            continue;
        }
        let points: Vec<Vec3> = list.iter().flat_map(|&f| tris[f as usize]).collect();
        let lo = points.iter().copied().fold(Vec3::splat(f32::MAX), Vec3::min);
        let hi = points.iter().copied().fold(Vec3::splat(f32::MIN), Vec3::max);
        let planes: Vec<(Vec3, f32)> = list
            .iter()
            .map(|&f| {
                let [a, b, c] = tris[f as usize];
                let n = (b - a).cross(c - a).normalize_or_zero();
                (n, n.dot(a))
            })
            .collect();
        // Convex with outward normals: every vertex on or behind every plane,
        // and some vertex well behind each (a room's shell faces inward).
        let convex = planes.iter().all(|&(n, d)| {
            n != Vec3::ZERO && points.iter().all(|&p| n.dot(p) - d <= 1e-3) && points.iter().any(|&p| n.dot(p) - d < -0.02)
        });
        if !convex || (hi - lo).length() > SOLID_SPAN {
            continue;
        }
        solid_index.insert(r, solids.len());
        solids.push((planes, lo, hi));
    }
    let mut grid = XyGrid::new(1.0);
    for (s, (_, lo, hi)) in solids.iter().enumerate() {
        grid.insert(s as u32, *lo, *hi);
    }
    // Signed distance outside solid `s` (negative inside).
    let outside = |p: Vec3, s: usize| solids[s].0.iter().map(|&(n, d)| n.dot(p) - d).fold(f32::MIN, f32::max);
    let mut out = Vec::new();
    for (i, t) in tris.iter().enumerate() {
        let Some(&own) = solid_index.get(&solid_of[i]) else { continue };
        let centroid = (t[0] + t[1] + t[2]) / 3.0;
        let others: Vec<usize> = grid.at(centroid.x, centroid.y).iter().map(|&s| s as usize)
            .filter(|&s| s != own && outside(centroid, s) <= -BURIED_DEPTH).collect();
        if others.is_empty() {
            continue;
        }
        // Barycentric samples, pulled in from the corners by CAP_OVERLAP.
        let top = t[0].z.max(t[1].z).max(t[2].z);
        let corners = t.map(|p| p + (centroid - p).normalize_or_zero() * CAP_OVERLAP);
        const N: usize = 6;
        let samples: Vec<Vec3> = (0..=N)
            .flat_map(|a| (0..=N - a).map(move |b| (a, b)))
            .map(|(a, b)| {
                let (u, v) = (a as f32 / N as f32, b as f32 / N as f32);
                corners[0] + (corners[1] - corners[0]) * u + (corners[2] - corners[0]) * v
            })
            .collect();
        let buried = others.iter().any(|&s| {
            let flags: Vec<bool> = samples.iter().map(|&p| outside(p, s) <= BURIED_TOL).collect();
            let reach = samples.iter().zip(&flags).all(|(p, &f)| f || p.z < top - REACH);
            reach && flags.iter().filter(|&&f| f).count() as f32 >= BURIED_SHARE * flags.len() as f32
        });
        if buried {
            out.push(i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(z: f32, x0: f32, x1: f32) -> [[Vec3; 3]; 2] {
        let (a, b, c, d) = (
            Vec3::new(x0, -1.0, z),
            Vec3::new(x1, -1.0, z),
            Vec3::new(x1, 1.0, z),
            Vec3::new(x0, 1.0, z),
        );
        [[a, b, c], [a, c, d]]
    }
    fn wall(x: f32, z0: f32, z1: f32) -> [[Vec3; 3]; 2] {
        let (a, b, c, d) = (
            Vec3::new(x, -1.0, z0),
            Vec3::new(x, 1.0, z0),
            Vec3::new(x, 1.0, z1),
            Vec3::new(x, -1.0, z1),
        );
        [[a, b, c], [a, c, d]]
    }

    #[test]
    fn sheet_under_a_top_sheet_is_interior_but_a_curb_is_not() {
        let mut tris: Vec<[Vec3; 3]> = Vec::new();
        tris.extend(quad(0.0, -3.0, 3.0)); // floor 0..1
        tris.extend(quad(0.10, -1.0, 2.0)); // top sheet 2..3
        let inner = tris.len();
        tris.extend(quad(0.05, 0.0, 1.5)); // inner sheet 5 cm under it 4..5
        tris.extend(wall(0.0, 0.0, 0.05)); // inner sheet lip 6..7
        let curb = tris.len();
        tris.extend(wall(-1.0, 0.0, 0.10)); // the top sheet's own front 8..9
        let found = interior(&tris, &XyGrid::triangles(crate::curbs::CELL, &tris));
        for i in [inner, inner + 1, inner + 2, inner + 3] {
            assert!(found.contains(&i), "inner face {i} interior: {found:?}");
        }
        assert!(!found.contains(&curb) && !found.contains(&(curb + 1)), "a riser touching its top stays");
        assert!(!found.contains(&0) && !found.contains(&2), "floor and top stay");
    }

    /// Closed box between `a` and `b` (centre line of its top), `w` wide in
    /// y and `t` thick below the line, with both end caps.
    fn bar(a: Vec3, b: Vec3, w: f32, t: f32) -> Vec<[Vec3; 3]> {
        let p = |q: Vec3, y: f32, dz: f32| Vec3::new(q.x, y, q.z + dz);
        let (y0, y1) = (-w / 2.0, w / 2.0);
        let quads = [
            [p(a, y0, 0.0), p(b, y0, 0.0), p(b, y1, 0.0), p(a, y1, 0.0)],
            [p(a, y1, -t), p(b, y1, -t), p(b, y0, -t), p(a, y0, -t)],
            [p(a, y0, -t), p(b, y0, -t), p(b, y0, 0.0), p(a, y0, 0.0)],
            [p(b, y1, -t), p(a, y1, -t), p(a, y1, 0.0), p(b, y1, 0.0)],
            [p(a, y1, -t), p(a, y0, -t), p(a, y0, 0.0), p(a, y1, 0.0)],
            [p(b, y0, -t), p(b, y1, -t), p(b, y1, 0.0), p(b, y0, 0.0)],
        ];
        quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect()
    }

    #[test]
    fn caps_between_abutting_bars_are_interior_but_box_faces_and_thin_walls_stay() {
        // A kinked bar: a sloped box down to x = 2, a level box on from
        // x = 2.01 (one centimetre apart, as in GTA's handrails).
        let mut tris = bar(Vec3::new(0.0, 0.0, 1.0), Vec3::new(2.0, 0.0, 0.0), 0.15, 0.04);
        tris.extend(bar(Vec3::new(2.01, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0), 0.15, 0.04));
        let found = abutting_caps(&tris);
        // Caps are faces 4 and 5 (triangles 8..12) of each box.
        let (slope_end, level_start) = ([10, 11], [12 + 8, 12 + 9]);
        for i in slope_end.into_iter().chain(level_start) {
            assert!(found.contains(&i), "joint cap {i} interior: {found:?}");
        }
        for i in [8, 9, 12 + 10, 12 + 11] {
            assert!(!found.contains(&i), "outer end cap {i} stays: {found:?}");
        }
        assert_eq!(found.len(), 4, "only the joint caps: {found:?}");
        // A single 15 cm cube and a 1 cm thick plate: nothing is interior.
        let cube = bar(Vec3::new(0.0, 5.0, 1.0), Vec3::new(0.15, 5.0, 1.0), 0.15, 0.15);
        assert!(abutting_caps(&cube).is_empty());
        let plate = bar(Vec3::new(0.0, 8.0, 1.0), Vec3::new(1.0, 8.0, 1.0), 0.01, 1.0);
        assert!(abutting_caps(&plate).is_empty(), "broad faces of a thin plate stay");
        // A 2 cm sheet laid as small tiles: top and bottom tiles face each
        // other's planes 2 cm apart, but each rim is only 2 cm deep, so they
        // stay. (The tiles' touching end caps are interior.)
        let mut sheet = Vec::new();
        for k in 0..4 {
            let x = k as f32 * 0.3;
            sheet.extend(bar(Vec3::new(x, 12.0, 1.0), Vec3::new(x + 0.3, 12.0, 1.0), 0.3, 0.02));
        }
        for i in abutting_caps(&sheet) {
            let t = sheet[i];
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize();
            assert!(n.z.abs() < 0.5, "sheet top/bottom face {i} stays (n {n})");
        }
    }

    #[test]
    fn landing_panel_end_inside_the_flight_panel_is_buried_but_resting_boxes_stay() {
        // GTA's kinked handrail: the flight's sloped panel (16 cm wide, 0.9 m
        // deep) runs 0.4 m on into the landing's level panel, whose top it
        // meets at x = 2.
        let mut tris = bar(Vec3::new(0.0, 0.0, 1.0), Vec3::new(2.4, 0.0, -0.2), 0.16, 0.9);
        let level = tris.len();
        tris.extend(bar(Vec3::new(2.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0), 0.15, 0.9));
        let found = buried_faces(&tris);
        // Faces 4 and 5 are the caps at `a` and `b` (triangles 8..12).
        assert!(found.contains(&(level + 8)) && found.contains(&(level + 9)), "landing panel's end inside the flight: {found:?}");
        assert!(!found.contains(&(level + 10)) && !found.contains(&(level + 11)), "its far end stays: {found:?}");
        // Tops (faces 0) of both panels stay; the flight's end (inside the
        // landing panel) may go.
        for i in [0, 1, level, level + 1] {
            assert!(!found.contains(&i), "top {i} stays: {found:?}");
        }
        // A crate resting on another, a plate lying flush on a box and a lone
        // box keep every face.
        let mut crates = bar(Vec3::new(0.0, 5.0, 1.0), Vec3::new(1.0, 5.0, 1.0), 1.0, 1.0);
        crates.extend(bar(Vec3::new(0.0, 5.0, 2.0), Vec3::new(1.0, 5.0, 2.0), 1.0, 1.0));
        crates.extend(bar(Vec3::new(3.0, 5.0, 1.01), Vec3::new(4.0, 5.0, 1.01), 0.5, 0.01));
        crates.extend(bar(Vec3::new(3.0, 5.0, 1.0), Vec3::new(4.0, 5.0, 1.0), 1.0, 1.0));
        assert!(buried_faces(&crates).is_empty(), "stacked crates and a flush plate stay");
    }
}

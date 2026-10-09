//! GTA collision bounds decoded into triangles, independent of where the
//! bytes come from (the runtime reads GTA's memory, a bake reads resources):
//! the skater flag rule, authored primitive headers, geometry polygons and
//! the shapes primitives become. Arithmetic follows `glam::Vec3`'s operation
//! order term for term, so results are bit-identical to the glam versions.

use crate::Tri;

pub type V3 = [f32; 3];

/// CodeWalker `EBoundCompositeFlags`.
pub const PED: u32 = 1 << 9;
pub const FOLIAGE: u32 = 1 << 19;
pub const TEST_WEAPON: u32 = 1 << 21;
pub const TEST_CAMERA: u32 = 1 << 22;

/// What a skater collides with (the bake's `primitives::skater_collides`):
/// what a ped collides with, minus foliage. An archetype with no flags is kept.
pub fn skater_collides(type_flags: u32, include: u32) -> bool {
    if type_flags == 0 && include == 0 {
        return true;
    }
    type_flags & FOLIAGE == 0 && include & PED != 0
}

/// A surface GTA's bullets and camera stop at that a ped passes: the
/// bullet/camera copy of the map (`hi@` bounds, include `0x00610000`), which
/// has surfaces the ped collision skips. Never foliage or vehicle-only
/// barriers (they stop neither). Gap filler only (`clean::fill_gaps`).
pub fn solid_to_sight(type_flags: u32, include: u32) -> bool {
    !skater_collides(type_flags, include) && type_flags & FOLIAGE == 0 && include & (TEST_WEAPON | TEST_CAMERA) == TEST_WEAPON | TEST_CAMERA
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn neg(a: V3) -> V3 {
    [-a[0], -a[1], -a[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    (a[0] * b[0]) + (a[1] * b[1]) + (a[2] * b[2])
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - b[1] * a[2], a[2] * b[0] - b[2] * a[0], a[0] * b[1] - b[0] * a[1]]
}
fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}
fn normalize(a: V3) -> V3 {
    mul(a, 1.0 / length(a))
}
fn finite(a: V3) -> bool {
    a.iter().all(|v| v.is_finite())
}

/// A bound's placement: row vectors (x, y, z axes, position), `p' = x*r0 +
/// y*r1 + z*r2 + r3`; `identity` skips the transform.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub rows: [V3; 4],
    pub identity: bool,
}

impl Place {
    pub fn apply(&self, p: V3) -> V3 {
        if self.identity {
            return p;
        }
        add(add(add(mul(self.rows[0], p[0]), mul(self.rows[1], p[1])), mul(self.rows[2], p[2])), self.rows[3])
    }
}

/// The XY rectangle (min x, min y, max x, max y) a triangle's box must touch.
pub type Clip = (f32, f32, f32, f32);

fn touches(t: &[V3; 3], clip: Clip) -> bool {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in t {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    hi[0] >= clip.0 && lo[0] <= clip.2 && hi[1] >= clip.1 && lo[1] <= clip.3
}

/// What a decode found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub triangles: usize,
    pub primitives: usize,
    pub vegetation: usize,
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn v3_at(b: &[u8], o: usize) -> V3 {
    [f32_at(b, o), f32_at(b, o + 4), f32_at(b, o + 8)]
}
fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

/// GRASS_LONG, GRASS, GRASS_SHORT: the vegetation materials GTA also uses for
/// lawn slabs (flat boxes peds walk on, e.g. city park verges).
fn lawn_material(material: u8) -> bool {
    matches!(material, 46..=48)
}

/// A vegetation volume that stays collision, by its placed bounds: a lawn
/// (GRASS at most 1 m thick, at least 2 m across both ways) or a hedge or
/// tree (`clean::hedge`). Grass clumps and bushes stay out.
fn solid_vegetation(shape: &[[V3; 3]], place: &Place, material: u8) -> bool {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in shape.iter().flatten().map(|&p| place.apply(p)) {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    !shape.is_empty()
        && if lawn_material(material) {
            hi[2] - lo[2] <= 1.0 && hi[0] - lo[0] >= 2.0 && hi[1] - lo[1] >= 2.0
        } else {
            crate::clean::hedge(material, lo, hi)
        }
}

/// An authored primitive bound (sphere, capsule, box, disc, cylinder) from
/// its first 0x60 header bytes: type `+0x10`, radius `+0x14`, AABB `+0x20` /
/// `+0x30`, margin `+0x2C`, material `+0x4C`, centre `+0x50`.
pub fn primitive(h: &[u8], place: &Place, clip: Clip, out: &mut Vec<Tri>, counts: &mut Counts) {
    let (lo, hi, c) = (v3_at(h, 0x30), v3_at(h, 0x20), v3_at(h, 0x50));
    let (radius, margin, material) = (f32_at(h, 0x14), f32_at(h, 0x2C), h[0x4C]);
    if !(finite(lo) && finite(hi) && finite(c) && radius.is_finite() && margin.is_finite()) {
        return;
    }
    counts.primitives += 1;
    let (x, y) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let mut scratch: Vec<[V3; 3]> = Vec::new();
    match h[0x10] {
        0 if radius > 0.0 => shapes::sphere(c, radius, &mut scratch),
        1 if margin > 0.0 && radius >= margin => {
            let half = mul(y, radius - margin);
            shapes::capsule(sub(c, half), add(c, half), margin, &mut scratch);
        }
        3 if hi[0] > lo[0] && hi[1] > lo[1] && hi[2] > lo[2] => shapes::box_from_corners(
            [lo, [hi[0], hi[1], lo[2]], [hi[0], lo[1], hi[2]], [lo[0], hi[1], hi[2]]],
            &mut scratch,
        ),
        12 if radius > 0.0 && margin > 0.0 => {
            let half = mul(x, margin);
            shapes::cylinder(sub(c, half), add(c, half), radius, &mut scratch);
        }
        13 if hi[0] > lo[0] && hi[1] > lo[1] => {
            let half = mul(y, (hi[1] - lo[1]) * 0.5);
            shapes::cylinder(sub(c, half), add(c, half), (hi[0] - lo[0]) * 0.5, &mut scratch);
        }
        _ => {}
    }
    let full = Place { identity: false, ..*place };
    if crate::clean::soft_vegetation(material) && !solid_vegetation(&scratch, &full, material) {
        counts.vegetation += 1;
        return;
    }
    for t in scratch {
        // Primitive bounds always take the full transform.
        let w = t.map(|p| full.apply(p));
        if w.iter().all(|v| finite(*v)) && touches(&w, clip) {
            out.push(Tri { v: w, material });
        }
    }
}

/// A geometry / BVH bound's polygons (CodeWalker `Bounds.cs`): `polys` 16 B
/// records (type = byte 0 & 7), `verts` i16 x 3 dequantised by `quantum`
/// about `centre`, per-polygon material indices into 8 B `materials`
/// (either may be empty: material 0). Triangles and primitive polygons as
/// the bake decodes them, so a triangle's vertex bits match its `.svsd` key.
#[allow(clippy::too_many_arguments)]
pub fn geometry(
    verts: &[u8],
    polys: &[u8],
    materials: &[u8],
    poly_material: &[u8],
    quantum: V3,
    centre: V3,
    place: &Place,
    clip: Clip,
    out: &mut Vec<Tri>,
    counts: &mut Counts,
) {
    let nv = verts.len() / 6;
    let local = |i: u16| -> Option<V3> {
        let i = (i & 0x7FFF) as usize;
        if i >= nv {
            return None;
        }
        let c = |k: usize| i16::from_le_bytes([verts[i * 6 + k * 2], verts[i * 6 + k * 2 + 1]]) as f32;
        Some(add([c(0) * quantum[0], c(1) * quantum[1], c(2) * quantum[2]], centre))
    };
    let push = |t: [V3; 3], material: u8, out: &mut Vec<Tri>| {
        if t.iter().all(|v| finite(*v)) && touches(&t, clip) {
            out.push(Tri { v: t, material });
        }
    };
    let mut scratch: Vec<[V3; 3]> = Vec::new();
    for (i, rec) in polys.as_chunks::<16>().0.iter().enumerate() {
        let kind = rec[0] & 7;
        let material = poly_material.get(i).and_then(|&m| materials.get(m as usize * 8)).map_or(0, |&m| m);
        if kind == 0 {
            let v = [u16_at(rec, 4), u16_at(rec, 6), u16_at(rec, 8)].map(local);
            if let [Some(a), Some(b), Some(c)] = v {
                counts.triangles += 1;
                push([a, b, c].map(|p| place.apply(p)), material, out);
            }
            continue;
        }
        counts.primitives += 1;
        scratch.clear();
        let ok = match kind {
            1 => local(u16_at(rec, 2)).map(|c| shapes::sphere(c, f32_at(rec, 4), &mut scratch)),
            2 => local(u16_at(rec, 2)).zip(local(u16_at(rec, 8))).map(|(a, b)| shapes::capsule(a, b, f32_at(rec, 4), &mut scratch)),
            3 => match [4, 6, 8, 10].map(|o| local(u16_at(rec, o))) {
                [Some(a), Some(b), Some(c), Some(d)] => {
                    shapes::box_from_corners([a, b, c, d], &mut scratch);
                    Some(())
                }
                _ => None,
            },
            4 => local(u16_at(rec, 2)).zip(local(u16_at(rec, 8))).map(|(a, b)| shapes::cylinder(a, b, f32_at(rec, 4), &mut scratch)),
            _ => None,
        };
        if crate::clean::soft_vegetation(material) && !(ok.is_some() && solid_vegetation(&scratch, place, material)) {
            counts.vegetation += 1;
            continue;
        }
        if ok.is_some() {
            for t in scratch.drain(..) {
                push(t.map(|p| place.apply(p)), material, out);
            }
        }
    }
}

/// Primitive polygons as triangles: the bake's shapes
/// (`world-cache/src/primitives.rs`), same sides, winding and order.
pub mod shapes {
    use super::{add, cross, dot, length, mul, neg, normalize, sub, V3};

    const SIDES: usize = 10;

    fn push_quad(out: &mut Vec<[V3; 3]>, q: [V3; 4]) {
        out.push([q[0], q[1], q[2]]);
        out.push([q[0], q[2], q[3]]);
    }

    /// RAGE box polygon: four corners, no two opposite; the other four are
    /// their reflections through the centre. Faces wound outward.
    pub fn box_from_corners(v: [V3; 4], out: &mut Vec<[V3; 3]>) {
        let c = mul(add(add(add(v[0], v[1]), v[2]), v[3]), 0.25);
        let hx = mul(sub(sub(add(v[1], v[2]), v[0]), v[3]), 0.25);
        let hy = mul(sub(sub(add(v[1], v[3]), v[0]), v[2]), 0.25);
        let hz = mul(sub(sub(add(v[2], v[3]), v[0]), v[1]), 0.25);
        let corner = |sx: f32, sy: f32, sz: f32| add(add(add(c, mul(hx, sx)), mul(hy, sy)), mul(hz, sz));
        let faces = [
            [(1., -1., -1.), (1., 1., -1.), (1., 1., 1.), (1., -1., 1.)],
            [(-1., -1., -1.), (-1., -1., 1.), (-1., 1., 1.), (-1., 1., -1.)],
            [(-1., 1., -1.), (-1., 1., 1.), (1., 1., 1.), (1., 1., -1.)],
            [(-1., -1., -1.), (1., -1., -1.), (1., -1., 1.), (-1., -1., 1.)],
            [(-1., -1., 1.), (1., -1., 1.), (1., 1., 1.), (-1., 1., 1.)],
            [(-1., -1., -1.), (-1., 1., -1.), (1., 1., -1.), (1., -1., -1.)],
        ];
        // The corner set may be mirrored (left-handed axes): flip to keep outward winding.
        let flip = dot(cross(hx, hy), hz) < 0.0;
        for f in faces {
            let mut q = f.map(|(x, y, z)| corner(x, y, z));
            if flip {
                q.reverse();
            }
            push_quad(out, q);
        }
    }

    /// Orthonormal pair perpendicular to `axis` (unit).
    fn perpendiculars(axis: V3) -> (V3, V3) {
        let helper = if axis[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
        let u = normalize(cross(axis, helper));
        (u, cross(axis, u))
    }

    /// Ring points around `c` (counterclockwise seen from +axis).
    fn ring(c: V3, u: V3, w: V3, r: f32) -> Vec<V3> {
        (0..SIDES)
            .map(|k| {
                let a = std::f32::consts::TAU * k as f32 / SIDES as f32;
                add(add(c, mul(u, r * a.cos())), mul(w, r * a.sin()))
            })
            .collect()
    }

    /// Prism side walls between rings `lo` (at the axis start) and `hi`.
    fn walls(lo: &[V3], hi: &[V3], out: &mut Vec<[V3; 3]>) {
        for k in 0..SIDES {
            let n = (k + 1) % SIDES;
            push_quad(out, [lo[k], lo[n], hi[n], hi[k]]);
        }
    }

    /// Fan cap; outward along +axis when `up`.
    fn cap(center: V3, ring: &[V3], up: bool, out: &mut Vec<[V3; 3]>) {
        for k in 0..SIDES {
            let n = (k + 1) % SIDES;
            out.push(if up { [center, ring[k], ring[n]] } else { [center, ring[n], ring[k]] });
        }
    }

    pub fn cylinder(a: V3, b: V3, r: f32, out: &mut Vec<[V3; 3]>) {
        let d = sub(b, a);
        if r.is_nan() || r <= 0.0 || length(d) < 1e-4 {
            return;
        }
        let axis = normalize(d);
        let (u, w) = perpendiculars(axis);
        let lo = ring(a, u, w, r);
        let hi = ring(b, u, w, r);
        walls(&lo, &hi, out);
        cap(b, &hi, true, out);
        cap(a, &lo, false, out);
    }

    /// Cylinder plus a pointed cap of one radius at each end.
    pub fn capsule(a: V3, b: V3, r: f32, out: &mut Vec<[V3; 3]>) {
        let d = sub(b, a);
        if r.is_nan() || r <= 0.0 {
            return;
        }
        if length(d) < 1e-4 {
            sphere(a, r, out);
            return;
        }
        let axis = normalize(d);
        let (u, w) = perpendiculars(axis);
        let lo = ring(a, u, w, r);
        let hi = ring(b, u, w, r);
        walls(&lo, &hi, out);
        let top = add(b, mul(axis, r));
        let bottom = sub(a, mul(axis, r));
        for k in 0..SIDES {
            let n = (k + 1) % SIDES;
            out.push([top, hi[k], hi[n]]);
            out.push([bottom, lo[n], lo[k]]);
        }
    }

    /// Octahedron subdivided once and pushed to the radius.
    pub fn sphere(c: V3, r: f32, out: &mut Vec<[V3; 3]>) {
        if r.is_nan() || r <= 0.0 {
            return;
        }
        let (px, py, pz) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        let faces = [
            [px, py, pz],
            [py, neg(px), pz],
            [neg(px), neg(py), pz],
            [neg(py), px, pz],
            [py, px, neg(pz)],
            [neg(px), py, neg(pz)],
            [neg(py), neg(px), neg(pz)],
            [px, neg(py), neg(pz)],
        ];
        let at = |v: V3| add(c, mul(normalize(v), r));
        for [a, b, d] in faces {
            let (ab, bd, da) = (mul(add(a, b), 0.5), mul(add(b, d), 0.5), mul(add(d, a), 0.5));
            for t in [[a, ab, da], [ab, b, bd], [da, bd, d], [ab, bd, da]] {
                out.push(t.map(at));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_rule_keeps_ped_collision_and_drops_foliage() {
        assert!(skater_collides(0x3e, 0x07f3_bec0), "map collision");
        assert!(skater_collides(0, 0), "no flags: kept");
        assert!(solid_to_sight(0x2, 0x0061_0000), "the bullet/camera copy fills gaps");
        assert!(!solid_to_sight(0x3e, 0x07f3_bec0), "walkable map is not a filler");
        assert!(!solid_to_sight(0x20, 0x0000_0080), "vehicle-only barriers never fill");
        assert!(!skater_collides(0x2, 0x0061_0000), "bullet-only copy");
        assert!(!skater_collides(0x10, 0x03e0_0000), "cover-only");
        assert!(!skater_collides(FOLIAGE, PED), "foliage");
    }

    #[test]
    fn shapes_have_their_triangle_counts_and_radius() {
        let mut out = Vec::new();
        shapes::sphere([1.0, 2.0, 3.0], 2.0, &mut out);
        assert_eq!(out.len(), 32);
        assert!(out.iter().flatten().all(|p| (length(sub(*p, [1.0, 2.0, 3.0])) - 2.0).abs() < 1e-5));
        out.clear();
        shapes::cylinder([0.0; 3], [0.0, 0.0, 2.0], 0.5, &mut out);
        assert_eq!(out.len(), 40);
        out.clear();
        shapes::capsule([0.0; 3], [2.0, 0.0, 0.0], 0.5, &mut out);
        assert_eq!(out.len(), 40);
        out.clear();
        shapes::box_from_corners([[0.0; 3], [1.0, 1.0, 0.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]], &mut out);
        assert_eq!(out.len(), 12);
    }
    /// One BVH box polygon of `material` spanning `size` (corners at 0 and
    /// the RAGE corner pattern), as `geometry` decodes it.
    fn box_poly(size: V3, material: u8) -> (Vec<Tri>, Counts) {
        let corners = [[0.0, 0.0, 0.0], [size[0], size[1], 0.0], [size[0], 0.0, size[2]], [0.0, size[1], size[2]]];
        let verts: Vec<u8> = corners.iter().flat_map(|c| c.iter().flat_map(|&v| ((v * 100.0) as i16).to_le_bytes())).collect();
        let mut poly = [0u8; 16];
        poly[0] = 3;
        for (k, o) in [4, 6, 8, 10].into_iter().enumerate() {
            poly[o..o + 2].copy_from_slice(&(k as u16).to_le_bytes());
        }
        let mut mat = [0u8; 8];
        mat[0] = material;
        let (mut out, mut counts) = (Vec::new(), Counts::default());
        let place = Place { rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]], identity: true };
        geometry(&verts, &poly, &mat, &[0], [0.01; 3], [0.0; 3], &place, (-1e4, -1e4, 1e4, 1e4), &mut out, &mut counts);
        (out, counts)
    }

    #[test]
    fn grass_slabs_are_ground_and_clumps_are_not() {
        let (lawn, c) = box_poly([8.0, 6.0, 0.4], 47);
        assert_eq!((lawn.len(), c.vegetation), (12, 0), "thin wide GRASS box: a lawn");
        let (clump, c) = box_poly([1.0, 1.0, 0.4], 47);
        assert_eq!((clump.len(), c.vegetation), (0, 1), "small grass clump");
        let (tall, c) = box_poly([8.0, 6.0, 2.0], 47);
        assert_eq!((tall.len(), c.vegetation), (0, 1), "tall grass volume");
        let (bush, c) = box_poly([8.0, 6.0, 0.4], 50);
        assert_eq!((bush.len(), c.vegetation), (0, 1), "BUSHES never ground");
        let (hedge, c) = box_poly([6.0, 1.2, 2.0], 50);
        assert_eq!((hedge.len(), c.vegetation), (12, 0), "a hedge is solid");
        let (clump, c) = box_poly([1.5, 1.5, 1.2], 50);
        assert_eq!((clump.len(), c.vegetation), (0, 1), "a bush is not");
    }
}

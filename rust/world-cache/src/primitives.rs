//! Collision primitives (boxes, spheres, capsules, cylinders) as triangles.
//!
//! `rage-formats` decodes only the triangle polygons of a geometry bound and
//! counts the rest; GTA builds much of its static collision (building walls,
//! posts, bollards, trunks) from these primitive polygons, which therefore
//! had no collision in skate mode (owner report 06:3x: buildings pass-through;
//! 1.24 M primitive polygons across the base map, e.g. `bh1_42_0.ybn`
//! 13,790 triangles + 1,998 primitives).
//!
//! Composite child flags decide what is kept (`skater_collides`).
//!
//! The bound tree is walked here with `rage-formats`' public resource reader,
//! using the same layout and transform order as its triangle path (ported
//! from CodeWalker.Core `Bounds.cs`). Output is world space, outward faces
//! counterclockwise, like the triangle path.
use rage_formats::math::Vec3;
use rage_formats::resource::{
    ResReader, SYSTEM_BASE, f32_le, prepare_rsc7, u16_le, u32_le, u64_le, vec3_le,
};
use rage_formats::{BoundTransform, Triangle};

/// Sides of the prisms that stand in for cylinders and capsules.
const SIDES: usize = 10;

/// Composite child flags (CodeWalker `EBoundCompositeFlags`): the child's
/// type flags and the include flags of what collides with it.
pub const FOLIAGE: u32 = 1 << 19;
pub const PED: u32 = 1 << 9;

/// What the skater collides with: what a ped collides with, minus foliage
/// (peds push through bushes; 176 K foliage primitives). Weapon-, cover-,
/// vehicle- and animal-only children are left out (no PED include flag).
/// A bound with no composite parent carries no flags and is kept.
pub fn skater_collides((kind, include): (u32, u32)) -> bool {
    if kind == 0 && include == 0 {
        return true;
    }
    kind & FOLIAGE == 0 && include & PED != 0
}

/// Soft vegetation GTA authors as volumes: bush and grass clumps as boxes,
/// spheres and capsules (GRASS_LONG, GRASS, GRASS_SHORT, BUSHES, TWIGS,
/// LEAVES; HAY bales and tree bark stay solid). Peds push through them; as
/// skate collision they were invisible cubes (owner 2026-10-05: "small,
/// medium, large bushes and some grass are just big cubes"). Triangle-mesh
/// grass is ground and is kept.
pub fn soft_vegetation(material: u8) -> bool {
    matches!(material, 46..=48 | 50..=52)
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub primitives: bool,
    /// Keep every child regardless of flags (the pre-filter behaviour).
    pub all_flags: bool,
}

#[derive(Default, Debug, Clone)]
pub struct Counts {
    /// Polygons by the owning composite child's (type, include) flags.
    pub by_flags: std::collections::BTreeMap<(u32, u32), usize>,
    pub kept_polygons: usize,
    pub dropped_polygons: usize,
    pub boxes: usize,
    pub spheres: usize,
    pub capsules: usize,
    pub cylinders: usize,
    pub skipped: usize,
    /// Vegetation volumes left out (`soft_vegetation`).
    pub vegetation: usize,
    /// Diagnostics: when set, (first output triangle, child flags) of every
    /// geometry emitted, so a triangle can be traced to its child flags.
    pub record_spans: bool,
    pub spans: Vec<(usize, (u32, u32))>,
}

/// World-space triangles of a `.ybn`: triangle polygons (decoded as
/// `rage-formats` does) plus triangulated primitives, for the bound children
/// the skater collides with.
pub fn triangles(
    data: &[u8],
    options: Options,
    counts: &mut Counts,
) -> Result<Vec<Triangle>, String> {
    let (system, graphics) = prepare_rsc7(data).map_err(|e| format!("{e:#}"))?;
    let r = ResReader {
        system: &system,
        graphics: &graphics,
    };
    Ok(triangles_at(&r, SYSTEM_BASE, options, counts))
}

/// As [`triangles`], for a bound tree rooted at `va` inside another
/// resource (a drawable's embedded bound).
pub fn triangles_at(
    r: &ResReader<'_>,
    va: u64,
    options: Options,
    counts: &mut Counts,
) -> Vec<Triangle> {
    let mut out = Vec::new();
    walk(r, va, 0, &mut Vec::new(), (0, 0), options, &mut out, counts);
    out
}

fn walk(
    r: &ResReader<'_>,
    va: u64,
    depth: usize,
    stack: &mut Vec<BoundTransform>,
    flags: (u32, u32),
    options: Options,
    out: &mut Vec<Triangle>,
    counts: &mut Counts,
) {
    if depth > 8 {
        return;
    }
    let Some(head) = r.resolve(va, 0x70) else {
        return;
    };
    match head[0x10] {
        // Authored bound primitives (common in YFT physics composites), as
        // opposed to primitive polygon records inside a geometry bound.
        0 | 1 | 3 | 12 | 13 => {
            let keep = options.all_flags || skater_collides(flags);
            *counts.by_flags.entry(flags).or_default() += 1;
            if !keep {
                counts.dropped_polygons += 1;
                return;
            }
            counts.kept_polygons += 1;
            if !options.primitives {
                return;
            }
            let material = head[0x4c];
            if !options.all_flags && soft_vegetation(material) {
                counts.vegetation += 1;
                return;
            }
            if counts.record_spans {
                counts.spans.push((out.len(), flags));
            }
            let mut local = Vec::new();
            bound_primitive(head, &mut local, counts);
            for vertices in local {
                out.push(Triangle {
                    vertices: vertices.map(|p| stack.iter().rev().fold(p, |p, xf| xf.apply(p))),
                    material,
                });
            }
        }
        // Geometry / GeometryBvh
        4 | 8 => {
            let keep = options.all_flags || skater_collides(flags);
            if counts.record_spans {
                counts.spans.push((out.len(), flags));
            }
            let polygons = geometry(r, va, stack, keep, options.primitives, out, counts);
            *counts.by_flags.entry(flags).or_default() += polygons;
            if keep {
                counts.kept_polygons += polygons;
            } else {
                counts.dropped_polygons += polygons;
            }
        }
        // Composite
        10 => {
            let Some(b) = r.resolve(va, 0xB0) else { return };
            let children = u64_le(b, 0x70);
            let transforms = u64_le(b, 0x78);
            let flags_ptr = u64_le(b, 0x90);
            let count = u16_le(b, 0xA0) as usize;
            if count == 0 {
                return;
            }
            let child_flags: Vec<(u32, u32)> = match r.resolve_optional(flags_ptr, count * 8) {
                Some(Some(fb)) => (0..count)
                    .map(|i| (u32_le(fb, i * 8), u32_le(fb, i * 8 + 4)))
                    .collect(),
                _ => vec![(0, 0); count],
            };
            let Some(pointers) = r.read_u64_list(children, count) else {
                return;
            };
            let xfs: Vec<BoundTransform> = match r.resolve_optional(transforms, count * 64) {
                Some(Some(tb)) => (0..count)
                    .map(|i| {
                        let o = i * 64;
                        BoundTransform {
                            columns: [
                                vec3_le(tb, o),
                                vec3_le(tb, o + 16),
                                vec3_le(tb, o + 32),
                                vec3_le(tb, o + 48),
                            ],
                        }
                    })
                    .collect(),
                _ => vec![BoundTransform::identity(); count],
            };
            for (i, &p) in pointers.iter().enumerate() {
                if p != 0 {
                    stack.push(xfs[i]);
                    walk(r, p, depth + 1, stack, child_flags[i], options, out, counts);
                    stack.pop();
                }
            }
        }
        _ => {}
    }
}

// phBound header layout: pinned rage-formats ybn.rs and CodeWalker Bounds.cs
// (BoundSphere/Capsule/Box/Disc/Cylinder). Bounds are authored collision shapes,
// not the drawable/model AABB. Composite transforms are applied by walk().
fn bound_primitive(b: &[u8], out: &mut Vec<[Vec3; 3]>, counts: &mut Counts) {
    let lo = vec3_le(b, 0x30);
    let hi = vec3_le(b, 0x20);
    let c = vec3_le(b, 0x50);
    let radius = f32_le(b, 0x14);
    let margin = f32_le(b, 0x2c);
    if ![
        lo.x, lo.y, lo.z, hi.x, hi.y, hi.z, c.x, c.y, c.z, radius, margin,
    ]
    .iter()
    .all(|x| x.is_finite())
    {
        counts.skipped += 1;
        return;
    }
    match b[0x10] {
        0 if radius > 0. => {
            counts.spheres += 1;
            sphere(c, radius, out);
        }
        1 if margin > 0. && radius >= margin => {
            counts.capsules += 1;
            let half = Vec3::Y * (radius - margin);
            capsule(c - half, c + half, margin, out);
        }
        3 if hi.x > lo.x && hi.y > lo.y && hi.z > lo.z => {
            counts.boxes += 1;
            box_from_corners(
                [
                    lo,
                    Vec3::new(hi.x, hi.y, lo.z),
                    Vec3::new(hi.x, lo.y, hi.z),
                    Vec3::new(lo.x, hi.y, hi.z),
                ],
                out,
            );
        }
        12 if radius > 0. && margin > 0. => {
            counts.cylinders += 1;
            let half = Vec3::X * margin;
            cylinder(c - half, c + half, radius, out);
        }
        13 if hi.x > lo.x && hi.y > lo.y => {
            counts.cylinders += 1;
            let half = Vec3::Y * ((hi.y - lo.y) * 0.5);
            cylinder(c - half, c + half, (hi.x - lo.x) * 0.5, out);
        }
        _ => {
            counts.skipped += 1;
        }
    }
}

/// Emits the geometry's triangles (when `keep`); returns its polygon count.
fn geometry(
    r: &ResReader<'_>,
    va: u64,
    stack: &[BoundTransform],
    keep: bool,
    primitives: bool,
    out: &mut Vec<Triangle>,
    counts: &mut Counts,
) -> usize {
    let Some(b) = r.resolve(va, 0x130) else {
        return 0;
    };
    let polygons_ptr = u64_le(b, 0x88);
    let quantum = vec3_le(b, 0x90);
    let center = vec3_le(b, 0xA0);
    let vertices_ptr = u64_le(b, 0xB0);
    let vertices_count = u32_le(b, 0xD0) as usize;
    let polygons_count = u32_le(b, 0xD4) as usize;
    let materials_ptr = u64_le(b, 0xF0);
    let poly_material_ptr = u64_le(b, 0x118);
    let materials_count = (b[0x120] as usize).max(4);
    if polygons_count == 0 || vertices_count == 0 {
        return 0;
    }
    if !keep {
        return polygons_count;
    }
    let Some(vb) = r.resolve(vertices_ptr, vertices_count * 6) else {
        return 0;
    };
    let Some(pb) = r.resolve(polygons_ptr, polygons_count * 16) else {
        return 0;
    };
    let materials: Vec<u32> = match r.resolve_optional(materials_ptr, materials_count * 8) {
        Some(Some(mb)) => (0..materials_count).map(|i| u32_le(mb, i * 8)).collect(),
        _ => Vec::new(),
    };
    let poly_materials: &[u8] = match r.resolve_optional(poly_material_ptr, polygons_count) {
        Some(Some(pm)) => pm,
        _ => &[],
    };
    // Local-space vertex (dequantised, offset by the geometry centre).
    let local = |i: u16| -> Option<Vec3> {
        let i = (i & 0x7FFF) as usize;
        if i >= vertices_count {
            return None;
        }
        let x = i16::from_le_bytes([vb[i * 6], vb[i * 6 + 1]]) as f32;
        let y = i16::from_le_bytes([vb[i * 6 + 2], vb[i * 6 + 3]]) as f32;
        let z = i16::from_le_bytes([vb[i * 6 + 4], vb[i * 6 + 5]]) as f32;
        Some(Vec3::new(x * quantum.x, y * quantum.y, z * quantum.z) + center)
    };
    // Innermost transform first, as the triangle path does.
    let world = |p: Vec3| stack.iter().rev().fold(p, |p, xf| xf.apply(p));

    for i in 0..polygons_count {
        let rec = &pb[i * 16..i * 16 + 16];
        let kind = rec[0] & 7;
        let material = poly_materials
            .get(i)
            .and_then(|&m| materials.get(m as usize))
            .map_or(0, |m| (m & 0xFF) as u8);
        if kind == 0 {
            // Triangle, as rage-formats decodes it (indices @4/6/8, top bit a flag).
            let v = [u16_le(rec, 4), u16_le(rec, 6), u16_le(rec, 8)].map(local);
            if let [Some(a), Some(b), Some(c)] = v {
                out.push(Triangle {
                    vertices: [a, b, c].map(world),
                    material,
                });
            }
            continue;
        }
        if !primitives {
            continue;
        }
        if soft_vegetation(material) {
            counts.vegetation += 1;
            continue;
        }
        let mut local_tris: Vec<[Vec3; 3]> = Vec::new();
        let ok = match kind {
            // Sphere: index @2, radius @4
            1 => local(u16_le(rec, 2)).map(|c| {
                counts.spheres += 1;
                sphere(c, f32_le(rec, 4), &mut local_tris)
            }),
            // Capsule: index1 @2, radius @4, index2 @8
            2 => local(u16_le(rec, 2))
                .zip(local(u16_le(rec, 8)))
                .map(|(a, b)| {
                    counts.capsules += 1;
                    capsule(a, b, f32_le(rec, 4), &mut local_tris)
                }),
            // Box: four corner indices @4..12
            3 => {
                let c = [
                    u16_le(rec, 4),
                    u16_le(rec, 6),
                    u16_le(rec, 8),
                    u16_le(rec, 10),
                ]
                .map(local);
                match c {
                    [Some(a), Some(b), Some(cc), Some(d)] => {
                        counts.boxes += 1;
                        Some(box_from_corners([a, b, cc, d], &mut local_tris))
                    }
                    _ => None,
                }
            }
            // Cylinder: index1 @2, radius @4, index2 @8
            4 => local(u16_le(rec, 2))
                .zip(local(u16_le(rec, 8)))
                .map(|(a, b)| {
                    counts.cylinders += 1;
                    cylinder(a, b, f32_le(rec, 4), &mut local_tris)
                }),
            _ => None,
        };
        if ok.is_none() {
            counts.skipped += 1;
            continue;
        }
        for t in local_tris {
            out.push(Triangle {
                vertices: t.map(world),
                material,
            });
        }
    }
    polygons_count
}

fn push_quad(out: &mut Vec<[Vec3; 3]>, q: [Vec3; 4]) {
    out.push([q[0], q[1], q[2]]);
    out.push([q[0], q[2], q[3]]);
}

/// RAGE box polygon: four corners, no two opposite; the other four are their
/// reflections through the centre. Faces wound outward (counterclockwise).
fn box_from_corners(v: [Vec3; 4], out: &mut Vec<[Vec3; 3]>) {
    let c = (v[0] + v[1] + v[2] + v[3]) * 0.25;
    let hx = (v[1] + v[2] - v[0] - v[3]) * 0.25;
    let hy = (v[1] + v[3] - v[0] - v[2]) * 0.25;
    let hz = (v[2] + v[3] - v[0] - v[1]) * 0.25;
    let corner = |sx: f32, sy: f32, sz: f32| c + hx * sx + hy * sy + hz * sz;
    let faces = [
        [(1., -1., -1.), (1., 1., -1.), (1., 1., 1.), (1., -1., 1.)],
        [
            (-1., -1., -1.),
            (-1., -1., 1.),
            (-1., 1., 1.),
            (-1., 1., -1.),
        ],
        [(-1., 1., -1.), (-1., 1., 1.), (1., 1., 1.), (1., 1., -1.)],
        [
            (-1., -1., -1.),
            (1., -1., -1.),
            (1., -1., 1.),
            (-1., -1., 1.),
        ],
        [(-1., -1., 1.), (1., -1., 1.), (1., 1., 1.), (-1., 1., 1.)],
        [
            (-1., -1., -1.),
            (-1., 1., -1.),
            (1., 1., -1.),
            (1., -1., -1.),
        ],
    ];
    // The corner set may be mirrored (left-handed axes): flip to keep outward winding.
    let flip = hx.cross(hy).dot(hz) < 0.0;
    for f in faces {
        let mut q = f.map(|(x, y, z)| corner(x, y, z));
        if flip {
            q.reverse();
        }
        push_quad(out, q);
    }
}

/// Orthonormal pair perpendicular to `axis` (unit).
fn perpendiculars(axis: Vec3) -> (Vec3, Vec3) {
    let helper = if axis.z.abs() < 0.9 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(1.0, 0.0, 0.0)
    };
    let u = axis.cross(helper).normalize();
    (u, axis.cross(u))
}

/// Ring points around `c` (counterclockwise seen from +axis).
fn ring(c: Vec3, u: Vec3, w: Vec3, r: f32) -> Vec<Vec3> {
    (0..SIDES)
        .map(|k| {
            let a = std::f32::consts::TAU * k as f32 / SIDES as f32;
            c + u * (r * a.cos()) + w * (r * a.sin())
        })
        .collect()
}

/// Prism side walls between rings `lo` (at the axis start) and `hi`.
fn walls(lo: &[Vec3], hi: &[Vec3], out: &mut Vec<[Vec3; 3]>) {
    for k in 0..SIDES {
        let n = (k + 1) % SIDES;
        push_quad(out, [lo[k], lo[n], hi[n], hi[k]]);
    }
}

/// Fan cap; `outward` along +axis when `up`.
fn cap(center: Vec3, ring: &[Vec3], up: bool, out: &mut Vec<[Vec3; 3]>) {
    for k in 0..SIDES {
        let n = (k + 1) % SIDES;
        out.push(if up {
            [center, ring[k], ring[n]]
        } else {
            [center, ring[n], ring[k]]
        });
    }
}

fn cylinder(a: Vec3, b: Vec3, r: f32, out: &mut Vec<[Vec3; 3]>) {
    let d = b - a;
    if !(r > 0.0) || d.length() < 1e-4 {
        return;
    }
    let axis = d.normalize();
    let (u, w) = perpendiculars(axis);
    let lo = ring(a, u, w, r);
    let hi = ring(b, u, w, r);
    walls(&lo, &hi, out);
    cap(b, &hi, true, out);
    cap(a, &lo, false, out);
}

/// Cylinder plus a pointed cap of one radius at each end.
fn capsule(a: Vec3, b: Vec3, r: f32, out: &mut Vec<[Vec3; 3]>) {
    let d = b - a;
    if !(r > 0.0) {
        return;
    }
    if d.length() < 1e-4 {
        sphere(a, r, out);
        return;
    }
    let axis = d.normalize();
    let (u, w) = perpendiculars(axis);
    let lo = ring(a, u, w, r);
    let hi = ring(b, u, w, r);
    walls(&lo, &hi, out);
    let top = b + axis * r;
    let bottom = a - axis * r;
    for k in 0..SIDES {
        let n = (k + 1) % SIDES;
        out.push([top, hi[k], hi[n]]);
        out.push([bottom, lo[n], lo[k]]);
    }
}

/// Octahedron subdivided once and pushed to the radius.
fn sphere(c: Vec3, r: f32, out: &mut Vec<[Vec3; 3]>) {
    if !(r > 0.0) {
        return;
    }
    let px = Vec3::new(1.0, 0.0, 0.0);
    let py = Vec3::new(0.0, 1.0, 0.0);
    let pz = Vec3::new(0.0, 0.0, 1.0);
    let faces = [
        [px, py, pz],
        [py, -px, pz],
        [-px, -py, pz],
        [-py, px, pz],
        [py, px, -pz],
        [-px, py, -pz],
        [-py, -px, -pz],
        [px, -py, -pz],
    ];
    let at = |v: Vec3| c + v.normalize() * r;
    for [a, b, d] in faces {
        let (ab, bd, da) = ((a + b) * 0.5, (b + d) * 0.5, (d + a) * 0.5);
        for t in [[a, ab, da], [ab, b, bd], [da, bd, d], [ab, bd, da]] {
            out.push(t.map(at));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_bound(
        kind: u8,
        min: Vec3,
        max: Vec3,
        center: Vec3,
        radius: f32,
        margin: f32,
    ) -> Vec<u8> {
        let mut b = vec![0u8; 0x80];
        b[0x10] = kind;
        b[0x14..0x18].copy_from_slice(&radius.to_le_bytes());
        b[0x2c..0x30].copy_from_slice(&margin.to_le_bytes());
        b[0x4c] = 17;
        for (o, v) in [(0x20, max), (0x30, min), (0x50, center)] {
            for (k, f) in [v.x, v.y, v.z].into_iter().enumerate() {
                b[o + k * 4..o + k * 4 + 4].copy_from_slice(&f.to_le_bytes());
            }
        }
        b
    }

    fn decode_bound(b: &[u8]) -> Vec<Triangle> {
        let r = ResReader {
            system: b,
            graphics: &[],
        };
        triangles_at(
            &r,
            SYSTEM_BASE,
            Options {
                primitives: true,
                all_flags: false,
            },
            &mut Counts::default(),
        )
    }

    #[test]
    fn bush_volumes_are_not_collision() {
        let mut b = raw_bound(3, Vec3::new(-1., -1., 0.), Vec3::new(1., 1., 1.5), Vec3::ZERO, 0., 0.);
        assert_eq!(decode_bound(&b).len(), 12, "a solid (material 17) box");
        b[0x4c] = 50; // BUSHES
        assert!(decode_bound(&b).is_empty(), "a bush box");
        b[0x4c] = 49; // HAY bale
        assert_eq!(decode_bound(&b).len(), 12, "hay bales stay solid");
    }

    #[test]
    fn authored_bound_box_is_not_the_parent_model_box() {
        // BoundBox geometry is BoxMin/BoxMax, not SphereCenter + model bounds.
        let min = Vec3::new(-1., -2., 0.);
        let max = Vec3::new(1., 2., 0.1);
        let b = raw_bound(3, min, max, Vec3::new(99., 99., 99.), 1., 0.01);
        let tris = decode_bound(&b);
        assert_eq!(tris.len(), 12);
        assert!(tris.iter().all(|t| t.material == 17));
        let lo = tris.iter().flat_map(|t| t.vertices).fold(max, Vec3::min);
        let hi = tris.iter().flat_map(|t| t.vertices).fold(min, Vec3::max);
        assert_eq!((lo, hi), (min, max));
        assert!(outward(
            &tris.iter().map(|t| t.vertices).collect::<Vec<_>>(),
            (min + max) * 0.5
        ));
    }

    #[test]
    fn bound_cylinder_uses_y_axis_and_authored_radius() {
        let c = Vec3::new(10., 20., 30.);
        let b = raw_bound(
            13,
            Vec3::new(-2., -3., -2.),
            Vec3::new(2., 3., 2.),
            c,
            100.,
            0.04,
        );
        let tris = decode_bound(&b);
        assert_eq!(tris.len(), SIDES * 4);
        for p in tris.iter().flat_map(|t| t.vertices) {
            assert!((p.y - 17.).abs() < 1e-5 || (p.y - 23.).abs() < 1e-5);
            let radial = ((p.x - c.x).powi(2) + (p.z - c.z).powi(2)).sqrt();
            assert!(radial < 1e-5 || (radial - 2.).abs() < 1e-5);
        }
    }

    #[test]
    fn bound_round_shapes_use_their_distinct_header_contracts() {
        let c = Vec3::new(10., 20., 30.);
        // Sphere: radius, not margin; every mesh vertex lies on it.
        let sphere = decode_bound(&raw_bound(0, Vec3::ZERO, Vec3::ZERO, c, 2., 0.25));
        assert_eq!(sphere.len(), 32);
        assert!(
            sphere
                .iter()
                .flat_map(|t| t.vertices)
                .all(|v| ((v - c).length() - 2.).abs() < 1e-5)
        );
        // Capsule: local Y, half-height SphereRadius and radial Margin.
        let capsule = decode_bound(&raw_bound(1, Vec3::ZERO, Vec3::ZERO, c, 3., 0.5));
        assert!(!capsule.is_empty());
        let min_y = capsule
            .iter()
            .flat_map(|t| t.vertices)
            .map(|v| v.y)
            .fold(f32::INFINITY, f32::min);
        let max_y = capsule
            .iter()
            .flat_map(|t| t.vertices)
            .map(|v| v.y)
            .fold(f32::NEG_INFINITY, f32::max);
        assert_eq!((min_y, max_y), (17., 23.));
        // Disc: local X thickness Margin, radial SphereRadius.
        let disc = decode_bound(&raw_bound(12, Vec3::ZERO, Vec3::ZERO, c, 2., 0.25));
        assert_eq!(disc.len(), SIDES * 4);
        for v in disc.iter().flat_map(|t| t.vertices) {
            assert!((v.x - 9.75).abs() < 1e-5 || (v.x - 10.25).abs() < 1e-5);
            let r = ((v.y - c.y).powi(2) + (v.z - c.z).powi(2)).sqrt();
            assert!(r < 1e-5 || (r - 2.).abs() < 1e-5);
        }
        let invalid = raw_bound(0, Vec3::ZERO, Vec3::ZERO, c, f32::NAN, 0.);
        assert!(decode_bound(&invalid).is_empty());
    }

    #[test]
    fn authored_bound_respects_composite_filter_and_transform() {
        let b = raw_bound(3, Vec3::ZERO, Vec3::new(1., 1., 1.), Vec3::ZERO, 1., 0.);
        let r = ResReader {
            system: &b,
            graphics: &[],
        };
        let xf = BoundTransform {
            columns: [Vec3::Y, -Vec3::X, Vec3::Z, Vec3::new(5., 6., 7.)],
        };
        let mut out = Vec::new();
        let mut counts = Counts::default();
        let options = Options {
            primitives: true,
            all_flags: false,
        };
        walk(
            &r,
            SYSTEM_BASE,
            0,
            &mut vec![xf],
            (0, PED),
            options,
            &mut out,
            &mut counts,
        );
        assert_eq!(out.len(), 12);
        let lo = out
            .iter()
            .flat_map(|t| t.vertices)
            .fold(Vec3::new(99., 99., 99.), Vec3::min);
        let hi = out
            .iter()
            .flat_map(|t| t.vertices)
            .fold(Vec3::new(-99., -99., -99.), Vec3::max);
        assert_eq!(lo, Vec3::new(4., 6., 7.));
        assert_eq!(hi, Vec3::new(5., 7., 8.));
        out.clear();
        walk(
            &r,
            SYSTEM_BASE,
            0,
            &mut vec![xf],
            (FOLIAGE, PED),
            options,
            &mut out,
            &mut counts,
        );
        assert!(out.is_empty());
        assert_eq!(counts.dropped_polygons, 1);
    }

    fn outward(tris: &[[Vec3; 3]], c: Vec3) -> bool {
        tris.iter().all(|t| {
            let n = (t[1] - t[0]).cross(t[2] - t[0]);
            let mid = (t[0] + t[1] + t[2]) * (1.0 / 3.0);
            n.dot(mid - c) > 0.0
        })
    }

    #[test]
    fn box_corners_rebuild_the_unit_box_outward() {
        let v = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
        ];
        let mut t = Vec::new();
        box_from_corners(v, &mut t);
        assert_eq!(t.len(), 12);
        let c = Vec3::new(0.5, 0.5, 0.5);
        assert!(outward(&t, c));
        for tri in &t {
            for p in tri {
                for k in [p.x, p.y, p.z] {
                    assert!(k.abs() < 1e-6 || (k - 1.0).abs() < 1e-6, "{p:?}");
                }
            }
        }
        // Mirrored corner order still winds outward.
        let mut m = Vec::new();
        box_from_corners([v[0], v[2], v[1], v[3]], &mut m);
        assert!(outward(&m, c));
    }

    #[test]
    fn round_primitives_wind_outward() {
        let (a, b) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 3.0));
        let mid = Vec3::new(0.0, 0.0, 1.5);
        let mut t = Vec::new();
        cylinder(a, b, 0.2, &mut t);
        assert!(outward(&t, mid));
        let mut t = Vec::new();
        capsule(a, b, 0.2, &mut t);
        assert!(outward(&t, mid));
        let mut t = Vec::new();
        sphere(mid, 0.5, &mut t);
        assert_eq!(t.len(), 32);
        assert!(outward(&t, mid));
        // A tilted pole too.
        let mut t = Vec::new();
        cylinder(a, Vec3::new(1.0, 2.0, 0.5), 0.1, &mut t);
        assert!(outward(&t, Vec3::new(0.5, 1.0, 0.25)));
    }
}

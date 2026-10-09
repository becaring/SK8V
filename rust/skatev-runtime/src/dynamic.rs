//! Moving GTA entities (vehicles, peds, props) as Skate dynamic geometry.
//! The host supplies entity transforms and explicit box fallbacks each frame.
//! Owned model bounds become outward-facing triangles in Skate space and go through
//! Skate's own contact code (overlay patch 0003). Static Los Santos collision
//! never comes through here.
use crate::coords;
use bevy_math::{Quat, Vec3};
use std::{collections::HashMap, path::{Path, PathBuf}};

#[derive(Clone, Debug)]
pub struct HostBody {
    pub model: u32,
    pub position: Vec3,
    pub axes: [Vec3; 3],
    pub fallback: HostBox,
    pub allow_box: bool,
    /// Host-sampled world velocities (GTA space, m/s and rad/s); `None` when
    /// the sample is not finite or implausible (no contact exchange).
    pub velocity: Option<(Vec3, Vec3)>,
    /// Vehicle `bumper_r` bone (GTA world), when the host found one.
    pub grab_point: Option<Vec3>,
}

impl HostBody {
    pub fn from_abi(b: &crate::SvDynamicBody) -> Option<Self> {
        let v = |p: crate::SvVec3| Vec3::new(p.x, p.y, p.z);
        let position = v(b.position);
        let axes = [v(b.right), v(b.forward), v(b.up)];
        let determinant = axes[0].cross(axes[1]).dot(axes[2]);
        if !position.is_finite() || axes.iter().any(|a| !a.is_finite() || a.length_squared() > 10000.)
            || !determinant.is_finite() || determinant.abs() < 1e-6 { return None; }
        let q = b.fallback.rotation;
        let rotation = Quat::from_xyzw(q.x, q.y, q.z, q.w);
        let fallback = HostBox { tag: b.fallback.tag & !DYNAMIC_TAG,
            center: v(b.fallback.center), rotation: rotation.normalize(), half_extents: v(b.fallback.half_extents) };
        if b.flags & 1 != 0 && !usable(&fallback) { return None; }
        let velocity = Some((v(b.linear_velocity), v(b.angular_velocity)))
            .filter(|(l, a)| l.is_finite() && a.is_finite() && l.length() < 500.0 && a.length() < 200.0);
        let grab_point = (b.grab_flags & 1 != 0)
            .then(|| v(b.grab_point))
            .filter(|p| p.is_finite());
        Some(Self { model: b.model_hash, position, axes, fallback, allow_box: b.flags & 1 != 0, velocity, grab_point })
    }

    pub fn transform(&self, t: &svwc::Tri) -> ([[f32; 3]; 3], u32) {
        let mut points = t.v.map(|p| coords::to_skate(self.position + self.axes[0]*p[0] + self.axes[1]*p[1] + self.axes[2]*p[2]).to_array());
        if self.axes[0].cross(self.axes[1]).dot(self.axes[2]) < 0. { points.swap(1, 2); }
        (points, self.fallback.tag | DYNAMIC_TAG)
    }
}

/// Exact extracted model bounds, loaded once per model on the simulation worker.
/// There is no filesystem I/O on the ScriptHookV fiber. Failed models are logged
/// once; a fallback is used only when the host explicitly allows its small box.
pub struct Templates {
    directory: PathBuf,
    /// Each model's bound triangles with their packed Skate surface.
    models: HashMap<u32, Option<Vec<(svwc::Tri, u32)>>>,
    /// Model space bound box of each loaded template (min, max).
    extents: HashMap<u32, (Vec3, Vec3)>,
    masses: HashMap<u32, Mass>,
    /// Metres the skitch grab line sits behind the rearmost bound.
    pub standoff: f32,
}

/// Grab line height above the body's underside for a vehicle with no
/// `bumper_r` bone (Skate raises a line under its own reach floor anyway).
pub const NO_BONE_HEIGHT: f32 = 0.6;

/// Default skitch standoff (`SkitchStandoff` in the INI overrides it).
pub const SKITCH_STANDOFF: f32 = 0.3;

/// Authored mass data (`<cache>.prop-masses.txt`): fragment masses and
/// vehicles from handling.meta.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mass {
    pub kg: f32,
    pub inertia_multiplier: Vec3,
    pub centre_of_mass_offset: Vec3,
}

/// Parses `<cache>.prop-masses.txt` lines: model kg mult.xyz com.xyz.
pub fn parse_masses(text: &str) -> HashMap<u32, Mass> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('#').next()?.split_whitespace().collect();
            if f.len() != 8 { return None; }
            let model = u32::from_str_radix(f[0], 16).ok()?;
            let n: Vec<f32> = f[1..].iter().map(|x| x.parse().ok()).collect::<Option<_>>()?;
            (n.iter().all(|x| x.is_finite()) && n[0] > 0.0).then(|| (model, Mass {
                kg: n[0],
                inertia_multiplier: Vec3::new(n[1], n[2], n[3]),
                centre_of_mass_offset: Vec3::new(n[4], n[5], n[6]),
            }))
        })
        .collect()
}

impl Templates {
    pub fn new(world_cache: &Path) -> Self {
        let masses = std::fs::read_to_string(world_cache.with_extension("prop-masses.txt"))
            .map(|t| parse_masses(&t))
            .unwrap_or_default();
        Self { directory: world_cache.with_extension("prop-models"), models: HashMap::new(), extents: HashMap::new(), masses, standoff: SKITCH_STANDOFF }
    }

    /// Contact-exchange proxies (Skate space) for the moving vehicles and
    /// objects among `bodies`. Mass is the authored mass when known, else the
    /// entity is kinematic for the solve. Inertia is that of the entity's
    /// bound box (template, else the host box), scaled by handling.meta's
    /// inertia multiplier for vehicles. Peds are excluded (separate path).
    pub fn proxies(&self, bodies: &[HostBody]) -> Vec<skate_host::bridge::HostBody> {
        let mut out = Vec::new();
        for b in bodies {
            let Some((linear, angular)) = b.velocity else { continue };
            let kind = (b.fallback.tag >> 28) & 0x7;
            if kind == 1 {
                continue;
            }
            let scale = Vec3::new(b.axes[0].length(), b.axes[1].length(), b.axes[2].length());
            if !(scale.min_element() > 1e-5) { continue; }
            let unit = [b.axes[0] / scale.x, b.axes[1] / scale.y, b.axes[2] / scale.z];
            let mass = self.masses.get(&b.model).copied();
            let com_offset = mass.map_or(Vec3::ZERO, |m| m.centre_of_mass_offset);
            let (centre, half) = match self.extents.get(&b.model) {
                Some(&(lo, hi)) => {
                    let c = (lo + hi) * 0.5 + com_offset;
                    (b.position + b.axes[0] * c.x + b.axes[1] * c.y + b.axes[2] * c.z, (hi - lo) * 0.5 * scale)
                }
                None => {
                    let r = b.fallback.rotation;
                    (b.fallback.center + r * com_offset, b.fallback.half_extents)
                }
            };
            let (inverse_mass, inverse_inertia) = match mass {
                Some(m) => {
                    let h2 = half * half;
                    let i = Vec3::new(h2.y + h2.z, h2.x + h2.z, h2.x + h2.y) * (m.kg / 3.0) * m.inertia_multiplier;
                    let inv = |v: f32| if v > 1e-6 { 1.0 / v } else { 0.0 };
                    (1.0 / m.kg, [inv(i.x), inv(i.y), inv(i.z)])
                }
                None => (0.0, [0.0; 3]),
            };
            let basis = bevy_math::Mat3::from_cols(
                coords::to_skate(unit[0]), coords::to_skate(unit[1]), coords::to_skate(unit[2]));
            let q = Quat::from_mat3(&basis).normalize();
            out.push(skate_host::bridge::HostBody {
                tag: b.fallback.tag | DYNAMIC_TAG,
                position: coords::to_skate(centre).to_array(),
                orientation: q.to_array(),
                linear_velocity: coords::to_skate(linear).to_array(),
                angular_velocity: coords::to_skate(angular).to_array(),
                inverse_mass,
                inverse_inertia,
                collision_group: if kind == 0 { skate_host::bridge::VEHICLE_GROUP } else { 0 },
                volumes: Vec::new(),
            });
        }
        out
    }
    /// Skitch grab lines: every vehicle gets one. A two-point line across the
    /// body's full width (lo.x..hi.x) at the `bumper_r` bone's model-space
    /// height (vans, buses, planes, bikes have none: `NO_BONE_HEIGHT` above
    /// the body's underside), at the rearmost extent (lo.y) of the exact
    /// template, or of the host's model box when the model has no template:
    /// the bone sits inside the bumper, and on cars
    /// whose body reaches further back (SUV spare wheels, tow bars) a line at
    /// the bone held the rider inside the body. Approach is the outward rear
    /// normal, model (0, -1, 0). A line under Skate's reach is raised by the
    /// session (`Session::set_grab_lines`).
    /// The line is an invisible wall `standoff` behind that rearmost extent:
    /// at the bodywork the rider's hands met the car's contacts, GTA's
    /// physics on the car and the limb colliders at once.
    /// Points and approach stay in model space; the frame carries the
    /// entity's live axes and position into Skate space.
    /// Model space bound box (min, max) of a loaded template.
    pub fn extent(&self, model: u32) -> Option<(Vec3, Vec3)> {
        self.extents.get(&model).copied()
    }

    pub fn grab_lines(&self, bodies: &[HostBody]) -> Vec<skate_host::bridge::GrabLine> {
        let mut out = Vec::new();
        for b in bodies {
            if (b.fallback.tag >> 28) & 0x7 != 0 { continue; }
            let m = bevy_math::Mat3::from_cols(b.axes[0], b.axes[1], b.axes[2]);
            if !(m.determinant().abs() > 1e-6) { continue; }
            let (lo, hi) = match self.extents.get(&b.model) {
                Some(&e) => e,
                None => {
                    // No template: the host's model box, in model space.
                    let scale = Vec3::new(b.axes[0].length(), b.axes[1].length(), b.axes[2].length());
                    let c = m.inverse() * (b.fallback.center - b.position);
                    let half = b.fallback.half_extents / scale;
                    (c - half, c + half)
                }
            };
            let z = match b.grab_point {
                Some(world) => (m.inverse() * (world - b.position)).z,
                None => lo.z + NO_BONE_HEIGHT.min((hi.z - lo.z) * 0.5),
            };
            if !(lo.is_finite() && hi.is_finite() && z.is_finite()) { continue; }
            let row = |v: Vec3| { let s = coords::to_skate(v); [s.x, s.y, s.z, 0.0] };
            let (linear, _) = b.velocity.unwrap_or((Vec3::ZERO, Vec3::ZERO));
            out.push(skate_host::bridge::GrabLine {
                tag: b.fallback.tag | DYNAMIC_TAG,
                frame: [row(b.axes[0]), row(b.axes[1]), row(b.axes[2]), row(b.position)],
                velocity: row(linear),
                points: [[lo.x, lo.y - self.standoff, z, 0.0], [hi.x, lo.y - self.standoff, z, 0.0]],
                approach: [0.0, -1.0, 0.0, 0.0],
            });
        }
        out
    }
    pub fn triangles(&mut self, bodies: &[HostBody]) -> (Vec<([[f32; 3]; 3], u32, u32)>, Vec<String>) {
        let mut triangles = Vec::new();
        let mut messages = Vec::new();
        for body in bodies {
            if body.model != 0 && !self.models.contains_key(&body.model) {
                let path = self.directory.join(format!("{:08x}.svwc", body.model));
                let loaded = svwc::Cache::open(&path).and_then(|mut cache| {
                    if cache.record_count > 1_000_000 { return Err(std::io::Error::other("model record limit exceeded")); }
                    let tris = cache.read_all()?;
                    if tris.is_empty() { return Err(std::io::Error::other("empty model bound")); }
                    Ok(tris)
                });
                match loaded {
                    Ok(tris) => {
                        messages.push(format!("dynamic model {:08x}: {} exact bound triangles{}", body.model, tris.len(),
                            self.masses.get(&body.model).map_or(String::new(), |m| format!(", {:.1} kg", m.kg))));
                        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                        for t in &tris {
                            for p in t.v {
                                lo = lo.min(Vec3::from_array(p));
                                hi = hi.max(Vec3::from_array(p));
                            }
                        }
                        self.extents.insert(body.model, (lo, hi));
                        let surfaced = tris.into_iter().map(|t| { let surface = crate::materials::surface(t.material).packed(); (t, surface) }).collect();
                        self.models.insert(body.model, Some(surfaced));
                    }
                    Err(e) => {
                        messages.push(format!("dynamic model {:08x}: exact bound unavailable ({e}); box fallback {}", body.model, body.allow_box));
                        self.models.insert(body.model, None);
                    }
                }
            }
            if let Some(Some(model)) = self.models.get(&body.model) {
                triangles.extend(model.iter().map(|(t, surface)| { let (p, id) = body.transform(t); (p, id, *surface) }));
            } else if body.allow_box {
                triangles.extend(box_triangles(&body.fallback).into_iter().map(|(p, id)| (p, id, 0)));
            }
        }
        (triangles, messages)
    }
}

/// High bit marks host entities in triangle tags; static map tags never set it.
pub const DYNAMIC_TAG: u32 = 0x8000_0000;

#[derive(Clone, Copy, Debug)]
pub struct HostBox {
    pub tag: u32,
    pub center: Vec3,
    pub rotation: Quat,
    pub half_extents: Vec3,
}

/// The 12 outward triangles of a box, in Skate space, tagged.
pub fn box_triangles(b: &HostBox) -> [([[f32; 3]; 3], u32); 12] {
    let corner = |sx: f32, sy: f32, sz: f32| {
        let local = Vec3::new(
            sx * b.half_extents.x,
            sy * b.half_extents.y,
            sz * b.half_extents.z,
        );
        coords::to_skate(b.center + b.rotation * local).to_array()
    };
    // Quads listed counterclockwise seen from outside (GTA space).
    let quads: [[(f32, f32, f32); 4]; 6] = [
        [(-1., -1., 1.), (1., -1., 1.), (1., 1., 1.), (-1., 1., 1.)], // +z
        [
            (-1., -1., -1.),
            (-1., 1., -1.),
            (1., 1., -1.),
            (1., -1., -1.),
        ], // -z
        [(1., -1., -1.), (1., 1., -1.), (1., 1., 1.), (1., -1., 1.)], // +x
        [
            (-1., -1., -1.),
            (-1., -1., 1.),
            (-1., 1., 1.),
            (-1., 1., -1.),
        ], // -x
        [(-1., 1., -1.), (-1., 1., 1.), (1., 1., 1.), (1., 1., -1.)], // +y
        [
            (-1., -1., -1.),
            (1., -1., -1.),
            (1., -1., 1.),
            (-1., -1., 1.),
        ], // -y
    ];
    let tag = b.tag | DYNAMIC_TAG;
    let mut out = [([[0.0; 3]; 3], tag); 12];
    for (i, q) in quads.iter().enumerate() {
        let p = q.map(|(x, y, z)| corner(x, y, z));
        out[i * 2] = ([p[0], p[1], p[2]], tag);
        out[i * 2 + 1] = ([p[0], p[2], p[3]], tag);
    }
    out
}

/// Boxes that are finite, non-degenerate and not absurdly large.
pub fn usable(b: &HostBox) -> bool {
    b.center.is_finite()
        && b.rotation.is_finite()
        && b.half_extents.is_finite()
        && b.half_extents.min_element() > 0.01
        && b.half_extents.max_element() < 40.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_exact_mesh_wins_over_box_and_follows_entity_transform() {
        let root = std::env::temp_dir().join(format!("skatev-model-test-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let directory = root.join("world.prop-models");
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("00000001.svwc");
        let authored = svwc::Tri { v: [[2.,0.,0.], [2.,1.,0.], [2.,0.,1.]], material: 3 };
        svwc::write(&mut std::fs::File::create(&file).unwrap(), &[authored], 64.).unwrap();
        let fallback = HostBox { tag: 19, center: Vec3::ZERO, rotation: Quat::IDENTITY, half_extents: Vec3::ONE };
        let mut body = HostBody { model: 1, position: Vec3::ZERO, axes: [Vec3::X, Vec3::Y, Vec3::Z], fallback, allow_box: true, velocity: None, grab_point: None };
        let mut models = Templates::new(&root.join("world.svwc"));
        let (first, messages) = models.triangles(&[body.clone()]);
        assert_eq!(first[0].2, crate::materials::surface(3).packed());
        assert_eq!(first[0].1, 19 | DYNAMIC_TAG);
        assert_eq!(first.len(), 1, "a model's opening must not be filled by its fallback box");
        assert_eq!(messages.len(), 1);
        std::fs::remove_file(&file).unwrap();
        body.position = Vec3::new(10., 20., 30.);
        let (moved, messages) = models.triangles(&[body.clone()]);
        assert_eq!(moved.len(), 1, "model data is cached independently of moving instances");
        assert!(messages.is_empty());
        assert_eq!(moved[0].0[0], [12., 30., -20.]);
        body.model = 2;
        body.allow_box = false;
        assert!(models.triangles(&[body.clone()]).0.is_empty());
        body.allow_box = true;
        assert_eq!(models.triangles(&[body]).0.len(), 12, "missing models need explicit fallback permission");
        assert!(models.triangles(&[]).0.is_empty(), "removed entities leave no geometry");
        std::fs::remove_dir(&directory).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn exact_bounds_preserve_gaps_and_full_affine_transform() {
        let fallback = HostBox { tag: 0x21234567, center: Vec3::ZERO, rotation: Quat::IDENTITY, half_extents: Vec3::ONE };
        let body = HostBody { model: 1, position: Vec3::new(10.,20.,30.), axes: [Vec3::Y*2., -Vec3::X, Vec3::Z], fallback, allow_box: true, velocity: None, grab_point: None };
        let t = svwc::Tri { v: [[2.,0.,0.], [2.,1.,0.], [2.,0.,1.]], material: 3 };
        let (points, tag) = body.transform(&t);
        assert_eq!(tag, 0xA1234567);
        assert_eq!(points, [[10.,30.,-24.], [9.,30.,-24.], [10.,31.,-24.]]);
        // Geometry stays the authored side face; no bounding-box face fills the opening at local x=0.
        assert!(points.iter().all(|p| p[2] == -24.));
    }

    #[test]
    fn grab_line_spans_template_width_at_bone_height_behind_the_body_and_skips_non_vehicles() {
        let mut t = Templates { directory: PathBuf::new(), models: HashMap::new(), extents: HashMap::new(), masses: HashMap::new(), standoff: 0.0 };
        t.extents.insert(7, (Vec3::new(-1.0, -2.5, -0.6), Vec3::new(1.0, 2.5, 0.8)));
        let fallback = HostBox { tag: 0x0000_0042, center: Vec3::ZERO, rotation: Quat::IDENTITY, half_extents: Vec3::ONE };
        let body = HostBody { model: 7, position: Vec3::new(10.0, 20.0, 1.0), axes: [Vec3::X, Vec3::Y, Vec3::Z], fallback,
            allow_box: false, velocity: Some((Vec3::new(0.0, 5.0, 0.0), Vec3::ZERO)), grab_point: Some(Vec3::new(10.2, 17.6, 0.7)) };
        let lines = t.grab_lines(&[body.clone()]);
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert_eq!(l.tag, 0x42 | DYNAMIC_TAG);
        assert_eq!(l.points[0][0], -1.0);
        assert_eq!(l.points[1][0], 1.0);
        // The bone is 0.1 m inside the body's rear (-2.5): the line is on the rear.
        assert!((l.points[0][1] + 2.5).abs() < 1e-5 && (l.points[0][2] + 0.3).abs() < 1e-5);
        assert_eq!(l.approach, [0.0, -1.0, 0.0, 0.0]);
        // A standoff moves the wall straight back, clear of the bodywork.
        t.standoff = 0.3;
        assert!((t.grab_lines(&[body.clone()])[0].points[1][1] + 2.8).abs() < 1e-5);
        t.standoff = 0.0;
        assert_eq!(l.frame[3], [10.0, 1.0, -20.0, 0.0]);
        assert_eq!(l.velocity, [0.0, 0.0, -5.0, 0.0]);
        let mut ped = body.clone();
        ped.fallback.tag = 1 << 28 | 0x42;
        assert!(t.grab_lines(&[ped]).is_empty());
        // No bone: a line at NO_BONE_HEIGHT above the underside (-0.6 + 0.6).
        let mut no_bone = body.clone();
        no_bone.grab_point = None;
        let l = &t.grab_lines(&[no_bone.clone()])[0];
        assert!((l.points[0][2] - 0.0).abs() < 1e-5 && (l.points[0][1] + 2.5).abs() < 1e-5);
        // No template either: the host box (here 1 x 2.5 x 0.7 half extents, centred 0.1 up) stands in.
        let mut boxed = no_bone;
        boxed.model = 99;
        boxed.fallback.center = Vec3::new(10.0, 20.0, 1.1);
        boxed.fallback.half_extents = Vec3::new(1.0, 2.5, 0.7);
        let l = &t.grab_lines(&[boxed])[0];
        assert!((l.points[0][0] + 1.0).abs() < 1e-5 && (l.points[1][0] - 1.0).abs() < 1e-5);
        assert!((l.points[0][1] + 2.5).abs() < 1e-5 && l.points[0][2].abs() < 1e-5);
    }

    #[test]
    fn dynamic_body_abi_is_stable_and_rejects_singular_transforms() {
        assert_eq!(std::mem::size_of::<crate::SvDynamicBody>(), 144);
        assert!(HostBody::from_abi(&crate::SvDynamicBody::default()).is_none());
        let axis = |x, y, z| crate::SvVec3 { x, y, z };
        let mut b = crate::SvDynamicBody {
            size: 144,
            right: axis(1.0, 0.0, 0.0),
            forward: axis(0.0, 1.0, 0.0),
            up: axis(0.0, 0.0, 1.0),
            linear_velocity: axis(3.0, 0.0, 0.0),
            angular_velocity: axis(0.0, 0.0, 1.0),
            ..Default::default()
        };
        b.fallback.rotation.w = 1.0;
        b.grab_point = axis(0.0, -2.0, 0.5);
        assert_eq!(HostBody::from_abi(&b).unwrap().velocity, Some((Vec3::new(3.0, 0.0, 0.0), Vec3::Z)));
        assert_eq!(HostBody::from_abi(&b).unwrap().grab_point, None, "no grab line without its flag");
        b.grab_flags = 1;
        assert_eq!(HostBody::from_abi(&b).unwrap().grab_point, Some(Vec3::new(0.0, -2.0, 0.5)));
        b.linear_velocity.x = f32::NAN;
        assert_eq!(HostBody::from_abi(&b).unwrap().velocity, None, "non-finite samples are dropped");
    }

    #[test]
    fn masses_parse_and_proxies_use_authored_mass_and_box_inertia() {
        let masses = parse_masses("# header
eb70965f 1200.000 1.2 1.2 1.3 0 0.05 -0.25  # blista BLISTA
bad line
000b75b9 2.000 1 1 1 0 0 0
");
        assert_eq!(masses.len(), 2);
        assert_eq!(masses[&0xeb70965f].kg, 1200.0);
        let mut t = Templates { directory: PathBuf::new(), models: HashMap::new(), extents: HashMap::new(), masses, standoff: 0.0 };
        let fallback = HostBox { tag: 9, center: Vec3::new(5.0, 6.0, 1.0), rotation: Quat::IDENTITY, half_extents: Vec3::new(1.0, 2.0, 0.5) };
        let body = HostBody { model: 0xeb70965f, position: Vec3::new(5.0, 6.0, 0.3), axes: [Vec3::X, Vec3::Y, Vec3::Z], fallback,
            allow_box: true, velocity: Some((Vec3::new(0.0, 8.0, 0.0), Vec3::new(0.0, 0.0, 0.5))), grab_point: None };
        let p = t.proxies(&[body.clone()]);
        assert_eq!(p.len(), 1);
        assert!((p[0].inverse_mass - 1.0 / 1200.0).abs() < 1e-9);
        // Box inertia about local x: m/3 (hy^2 + hz^2) * multiplier.
        let ix = 1200.0 / 3.0 * (4.0 + 0.25) * 1.2;
        assert!((1.0 / p[0].inverse_inertia[0] - ix).abs() < 1e-2 * ix);
        // GTA velocity (0, 8, 0) is Skate (0, 0, -8); spin about GTA z is about Skate y.
        assert_eq!(p[0].linear_velocity, [0.0, 0.0, -8.0]);
        assert_eq!(p[0].angular_velocity, [0.0, 0.5, 0.0]);
        // Peds never become proxies; an unknown mass is kinematic.
        let mut ped = body.clone();
        ped.fallback.tag = 1 << 28 | 3;
        assert!(t.proxies(&[ped]).is_empty());
        let mut unknown = body.clone();
        unknown.model = 42;
        assert_eq!(t.proxies(&[unknown])[0].inverse_mass, 0.0);
        t.masses.clear();
    }

    #[test]
    fn box_faces_point_outward_in_skate_space() {
        let b = HostBox {
            tag: 7,
            center: Vec3::new(10.0, -20.0, 3.0),
            rotation: Quat::from_rotation_z(0.6),
            half_extents: Vec3::new(2.0, 1.0, 0.75),
        };
        let center = coords::to_skate(b.center);
        for (t, tag) in box_triangles(&b) {
            assert_eq!(tag, 7 | DYNAMIC_TAG);
            let [p0, p1, p2] = t.map(Vec3::from_array);
            let n = (p1 - p0).cross(p2 - p0);
            let mid = (p0 + p1 + p2) / 3.0;
            assert!(n.dot(mid - center) > 0.0, "inward face {t:?}");
        }
    }

    #[test]
    fn top_face_is_at_box_top() {
        let b = HostBox {
            tag: 1,
            center: Vec3::new(0.0, 0.0, 1.0),
            rotation: Quat::IDENTITY,
            half_extents: Vec3::splat(0.5),
        };
        let tris = box_triangles(&b);
        // GTA z = 1.5 -> skate y = 1.5 for the first (+z) face.
        assert!(tris[0].0.iter().all(|p| (p[1] - 1.5).abs() < 1e-6));
    }

    #[test]
    fn rejects_degenerate_and_huge_boxes() {
        let ok = HostBox {
            tag: 0,
            center: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            half_extents: Vec3::ONE,
        };
        assert!(usable(&ok));
        assert!(!usable(&HostBox {
            half_extents: Vec3::new(1.0, 1.0, 0.0),
            ..ok
        }));
        assert!(!usable(&HostBox {
            half_extents: Vec3::splat(100.0),
            ..ok
        }));
        assert!(!usable(&HostBox {
            center: Vec3::NAN,
            ..ok
        }));
    }
}

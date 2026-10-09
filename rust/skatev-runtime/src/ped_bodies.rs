//! GTA peds in the contact exchange, with their own hitboxes (a ped
//! was a 0.6 m box of infinite mass, so the board caught
//! its corners, Michael was tripped as by a wall and peds only reacted to a
//! scripted push).
//!
//! The host sends every part of each nearby ped's ragdoll compound (the
//! capsules and boxes of its own fragment type, posed by its live skeleton,
//! with their authored masses). Here each ped becomes one rigid proxy body
//! (a standing ped moves as one; GTA's ragdoll articulates the response):
//! mass = the parts' masses, centre of mass and inertia from the parts as
//! solid shapes (parallel-axis), principal axes by Jacobi rotation. Its
//! volumes are the parts themselves, so Michael's limbs and the board meet
//! the ped's limbs. It reports Skate's character collision group.
use crate::coords;
use bevy_math::{Mat3, Quat, Vec3};
use skate_core::{
    math::{Basis3, Vector3},
    physics::{collision::Sphere, world_contact::ContactPrimitive},
};

/// Capsule along its local Y (RAGE phBoundCapsule).
pub const KIND_CAPSULE: u32 = 1;
/// Box with rounded edges of radius `radius` (RAGE phBoundBox margin).
pub const KIND_BOX: u32 = 3;

/// One posed ragdoll part, GTA world space.
#[derive(Clone, Copy, Debug)]
pub struct PedPart {
    pub tag: u32,
    pub kind: u32,
    /// Part axes (unit) and centre.
    pub axes: [Vec3; 3],
    pub centre: Vec3,
    /// Capsule: x = z = radius, y = half the segment length. Box: outer half
    /// extents (the rounded edge lies inside them).
    pub half_extents: Vec3,
    pub radius: f32,
    pub mass: f32,
    pub linear_velocity: Vec3,
    pub angular_velocity: Vec3,
}

impl PedPart {
    pub fn valid(&self) -> bool {
        let finite = |v: Vec3| v.is_finite();
        self.axes.iter().all(|a| finite(*a) && (a.length() - 1.0).abs() < 0.01)
            && finite(self.centre)
            && finite(self.half_extents)
            && self.half_extents.min_element() >= 0.0
            && self.radius.is_finite()
            && self.radius > 0.0
            && self.mass.is_finite()
            && self.mass > 0.0
            && finite(self.linear_velocity)
            && finite(self.angular_velocity)
            && (self.kind == KIND_CAPSULE || self.kind == KIND_BOX)
    }

    /// Principal moments of the part as a solid of its mass, about its own
    /// centre in its own axes.
    fn moments(&self) -> Vec3 {
        let m = self.mass;
        match self.kind {
            KIND_CAPSULE => {
                // Cylinder (segment 2h) plus two hemispheres, mass by volume.
                let (r, h) = (self.radius, self.half_extents.y);
                let cylinder = std::f32::consts::PI * r * r * 2.0 * h;
                let ball = 4.0 / 3.0 * std::f32::consts::PI * r * r * r;
                let (mc, ms) = (m * cylinder / (cylinder + ball), m * ball / (cylinder + ball));
                // Hemisphere centroid 3r/8 from its flat face.
                let axial = mc * r * r / 2.0 + ms * 2.0 / 5.0 * r * r;
                let transverse = mc * (r * r / 4.0 + (2.0 * h) * (2.0 * h) / 12.0)
                    + ms * (2.0 / 5.0 * r * r + h * h + 3.0 / 4.0 * h * r);
                Vec3::new(transverse, axial, transverse)
            }
            _ => {
                let e = self.half_extents * 2.0;
                Vec3::new(
                    m * (e.y * e.y + e.z * e.z) / 12.0,
                    m * (e.x * e.x + e.z * e.z) / 12.0,
                    m * (e.x * e.x + e.y * e.y) / 12.0,
                )
            }
        }
    }

    /// The part as a Skate collision primitive (Skate space).
    fn primitive(&self) -> ContactPrimitive {
        let v = |p: Vec3| {
            let s = coords::to_skate(p);
            Vector3::new(s.x, s.y, s.z)
        };
        match self.kind {
            KIND_CAPSULE if self.half_extents.y <= 0.0 => ContactPrimitive::Sphere(Sphere {
                center: v(self.centre),
                radius: self.radius,
            }),
            KIND_CAPSULE => ContactPrimitive::Capsule {
                center: v(self.centre),
                axis: v(self.axes[1]),
                half_length: self.half_extents.y,
                radius: self.radius,
            },
            _ => {
                let inner = (self.half_extents - Vec3::splat(self.radius)).max(Vec3::ZERO);
                let c = self.axes.map(coords::to_skate);
                ContactPrimitive::RoundedBox {
                    center: v(self.centre),
                    basis: Basis3 { columns: c.map(|a| a.to_array()) },
                    half_extents: Vector3::new(inner.x, inner.y, inner.z),
                    radius: self.radius,
                }
            }
        }
    }
}

/// Eigen-decomposition of a symmetric 3x3 matrix (cyclic Jacobi):
/// eigenvalues and the matrix whose columns are the eigenvectors.
fn jacobi(mut a: [[f32; 3]; 3]) -> (Vec3, Mat3) {
    let mut v = Mat3::IDENTITY.to_cols_array_2d();
    for _ in 0..32 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1e-9 {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-12 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            for k in 0..3 {
                let (akp, akq) = (a[k][p], a[k][q]);
                a[k][p] = c * akp - s * akq;
                a[k][q] = s * akp + c * akq;
            }
            for k in 0..3 {
                let (apk, aqk) = (a[p][k], a[q][k]);
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for row in &mut v {
                let (vp, vq) = (row[p], row[q]);
                row[p] = c * vp - s * vq;
                row[q] = s * vp + c * vq;
            }
        }
    }
    // v holds eigenvectors as columns in row-major storage: transpose.
    let columns = Mat3::from_cols_array_2d(&v).transpose();
    (Vec3::new(a[0][0], a[1][1], a[2][2]), columns)
}

/// One rigid proxy per ped (by tag), in tag order. Parts that fail
/// validation drop their whole ped (no partial body).
pub fn bodies(parts: &[PedPart]) -> Vec<skate_host::bridge::HostBody> {
    let mut tags: Vec<u32> = parts.iter().map(|p| p.tag).collect();
    tags.sort_unstable();
    tags.dedup();
    let mut out = Vec::with_capacity(tags.len());
    for tag in tags {
        let ped: Vec<&PedPart> = parts.iter().filter(|p| p.tag == tag).collect();
        if ped.iter().any(|p| !p.valid()) {
            continue;
        }
        let mass: f32 = ped.iter().map(|p| p.mass).sum();
        let com = ped.iter().fold(Vec3::ZERO, |s, p| s + p.centre * p.mass) / mass;
        // Inertia about the centre of mass, GTA world axes.
        let mut inertia = Mat3::ZERO;
        for p in &ped {
            let axes = Mat3::from_cols(p.axes[0], p.axes[1], p.axes[2]);
            let own = axes * Mat3::from_diagonal(p.moments()) * axes.transpose();
            let r = p.centre - com;
            let parallel = Mat3::from_diagonal(Vec3::splat(r.dot(r))) - Mat3::from_cols(r * r.x, r * r.y, r * r.z);
            inertia += own + parallel * p.mass;
        }
        // Skate space, then principal axes.
        let to = coords::basis().transpose();
        let skate_inertia = to * inertia * to.transpose();
        let (moments, mut axes) = jacobi(skate_inertia.transpose().to_cols_array_2d());
        if axes.determinant() < 0.0 {
            axes.z_axis = -axes.z_axis;
        }
        let inverse = |m: f32| if m > 1e-6 { 1.0 / m } else { 0.0 };
        let v = ped[0].linear_velocity;
        let w = ped[0].angular_velocity;
        out.push(skate_host::bridge::HostBody {
            tag: tag | crate::dynamic::DYNAMIC_TAG,
            position: coords::to_skate(com).to_array(),
            orientation: Quat::from_mat3(&axes).normalize().to_array(),
            linear_velocity: coords::to_skate(v).to_array(),
            angular_velocity: coords::to_skate(w).to_array(),
            inverse_mass: 1.0 / mass,
            inverse_inertia: [inverse(moments.x), inverse(moments.y), inverse(moments.z)],
            collision_group: skate_host::bridge::CHARACTER_GROUP,
            volumes: ped.iter().map(|p| p.primitive()).collect(),
        });
    }
    out
}

/// The angular velocity change (Skate space, rad/s) an impulse `j` at
/// `point` (Skate space) gives the rigid body `b`: I^-1 (r x j) about its
/// centre of mass, with the body's own posed inertia.
pub fn angular_change(b: &skate_host::bridge::HostBody, point: [f32; 3], j: [f32; 3]) -> Vec3 {
    let r = Mat3::from_quat(Quat::from_array(b.orientation));
    let inverse = r * Mat3::from_diagonal(Vec3::from_array(b.inverse_inertia)) * r.transpose();
    let arm = Vec3::from_array(point) - Vec3::from_array(b.position);
    inverse * arm.cross(Vec3::from_array(j))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(tag: u32, kind: u32, centre: Vec3, half: Vec3, radius: f32, mass: f32) -> PedPart {
        PedPart {
            tag,
            kind,
            axes: [Vec3::X, Vec3::Y, Vec3::Z],
            centre,
            half_extents: half,
            radius,
            mass,
            linear_velocity: Vec3::new(1.0, 0.0, 0.0),
            angular_velocity: Vec3::ZERO,
        }
    }

    #[test]
    fn one_body_per_ped_with_summed_mass_and_centre() {
        let parts = [
            part(7, KIND_CAPSULE, Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.1, 0.2, 0.1), 0.1, 10.0),
            part(7, KIND_BOX, Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.2, 0.1, 0.3), 0.02, 30.0),
            part(9, KIND_CAPSULE, Vec3::new(5.0, 0.0, 1.0), Vec3::new(0.1, 0.2, 0.1), 0.1, 5.0),
        ];
        let b = bodies(&parts);
        assert_eq!(b.len(), 2);
        assert!((1.0 / b[0].inverse_mass - 40.0).abs() < 1e-3);
        // COM at z = (10*1 + 30*2)/40 = 1.75 (GTA) -> Skate y.
        assert!((b[0].position[1] - 1.75).abs() < 1e-4);
        assert_eq!(b[0].volumes.len(), 2);
        assert_eq!(b[0].collision_group, skate_host::bridge::CHARACTER_GROUP);
        assert_eq!(b[0].linear_velocity, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn principal_inertia_reproduces_the_tensor() {
        // Two point-like boxes off-axis: a tilted principal frame.
        let parts = [
            part(1, KIND_BOX, Vec3::new(0.3, 0.2, 1.0), Vec3::splat(0.05), 0.01, 20.0),
            part(1, KIND_BOX, Vec3::new(-0.3, -0.2, 1.6), Vec3::splat(0.05), 0.01, 20.0),
        ];
        let b = &bodies(&parts)[0];
        let q = Quat::from_array(b.orientation);
        let r = Mat3::from_quat(q);
        let inv = Mat3::from_diagonal(Vec3::from_array(b.inverse_inertia));
        let world_inverse = r * inv * r.transpose();
        // A unit spin about the line joining the parts has (almost) no
        // inertia; across it the inertia is m r^2 summed.
        let d = coords::to_skate(Vec3::new(0.6, 0.4, -0.6)).normalize();
        let across = d.cross(Vec3::Y).normalize();
        let i_across = 1.0 / across.dot(world_inverse * across);
        let half = Vec3::new(0.6, 0.4, -0.6).length() / 2.0;
        assert!((i_across - (2.0 * 20.0 * half * half + 2.0 * 20.0 * 0.1 * 0.1 / 6.0)).abs() < 0.05, "{i_across}");
    }

    #[test]
    fn an_off_centre_hit_spins_the_ped_about_its_centre_of_mass() {
        // A 1.8 m standing column of two parts; the hit at shin height.
        let parts = [
            part(1, KIND_BOX, Vec3::new(0.0, 0.0, 0.45), Vec3::new(0.15, 0.1, 0.45), 0.01, 30.0),
            part(1, KIND_BOX, Vec3::new(0.0, 0.0, 1.35), Vec3::new(0.2, 0.1, 0.45), 0.01, 50.0),
        ];
        let b = &bodies(&parts)[0];
        // GTA: push along +x at z 0.2 m (centre of mass at 1.0125 m).
        let point = coords::to_skate(Vec3::new(0.0, 0.0, 0.2)).to_array();
        let j = coords::to_skate(Vec3::new(100.0, 0.0, 0.0)).to_array();
        let w = coords::from_skate(angular_change(b, point, j));
        // r x J = (0, 0, -0.8125) x (100, 0, 0) = (0, -81.25, 0): the feet go
        // forward, the head back (rotation about -y).
        let com_z: f32 = (30.0 * 0.45 + 50.0 * 1.35) / 80.0;
        let i_y = 30.0 * (0.3f32.powi(2) + 0.9f32.powi(2)) / 12.0 + 30.0 * (0.45 - com_z).powi(2)
            + 50.0 * (0.4f32.powi(2) + 0.9f32.powi(2)) / 12.0 + 50.0 * (1.35 - com_z).powi(2);
        let expected = -100.0 * (com_z - 0.2) / i_y;
        assert!(w.x.abs() < 1e-3 && w.z.abs() < 1e-3, "{w}");
        assert!((w.y - expected).abs() < 1e-3 * expected.abs(), "{w} {expected}");
        // Through the centre of mass: no spin.
        let point = coords::to_skate(Vec3::new(0.0, 0.0, com_z)).to_array();
        assert!(coords::from_skate(angular_change(b, point, j)).length() < 1e-4);
    }

    #[test]
    fn capsule_axis_follows_the_part_y_axis() {
        let mut p = part(3, KIND_CAPSULE, Vec3::ZERO, Vec3::new(0.08, 0.25, 0.08), 0.08, 4.0);
        p.axes = [Vec3::Y, -Vec3::X, Vec3::Z];
        let ContactPrimitive::Capsule { axis, half_length, radius, .. } = p.primitive() else { panic!() };
        assert_eq!((axis.x, axis.y, axis.z), (-1.0, 0.0, 0.0));
        assert_eq!((half_length, radius), (0.25, 0.08));
    }

    #[test]
    fn an_invalid_part_drops_its_ped() {
        let mut bad = part(4, KIND_BOX, Vec3::ZERO, Vec3::splat(0.1), 0.01, 1.0);
        bad.mass = f32::NAN;
        let good = part(4, KIND_BOX, Vec3::ONE, Vec3::splat(0.1), 0.01, 1.0);
        assert!(bodies(&[good, bad]).is_empty());
    }
}

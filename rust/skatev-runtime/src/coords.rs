//! GTA <-> Skate frames.
//!
//! GTA V: right-handed, Z up, metres. Heading 0 faces +Y (north) and grows
//! counterclockwise (90 faces -X).
//! Skate: right-handed, Y up, metres. The mashup feeds MW2 (Z up) through the
//! same axis map, `(x, y, z) -> (x, z, -y)` (crates/render_anim/src/skate/
//! collision.rs); GTA needs no unit scale because both sides are metres.
//! A skater spawned with `Quat::from_rotation_y(h)` faces skate +Z rotated by h.
//!
//! Winding: the SVWC census over Los Santos map bounds found 97.3% of
//! near-horizontal faces +Z under (b - a) x (c - a), i.e. counterclockwise as
//! Skate expects. The axis map is a proper rotation, so winding is preserved.
use bevy_math::{Mat3, Mat4, Quat, Vec3};

pub fn to_skate(p: Vec3) -> Vec3 {
    Vec3::new(p.x, p.z, -p.y)
}

pub fn from_skate(p: Vec3) -> Vec3 {
    Vec3::new(p.x, -p.z, p.y)
}

/// Maps Skate-space vectors to GTA-space vectors.
pub fn basis() -> Mat3 {
    Mat3::from_cols(Vec3::X, Vec3::Z, -Vec3::Y)
}

/// Skate spawn heading (radians about skate +Y) for a GTA heading in degrees.
pub fn skate_heading(gta_heading_degrees: f32) -> f32 {
    (gta_heading_degrees + 180.0).to_radians()
}

/// GTA heading (degrees, 0..360) of a GTA-space direction's XY projection.
pub fn gta_heading(forward: Vec3) -> f32 {
    let h = (-forward.x).atan2(forward.y).to_degrees();
    if h < 0.0 { h + 360.0 } else { h }
}

/// A Skate-space rotation expressed in GTA space.
pub fn rotation_from_skate(q: Quat) -> Quat {
    let b = Quat::from_mat3(&basis());
    (b * q * b.inverse()).normalize()
}

/// Rigid transform (rotation + translation) from Skate to GTA space.
pub fn transform_from_skate(m: Mat4) -> (Vec3, Quat) {
    let (_, r, t) = m.to_scale_rotation_translation();
    (from_skate(t), rotation_from_skate(r))
}

/// GTA script-camera rotation (degrees; x pitch, y roll, z yaw, order 2) for a
/// GTA-space view direction. Roll is left at 0 until verified in game.
pub fn gta_camera_rotation(forward: Vec3) -> Vec3 {
    let f = forward.normalize_or_zero();
    let pitch = f.z.clamp(-1.0, 1.0).asin().to_degrees();
    Vec3::new(pitch, 0.0, gta_heading(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-4
    }

    #[test]
    fn position_roundtrip_and_up_axis() {
        let p = Vec3::new(-1234.5, 678.25, 31.0);
        assert!(close(from_skate(to_skate(p)), p));
        assert!(close(to_skate(Vec3::Z), Vec3::Y), "GTA up is Skate up");
        assert!(close(to_skate(Vec3::Y), -Vec3::Z), "GTA north is Skate -Z");
        assert!(close(basis() * Vec3::Y, Vec3::Z));
        assert!(
            (basis().determinant() - 1.0).abs() < 1e-6,
            "proper rotation keeps winding"
        );
    }

    #[test]
    fn counterclockwise_gta_floor_stays_upward_in_skate() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X, Vec3::Y);
        assert!((b - a).cross(c - a).z > 0.0);
        let (sa, sb, sc) = (to_skate(a), to_skate(b), to_skate(c));
        assert!((sb - sa).cross(sc - sa).y > 0.0);
    }

    #[test]
    fn spawn_heading_faces_the_gta_heading() {
        for h in [0.0f32, 45.0, 90.0, 180.0, 270.0, 359.0] {
            let skate_forward = Quat::from_rotation_y(skate_heading(h)) * Vec3::Z;
            let gta_forward = basis() * skate_forward;
            let expected = Vec3::new(-h.to_radians().sin(), h.to_radians().cos(), 0.0);
            assert!(close(gta_forward, expected), "heading {h}");
            assert!(
                (gta_heading(gta_forward) - h).abs() < 1e-3
                    || (gta_heading(gta_forward) - h).abs() > 359.9
            );
        }
    }

    #[test]
    fn rotations_and_transforms_convert_consistently() {
        let q = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.2);
        let v = Vec3::new(0.3, -0.4, 0.8);
        assert!(close(
            rotation_from_skate(q) * (basis() * v),
            basis() * (q * v)
        ));
        let m = Mat4::from_rotation_translation(q, Vec3::new(1.0, 2.0, 3.0));
        let (t, r) = transform_from_skate(m);
        assert!(close(t, Vec3::new(1.0, -3.0, 2.0)));
        assert!(r.angle_between(rotation_from_skate(q)) < 1e-4);
    }

    #[test]
    fn camera_rotation_matches_gta_convention() {
        let r = gta_camera_rotation(Vec3::new(0.0, 1.0, 0.0));
        assert!(close(r, Vec3::ZERO));
        let r = gta_camera_rotation(Vec3::new(-1.0, 0.0, 1.0));
        assert!((r.x - 45.0).abs() < 1e-3 && (r.z - 90.0).abs() < 1e-3);
    }
}

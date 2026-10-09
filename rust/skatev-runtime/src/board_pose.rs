//! Native drawable presentation contract. These are authored Skate joints,
//! never inferred from velocity or a host physics object.
use bevy_math::{Mat4, Vec4};

pub const NAMES: [&str; 7] = [
    "SKATEBOARD_ROOT", "TRUCK_FRONT", "TRUCK_BACK", "LEFT_WHEELFRONT",
    "LEFT_WHEELBACK", "RIGHT_WHEELFRONT", "RIGHT_WHEELBACK",
];

pub fn world(names: &[String], bones: &[Mat4], root: Mat4) -> Option<[Mat4; 7]> {
    let blender = Mat4::from_cols(Vec4::X, -Vec4::Z, Vec4::Y, Vec4::W);
    let c = Mat4::from_mat3(crate::coords::basis());
    let mut result = [Mat4::IDENTITY; 7];
    for (out, name) in result.iter_mut().zip(NAMES) {
        let index = names.iter().position(|n| n == name)?;
        *out = c * root * *bones.get(index)? * blender * c.transpose();
        if !out.is_finite() { return None; }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec3;

    #[test]
    fn native_vertex_matches_existing_skin_coordinate_chain() {
        let names = NAMES.map(str::to_string);
        let bones = std::array::from_fn::<_, 7, _>(|i| {
            Mat4::from_rotation_y(i as f32 * 0.21)
                * Mat4::from_translation(Vec3::new(i as f32 * 0.1, 0.2, -0.3))
        });
        let root = Mat4::from_translation(Vec3::new(-1300., 24., 1400.))
            * Mat4::from_rotation_y(1.2);
        let actual = world(&names, &bones, root).unwrap();
        let c = Mat4::from_mat3(crate::coords::basis());
        let blender = Mat4::from_cols(Vec4::X, -Vec4::Z, Vec4::Y, Vec4::W);
        let inverse_bind = Mat4::from_rotation_z(0.8)
            * Mat4::from_translation(Vec3::new(0.1, 0.03, -0.1));
        let vertex = Vec3::new(0.08, -0.04, 0.3);
        for i in 0..7 {
            let expected = crate::coords::from_skate(
                (root * bones[i] * blender * inverse_bind).transform_point3(vertex));
            let converted = (actual[i] * c * inverse_bind * c.transpose())
                .transform_point3(c.transform_point3(vertex));
            assert!(expected.distance(converted) < 0.0003);
        }
    }

    #[test]
    fn missing_or_invalid_bones_fail_closed() {
        let names = NAMES.map(str::to_string);
        let mut bones = [Mat4::IDENTITY; 7];
        assert!(world(&names[..6], &bones, Mat4::IDENTITY).is_none());
        assert!(world(&names, &bones[..6], Mat4::IDENTITY).is_none());
        bones[4].w_axis.x = f32::NAN;
        assert!(world(&names, &bones, Mat4::IDENTITY).is_none());
    }
}

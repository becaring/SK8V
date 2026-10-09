//! Hall of Meat x-ray: Skate 3's `HOMSkaterPresEntity`, ported as working PC
//! math. During a bail the retail game draws the skeleton pieces of the
//! Marquee recipe `dem_bones_hom` (converted by tools/prepare-hom-xray.py to
//! `private/hom-xray/skeleton.glb`) for every bone the WipeoutScorer has
//! damaged to level 2 or more, skinned to the skater's pose and tinted by
//! level, through the `defaulthom` shader.
//!
//! Decoded behaviour (TU3):
//! - 82786E70, per tick while the bail runs: scorer bone `b` at level >= 2 is
//!   shown; its piece takes the `hall_of_meat_skeleton` "broken" colour, or
//!   the "severe" colour from level 4. Bones are visited in order, so a piece
//!   shared by two bones keeps the colour of the later one. Outside a bail
//!   nothing is shown.
//! - 82785C40: the bone -> piece table (`BONE_PIECES`).
//! - 82785778: the colours and `i_colorParams` come from
//!   `hall_of_meat_skeleton`; 82785528 binds them as `i_boneClr` per piece.
//! - `defaulthom_defaultPS`: see `shade`. `m_params` are the material's
//!   (`character.default_hom`, the `material_character` collection).
//!
//! Every number used comes from the user's converted collections; nothing
//! here is a retail value. The host draws the result on top of the frame.
use crate::hom::{BONES, be_f32, hex_bytes};
use crate::skin::SkinMesh;
use bevy_math::{Mat4, Vec3};
use std::path::Path;

/// Retail piece per WipeoutScorer bone (82785C40's table, in scorer order).
pub const BONE_PIECES: [&str; BONES] = [
    "Bones_Skull",
    "Bones_Neck",
    "Bones_Hand_Left",
    "Bones_Forearm_Left",
    "Bones_Bicep_Left",
    "Bones_Bicep_Left",
    "Bones_Hand_Right",
    "Bones_Forearm_Right",
    "Bones_Bicep_Right",
    "Bones_Bicep_Right",
    "Bones_Rib_Cage",
    "Bones_Rib_Cage",
    "Bones_Lower_Spine",
    "Bones_Lower_Spine",
    "Bones_Toes_Left",
    "Bones_Ankle_Left",
    "Bones_Calf_Left",
    "Bones_Thigh_Left",
    "Bones_Toes_Right",
    "Bones_Ankle_Right",
    "Bones_Calf_Right",
    "Bones_Thigh_Right",
    "Bones_Hips",
    "Bones_Skull",
    "Bones_Hips",
];

/// Lowest scorer level that shows a bone, and the level that turns it severe.
pub const BROKEN_LEVEL: i32 = 2;
const SEVERE_LEVEL: i32 = 4;

mod field {
    pub const SKELETON_KEY: &str = "hall_of_meat_skeleton";
    pub const BROKEN: &str = "Hash_F1C8F879829F12A0";
    pub const SEVERE: &str = "Hash_9C9AB482CDC3CF48";
    pub const COLOR_SCALE: &str = "Hash_B04194DE78D7063A";
    pub const COLOR_FLOOR: &str = "Hash_A69CD3B0559B2C5E";
    /// `material_character` / `default_hom`.
    pub const MATERIAL_CLASS: &str = "Hash_B6B8997286F19549";
    pub const MATERIAL_KEY: &str = "Hash_AD7C73E68EBA5F29";
    pub const M_PARAMS: &str = "Hash_F483C92D9C0B4131";
}

/// Diagnostic: SKATEV_XRAY_ALL (read once) shows every piece (alignment checks).
pub fn show_all() -> bool {
    static ALL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ALL.get_or_init(|| std::env::var_os("SKATEV_XRAY_ALL").is_some())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// `i_boneClr` for levels 2..3 and from level 4.
    pub broken: [f32; 3],
    pub severe: [f32; 3],
    /// `i_colorParams` x (texture contrast) and y (texture floor).
    pub color_params: [f32; 2],
    /// `m_params[0..2]` as the shader reads them (c1, c2).
    pub m_params: [[f32; 4]; 2],
}

fn floats(bytes: &[u8], n: usize) -> Result<Vec<f32>, String> {
    (0..n).map(|i| be_f32(bytes, 4 * i)).collect()
}

impl Tuning {
    pub fn from_collections(json: &serde_json::Value) -> Result<Self, String> {
        let rows = json["collections"].as_array().ok_or("skater collections without a collection list")?;
        let skeleton = rows
            .iter()
            .find(|c| c["key"] == field::SKELETON_KEY)
            .ok_or("hall_of_meat_skeleton missing from skater collections")?;
        let material = rows
            .iter()
            .find(|c| c["class"] == field::MATERIAL_CLASS && c["key"] == field::MATERIAL_KEY)
            .ok_or("default_hom material missing from skater collections")?;
        let data = |row: &serde_json::Value, name: &str| -> Result<Vec<u8>, String> {
            hex_bytes(row["fields"][name]["data"].as_str().ok_or_else(|| format!("x-ray field {name} missing"))?)
        };
        let vec3 = |name: &str| -> Result<[f32; 3], String> {
            let v = floats(&data(skeleton, name)?, 3)?;
            Ok([v[0], v[1], v[2]])
        };
        let params = material["fields"][field::M_PARAMS]["array"]["items"]
            .as_array()
            .ok_or("default_hom m_params missing")?;
        let mut m_params = [[0.0; 4]; 2];
        for (k, row) in m_params.iter_mut().enumerate() {
            let item = params.get(k).and_then(|v| v.as_str()).ok_or("default_hom m_params too short")?;
            let v = floats(&hex_bytes(item)?, 4)?;
            *row = [v[0], v[1], v[2], v[3]];
        }
        Ok(Self {
            broken: vec3(field::BROKEN)?,
            severe: vec3(field::SEVERE)?,
            color_params: [
                be_f32(&data(skeleton, field::COLOR_SCALE)?, 0)?,
                be_f32(&data(skeleton, field::COLOR_FLOOR)?, 0)?,
            ],
            m_params,
        })
    }
}

/// 82786E70: the colour each shown piece takes this tick (later bones win).
pub fn piece_colours(levels: &[i32; BONES], t: &Tuning) -> Vec<(&'static str, [f32; 3])> {
    let mut out: Vec<(&'static str, [f32; 3])> = Vec::new();
    for (b, &level) in levels.iter().enumerate() {
        if level < BROKEN_LEVEL {
            continue;
        }
        let colour = if level >= SEVERE_LEVEL { t.severe } else { t.broken };
        match out.iter_mut().find(|(p, _)| *p == BONE_PIECES[b]) {
            Some(entry) => entry.1 = colour,
            None => out.push((BONE_PIECES[b], colour)),
        }
    }
    out
}

/// The shader's output encoding (its last four instructions): a soft
/// shoulder into the half-range target, square-root encoded.
fn encode(x: f32) -> f32 {
    let shoulder = (x * 0.25 + 0.75).max(1.0) - (1.0 - x).clamp(0.0, 1.0).powi(2);
    (shoulder * 0.5).max(0.0).sqrt()
}

/// `defaulthom_defaultPS` for one surface point: `base` is the diffuse
/// texel, `normal` and `to_eye` unit vectors. Fog is not applied (GTA has its
/// own); with no fog the shader's fog alpha is 1, which the shader doubles.
pub fn shade(base: [f32; 3], normal: Vec3, to_eye: Vec3, bone: [f32; 3], t: &Tuning) -> [f32; 4] {
    let ndv = normal.dot(to_eye).clamp(0.0, 1.0);
    let rim = (1.0 - ndv).max(0.0).powf(t.m_params[1][3]);
    let fog_alpha = 1.0;
    let gain = 2.0 * fog_alpha;
    let contrast = (1.0 - rim) * t.color_params[0];
    let rgb: [f32; 3] = std::array::from_fn(|i| {
        let tex = base[i] * base[i];
        let tinted = tex.max(t.color_params[1]) * bone[i];
        let lit = tinted + tex * contrast * (tex - tinted);
        encode(lit * gain * t.m_params[0][1])
    });
    [rgb[0], rgb[1], rgb[2], encode(gain)]
}

/// One output vertex (triangle list), GTA world space, straight alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: Vec3,
    pub rgba: [f32; 4],
}

pub struct Xray {
    mesh: SkinMesh,
    /// Retail piece name per triangle (`None`: not an x-ray piece).
    piece: Vec<Option<&'static str>>,
    /// Pose bone per skin joint, built on first draw.
    map: Option<Vec<Option<usize>>>,
    pub tuning: Tuning,
}

/// The GTA ped wearing Skate's pose: its model and bone world matrices
/// (GTA space), which the x-ray follows.
pub struct Overlay<'a> {
    pub ped: &'a crate::ped::PedModel,
    pub world: &'a [Mat4],
}

impl Xray {
    pub fn load(data_root: &Path, collections: &serde_json::Value) -> Result<Self, String> {
        let tuning = Tuning::from_collections(collections)?;
        let path = data_root.join("private/hom-xray/skeleton.glb");
        let (mesh, materials, names) = SkinMesh::load_with_materials(&path, 0)?;
        let piece: Vec<Option<&'static str>> = materials
            .iter()
            .map(|&m| {
                let name = names.get(m)?.strip_prefix("Retail_")?;
                BONE_PIECES.iter().copied().find(|p| *p == name)
            })
            .collect();
        let missing: Vec<&str> =
            BONE_PIECES.iter().copied().filter(|p| !piece.contains(&Some(*p))).collect();
        if !missing.is_empty() {
            return Err(format!("{}: pieces missing: {missing:?}", path.display()));
        }
        Ok(Self { mesh, piece, map: None, tuning })
    }

    pub fn triangle_count(&self) -> usize {
        self.mesh.triangle_count()
    }

    /// Joint world matrices for this mesh from Skate's pose (the presenter's
    /// rule: world = root * bone * Blender basis). With `overlay` (the GTA
    /// ped wearing Skate's pose) each piece rides the GTA bone instead: the
    /// ped keeps its own proportions, so Skate's joints are a few cm off its
    /// limbs (the x-ray hand floated off the hand).
    fn joint_world(&mut self, names: &[String], bones: &[Mat4], root: Mat4, overlay: Option<&Overlay>) -> Vec<Option<Mat4>> {
        let map = self.map.get_or_insert_with(|| {
            self.mesh.joint_names.iter().map(|n| names.iter().position(|x| x == n)).collect()
        });
        let blender = Mat4::from_cols(
            bevy_math::Vec4::X,
            -bevy_math::Vec4::Z,
            bevy_math::Vec4::Y,
            bevy_math::Vec4::W,
        );
        let skate: Vec<Option<Mat4>> = map.iter().map(|i| i.and_then(|i| bones.get(i).map(|b| root * *b * blender))).collect();
        let Some(o) = overlay else { return skate };
        let c = Mat4::from_mat3(crate::coords::basis());
        let bind = |n: &str| self.mesh.inverse_bind_for(n).map(|ib| ib.inverse());
        let at = |n: &str| bind(n).map(|m| crate::coords::from_skate(m.w_axis.truncate()));
        let gta_at = |b: usize| o.ped.bind().get(b).map(|m| m.w_axis.truncate());
        self.mesh.joint_names.iter().zip(skate).map(|(name, fallback)| {
            // Pieces on a reparented copy or the upper neck ride their joint.
            let joint = name.strip_suffix("_REPARENTED").unwrap_or(if name == "NECK1" { "NECK" } else { name });
            let follow = || -> Option<Mat4> {
                let bone = o.ped.bone_for_source(joint)?;
                // Bind anchor: the joint onto the GTA joint; the pelvis by its
                // hip joints (GTA's pelvis joint sits 7 cm above them).
                let shift = if joint == "HIPS" {
                    let gta = |n: &str| o.ped.bone_for_source(n).and_then(gta_at);
                    (gta("LEFTUPLEG")? + gta("RIGHTUPLEG")?) / 2.0 - (at("LEFTUPLEG")? + at("RIGHTUPLEG")?) / 2.0
                } else {
                    gta_at(bone)? - at(joint)?
                };
                let skin = o.ped.skin_delta(o.world, bone)? * Mat4::from_translation(shift);
                Some(c.inverse() * skin * c * bind(name)?)
            };
            follow().or(fallback)
        }).collect()
    }

    /// This tick's x-ray: the shown pieces skinned to the pose and shaded for
    /// `eye` (Skate space). Empty when no bone is broken.
    pub fn draw(
        &mut self,
        levels: &[i32; BONES],
        names: &[String],
        bones: &[Mat4],
        root: Mat4,
        eye: Vec3,
        overlay: Option<&Overlay>,
    ) -> Vec<Vertex> {
        let all = [BROKEN_LEVEL; BONES];
        let levels = if show_all() { &all } else { levels };
        let colours = piece_colours(levels, &self.tuning);
        if colours.is_empty() {
            return Vec::new();
        }
        let world = self.joint_world(names, bones, root, overlay);
        let positions = self.mesh.posed_vertices(&world);
        let tris: Vec<([u32; 3], [f32; 3])> = self
            .mesh
            .triangles()
            .iter()
            .zip(&self.piece)
            .filter_map(|(t, p)| {
                let p = (*p)?;
                colours.iter().find(|(name, _)| *name == p).map(|(_, c)| (*t, *c))
            })
            .collect();
        // Area-weighted smooth normals of the shown pieces (pieces share no vertices).
        let mut normals = vec![Vec3::ZERO; positions.len()];
        for (t, _) in &tris {
            let [a, b, c] = t.map(|i| positions[i as usize]);
            let face = (b - a).cross(c - a);
            if face.is_finite() {
                for &i in t {
                    normals[i as usize] += face;
                }
            }
        }
        let mut out = Vec::with_capacity(tris.len() * 3);
        for (t, colour) in tris {
            for i in t {
                let p = positions[i as usize];
                if !p.is_finite() {
                    continue;
                }
                let n = normals[i as usize].normalize_or_zero();
                let v = (eye - p).normalize_or_zero();
                out.push(Vertex {
                    position: crate::coords::from_skate(p),
                    rgba: shade(self.mesh.vertex_color(i as usize), n, v, colour, &self.tuning),
                });
            }
        }
        out.truncate(out.len() / 3 * 3);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuning() -> Tuning {
        Tuning {
            broken: [0.6, 0.6, 0.6],
            severe: [0.9, 0.1, 0.0],
            color_params: [0.4, 0.2],
            m_params: [[0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 3.0]],
        }
    }

    #[test]
    fn only_broken_bones_show_and_later_bones_win() {
        let t = tuning();
        let mut levels = [0; BONES];
        assert!(piece_colours(&levels, &t).is_empty());
        levels[1] = 1; // neck bruised, not broken
        levels[0] = 5; // skull severe...
        levels[23] = 2; // ...then the second skull bone, broken: it wins
        levels[16] = 3;
        let c = piece_colours(&levels, &t);
        assert_eq!(c, vec![("Bones_Skull", t.broken), ("Bones_Calf_Left", t.broken)]);
        levels[23] = 0;
        assert_eq!(piece_colours(&levels, &t)[0], ("Bones_Skull", t.severe));
    }

    #[test]
    fn shading_follows_the_bone_colour_and_rim() {
        let t = tuning();
        let n = Vec3::Z;
        let red = shade([0.9; 3], n, Vec3::Z, t.severe, &t);
        assert!(red[0] > red[1] && red[1] >= red[2], "{red:?}");
        // Facing the eye vs grazing: the rim removes the texture contrast term.
        let facing = shade([0.9; 3], n, Vec3::Z, t.broken, &t);
        let grazing = shade([0.9; 3], n, Vec3::X, t.broken, &t);
        assert!(facing[0] > grazing[0], "{facing:?} {grazing:?}");
        for v in facing.iter().chain(&grazing) {
            assert!((0.0..=1.0).contains(v));
        }
        assert_eq!(facing[3], grazing[3]);
    }

    #[test]
    fn encode_is_monotonic_and_bounded() {
        let mut last = -1.0;
        for i in 0..=40 {
            let e = encode(i as f32 / 10.0);
            assert!(e >= last && e.is_finite());
            last = e;
        }
        assert_eq!(encode(0.0), 0.0);
    }
}

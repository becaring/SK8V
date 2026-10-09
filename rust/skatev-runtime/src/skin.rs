//! Skate's own skater + board mesh (the converted `private/skater.glb`),
//! skinned every published tick with Skate's live world-space bone pose.
//! This is presentation only: the pose is Skate's, nothing here feeds back
//! into the simulation. GTA draws the result as shaded triangles.
//!
//! The GLB reader is deliberately minimal: one skinned mesh, float positions
//! and normals, u8/u16 joints, float/normalised weights, PNG base colours.
use bevy_math::{Mat4, Vec2, Vec3};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
struct Vertex {
    position: Vec3,
    joints: [u16; 4],
    weights: [f32; 4],
    color: [f32; 3],
    alpha: f32,
}

/// One shaded output triangle in Skate space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadedTri {
    pub points: [Vec3; 3],
    /// Flat (face) colour.
    pub rgba: [u8; 4],
    /// Smooth (Gouraud) colour per corner: albedo x smooth-normal lighting.
    pub vertex_rgba: [[u8; 4]; 3],
    /// Texture coordinates per corner (GTA character only).
    pub uv: [[f32; 2]; 3],
    /// Lighting alone per corner (multiplies a real texture).
    pub light: [u8; 3],
    /// Character texture slot, `NO_TEXTURE` for untextured triangles.
    pub texture: u16,
    /// Alpha-cutout triangle (hair cards, lace): drawn only textured.
    pub cutout: bool,
    /// Authored deck/trucks/wheels; omitted by the host when the native drawable is visible.
    pub board: bool,
}

pub const NO_TEXTURE: u16 = u16::MAX;

pub fn light_byte(lit: f32) -> u8 {
    (lit.clamp(0.0, 1.0) * 255.0) as u8
}

/// Key light term shared by both mesh sources.
pub fn light_term(n: Vec3, light: Vec3) -> f32 {
    0.6 + 0.5 * n.dot(light).max(0.0)
}

/// Per-vertex lighting from area-weighted smooth normals of the posed mesh.
pub fn smooth_light(positions: &[Vec3], tris: &[[u32; 3]], light: Vec3) -> Vec<f32> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for t in tris {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        let face = (b - a).cross(c - a);
        if face.is_finite() {
            for &i in t {
                normals[i as usize] += face;
            }
        }
    }
    normals
        .iter()
        .map(|n| light_term(n.normalize_or_zero(), light))
        .collect()
}

/// The two strongest influences, with the top weight quantised: vertices
/// merged by simplification must share this, so a merged vertex can never be
/// stretched between bones (the in-game "spikes").
pub(crate) fn signature(b: &[u16; 4], w: &[f32; 4]) -> (u16, u16, u8) {
    let mut order = [0usize, 1, 2, 3];
    order.sort_by(|&x, &y| w[y].total_cmp(&w[x]));
    let second = if w[order[1]] > 0.05 {
        b[order[1]]
    } else {
        u16::MAX
    };
    (b[order[0]], second, (w[order[0]] * 4.0).round() as u8)
}

/// `p` skinned by up to four influences (`skin`: per bone, world * inverse bind).
pub(crate) fn skin_point(skin: &[Mat4], p: Vec3, b: &[u16; 4], w: &[f32; 4]) -> Vec3 {
    let mut out = Vec3::ZERO;
    for k in 0..4 {
        if w[k] > 0.0 {
            out += skin[(b[k] as usize).min(skin.len() - 1)].transform_point3(p) * w[k];
        }
    }
    out
}

/// More than half of a vertex's weight hangs from bone `root` or below it.
pub(crate) fn mostly_under(parents: &[Option<usize>], root: usize, b: &[u16; 4], w: &[f32; 4]) -> bool {
    let under = |mut i: usize| loop {
        if i == root {
            return true;
        }
        let Some(Some(parent)) = parents.get(i) else { return false };
        i = *parent;
    };
    (0..4).filter(|&k| under(b[k] as usize)).map(|k| w[k]).sum::<f32>() > 0.5
}

/// Vertex clustering for simplification: each vertex's cluster (numbered in
/// order of first member) and the first member of each cluster.
pub(crate) fn cluster<K: std::hash::Hash + Eq>(count: usize, key: impl Fn(usize) -> K) -> (Vec<u32>, Vec<usize>) {
    let mut index: HashMap<K, u32> = HashMap::new();
    let mut first = Vec::new();
    let map = (0..count)
        .map(|i| {
            *index.entry(key(i)).or_insert_with(|| {
                first.push(i);
                (first.len() - 1) as u32
            })
        })
        .collect();
    (map, first)
}

/// Triangles under a clustering, dropping degenerate and repeated ones:
/// (original triangle index, remapped triangle).
pub(crate) fn remap(tris: &[[u32; 3]], map: &[u32]) -> Vec<(usize, [u32; 3])> {
    let mut seen = std::collections::HashSet::new();
    tris.iter()
        .enumerate()
        .map(|(k, t)| (k, t.map(|v| map[v as usize])))
        .filter(|(_, t)| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .filter(|(_, t)| {
            let mut key = *t;
            key.sort();
            seen.insert(key)
        })
        .collect()
}

pub fn to_rgba(c: [f32; 3], lit: f32) -> [u8; 4] {
    [
        ((c[0] * lit).clamp(0.0, 1.0) * 255.0) as u8,
        ((c[1] * lit).clamp(0.0, 1.0) * 255.0) as u8,
        ((c[2] * lit).clamp(0.0, 1.0) * 255.0) as u8,
        255,
    ]
}

pub struct SkinMesh {
    vertices: Vec<Vertex>,
    triangles: Vec<[u32; 3]>,
    /// Per triangle: part of the skateboard (deck, trucks, wheels) rather than the skater.
    board: Vec<bool>,
    /// Joint name per skin joint, in skin order.
    pub joint_names: Vec<String>,
    /// Nearest ancestor skin joint per joint (for joints Skate does not pose).
    joint_parents: Vec<Option<usize>>,
    inverse_bind: Vec<Mat4>,
    /// Original (unsimplified) shoe vertices, grouped by foot ancestry.
    foot_vertices: [Vec<Vertex>; 2],
}

struct Glb {
    json: serde_json::Value,
    bin: Vec<u8>,
}

fn u32_at(b: &[u8], i: usize) -> Result<u32, String> {
    b.get(i..i + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| "truncated GLB".to_string())
}

impl Glb {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.get(0..4) != Some(b"glTF") || u32_at(bytes, 4)? != 2 {
            return Err("not a glTF 2.0 binary".into());
        }
        let json_len = u32_at(bytes, 12)? as usize;
        if u32_at(bytes, 16)? != 0x4E4F534A {
            return Err("GLB first chunk is not JSON".into());
        }
        let json_end = 20 + json_len;
        let json: serde_json::Value =
            serde_json::from_slice(bytes.get(20..json_end).ok_or("truncated GLB JSON")?)
                .map_err(|e| format!("GLB JSON: {e}"))?;
        let bin_len = u32_at(bytes, json_end)? as usize;
        if u32_at(bytes, json_end + 4)? != 0x004E4942 {
            return Err("GLB second chunk is not BIN".into());
        }
        let bin = bytes
            .get(json_end + 8..json_end + 8 + bin_len)
            .ok_or("truncated GLB BIN")?
            .to_vec();
        Ok(Self { json, bin })
    }

    fn view(&self, index: usize) -> Result<(&[u8], usize), String> {
        let v = &self.json["bufferViews"][index];
        let offset = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let length = v["byteLength"]
            .as_u64()
            .ok_or("bufferView without length")? as usize;
        let stride = v["byteStride"].as_u64().unwrap_or(0) as usize;
        Ok((
            self.bin
                .get(offset..offset + length)
                .ok_or("bufferView out of range")?,
            stride,
        ))
    }

    /// Accessor as rows of f32 (normalised integers become 0..1, others cast).
    fn read(&self, index: usize) -> Result<Vec<Vec<f32>>, String> {
        let a = &self.json["accessors"][index];
        let count = a["count"].as_u64().ok_or("accessor without count")? as usize;
        let width = match a["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            Some("MAT4") => 16,
            t => return Err(format!("unsupported accessor type {t:?}")),
        };
        let component = a["componentType"]
            .as_u64()
            .ok_or("accessor without componentType")?;
        let size = match component {
            5121 => 1,
            5123 => 2,
            5125 | 5126 => 4,
            c => return Err(format!("unsupported componentType {c}")),
        };
        let normalized = a["normalized"].as_bool().unwrap_or(false);
        let (data, stride) = self.view(
            a["bufferView"]
                .as_u64()
                .ok_or("sparse accessors unsupported")? as usize,
        )?;
        let base = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = if stride == 0 { width * size } else { stride };
        let mut rows = Vec::with_capacity(count);
        for i in 0..count {
            let mut row = Vec::with_capacity(width);
            for c in 0..width {
                let at = base + i * stride + c * size;
                let raw = data.get(at..at + size).ok_or("accessor out of range")?;
                row.push(match component {
                    5126 => f32::from_le_bytes(raw.try_into().unwrap()),
                    5121 if normalized => raw[0] as f32 / 255.0,
                    5121 => raw[0] as f32,
                    5123 if normalized => {
                        u16::from_le_bytes(raw.try_into().unwrap()) as f32 / 65535.0
                    }
                    5123 => u16::from_le_bytes(raw.try_into().unwrap()) as f32,
                    _ => u32::from_le_bytes(raw.try_into().unwrap()) as f32,
                });
            }
            rows.push(row);
        }
        Ok(rows)
    }

    fn image(&self, index: usize) -> Result<Texture, String> {
        let img = &self.json["images"][index];
        let (data, _) = self.view(
            img["bufferView"]
                .as_u64()
                .ok_or("external images unsupported")? as usize,
        )?;
        Texture::decode_png(data)
    }
}

struct Texture {
    width: usize,
    height: usize,
    rgba: Vec<[f32; 4]>,
}

impl Texture {
    fn decode_png(data: &[u8]) -> Result<Self, String> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| format!("png: {e}"))?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|e| format!("png: {e}"))?;
        let channels = info.color_type.samples();
        let (width, height) = (info.width as usize, info.height as usize);
        let rgba = buf[..info.buffer_size()]
            .chunks_exact(channels)
            .map(|p| {
                let g = |i: usize| p[i.min(channels - 1)] as f32 / 255.0;
                match channels {
                    1 => [g(0), g(0), g(0), 1.0],
                    2 => [g(0), g(0), g(0), g(1)],
                    3 => [g(0), g(1), g(2), 1.0],
                    _ => [g(0), g(1), g(2), g(3)],
                }
            })
            .collect();
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    fn sample(&self, uv: Vec2) -> [f32; 4] {
        let u = uv.x.rem_euclid(1.0);
        let v = uv.y.rem_euclid(1.0);
        let x = ((u * self.width as f32) as usize).min(self.width - 1);
        let y = ((v * self.height as f32) as usize).min(self.height - 1);
        self.rgba[y * self.width + x]
    }
}

impl SkinMesh {
    /// Loads the skater GLB. `budget` limits skater triangles only; authored
    /// deck, truck and wheel geometry is preserved (0 keeps every triangle).
    pub fn load(path: &Path, budget: usize) -> Result<Self, String> {
        Self::load_with_materials(path, budget).map(|(mesh, _, _)| mesh)
    }

    /// As `load`, also returning each kept triangle's glTF material index and
    /// the material names (budget 0 only: simplification merges triangles).
    pub fn load_with_materials(path: &Path, budget: usize) -> Result<(Self, Vec<usize>, Vec<String>), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let glb = Glb::parse(&bytes)?;
        let j = &glb.json;
        let skin = &j["skins"][0];
        let joints: Vec<usize> = skin["joints"]
            .as_array()
            .ok_or("GLB has no skin")?
            .iter()
            .filter_map(|v| v.as_u64().map(|v| v as usize))
            .collect();
        let joint_names: Vec<String> = joints
            .iter()
            .map(|&n| j["nodes"][n]["name"].as_str().unwrap_or("").to_string())
            .collect();
        // Parent node per node, then nearest ancestor that is a skin joint.
        let mut parent = HashMap::new();
        for (i, node) in j["nodes"].as_array().into_iter().flatten().enumerate() {
            for c in node["children"].as_array().into_iter().flatten() {
                if let Some(c) = c.as_u64() {
                    parent.insert(c as usize, i);
                }
            }
        }
        let joint_parents = joints
            .iter()
            .map(|&n| {
                let mut at = parent.get(&n).copied();
                while let Some(p) = at {
                    if let Some(k) = joints.iter().position(|&x| x == p) {
                        return Some(k);
                    }
                    at = parent.get(&p).copied();
                }
                None
            })
            .collect();
        let inverse_bind: Vec<Mat4> = glb
            .read(
                skin["inverseBindMatrices"]
                    .as_u64()
                    .ok_or("skin without inverse binds")? as usize,
            )?
            .into_iter()
            .map(|m| Mat4::from_cols_slice(&m))
            .collect();

        let mut textures: HashMap<usize, Texture> = HashMap::new();
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let mut materials = Vec::new();
        let mut board = Vec::new();
        for prim in j["meshes"][0]["primitives"]
            .as_array()
            .ok_or("GLB without mesh")?
        {
            let attr = &prim["attributes"];
            let get = |name: &str| {
                attr[name]
                    .as_u64()
                    .map(|v| v as usize)
                    .ok_or(format!("primitive without {name}"))
            };
            let positions = glb.read(get("POSITION")?)?;
            let joints0 = glb.read(get("JOINTS_0")?)?;
            let weights0 = glb.read(get("WEIGHTS_0")?)?;
            let uvs = attr["TEXCOORD_0"]
                .as_u64()
                .map(|i| glb.read(i as usize))
                .transpose()?;
            let indices = glb.read(
                prim["indices"]
                    .as_u64()
                    .ok_or("primitive without indices")? as usize,
            )?;
            let material = prim["material"].as_u64().unwrap_or(0) as usize;
            let is_board = j["materials"][material]["name"]
                .as_str()
                .is_some_and(|n| n.contains("Skate"));
            // glTF: alpha only matters for MASK/BLEND; OPAQUE ignores it
            // (the board textures carry non-coverage data in alpha).
            let cutoff = match j["materials"][material]["alphaMode"].as_str() {
                Some("MASK") | Some("BLEND") => j["materials"][material]["alphaCutoff"]
                    .as_f64()
                    .unwrap_or(0.5) as f32,
                _ => 0.0,
            };
            let pbr = &j["materials"][material]["pbrMetallicRoughness"];
            let factor: Vec<f32> = pbr["baseColorFactor"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_f64().map(|v| v as f32))
                        .collect()
                })
                .unwrap_or_else(|| vec![1.0; 4]);
            let texture = match pbr["baseColorTexture"]["index"].as_u64() {
                Some(t) => {
                    let image = j["textures"][t as usize]["source"]
                        .as_u64()
                        .ok_or("texture without source")? as usize;
                    if let std::collections::hash_map::Entry::Vacant(e) = textures.entry(image) {
                        e.insert(glb.image(image)?);
                    }
                    Some(image)
                }
                None => None,
            };
            let base = vertices.len() as u32;
            for (i, p) in positions.iter().enumerate() {
                let tex = match (texture, &uvs) {
                    (Some(t), Some(uv)) => textures[&t].sample(Vec2::new(uv[i][0], uv[i][1])),
                    _ => [1.0; 4],
                };
                let mut weights = [0.0f32; 4];
                let mut js = [0u16; 4];
                for k in 0..4 {
                    weights[k] = weights0[i][k];
                    js[k] = joints0[i][k] as u16;
                }
                let sum: f32 = weights.iter().sum();
                if sum > 0.0 {
                    weights.iter_mut().for_each(|w| *w /= sum);
                }
                vertices.push(Vertex {
                    position: Vec3::new(p[0], p[1], p[2]),
                    joints: js,
                    weights,
                    color: [tex[0] * factor[0], tex[1] * factor[1], tex[2] * factor[2]],
                    alpha: tex[3] * factor.get(3).copied().unwrap_or(1.0),
                });
            }
            for t in indices.as_chunks::<3>().0 {
                let tri = [
                    base + t[0][0] as u32,
                    base + t[1][0] as u32,
                    base + t[2][0] as u32,
                ];
                // Cutout faces (hair cards): DRAW_POLY has no alpha test.
                let alpha: f32 = tri.iter().map(|&i| vertices[i as usize].alpha).sum::<f32>() / 3.0;
                if alpha < cutoff {
                    continue;
                }
                triangles.push(tri);
                materials.push(material);
                board.push(is_board);
            }
        }
        let mut mesh = Self {
            vertices,
            triangles,
            board,
            joint_names,
            joint_parents,
            inverse_bind,
            foot_vertices: Default::default(),
        };
        for (side, name) in ["LEFTFOOT", "RIGHTFOOT"].iter().enumerate() {
            let Some(foot) = mesh.joint_names.iter().position(|n| n == name) else { continue };
            mesh.foot_vertices[side] = mesh.vertices.iter()
                .filter(|v| mostly_under(&mesh.joint_parents, foot, &v.joints, &v.weights))
                .copied().collect();
        }
        let names = j["materials"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| m["name"].as_str().unwrap_or("").to_string())
            .collect();
        if budget > 0 && mesh.triangles.len() > budget {
            mesh.simplify(&materials, budget);
            return Ok((mesh, Vec::new(), names));
        }
        Ok((mesh, materials, names))
    }

    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    /// Base colour (texture x factor) sampled at vertex `i`.
    pub fn vertex_color(&self, i: usize) -> [f32; 3] {
        self.vertices[i].color
    }

    /// Vertex positions skinned with world-space joint matrices (see `skin`).
    pub fn posed_vertices(&self, joint_world: &[Option<Mat4>]) -> Vec<Vec3> {
        self.posed(joint_world, None)
    }

    /// As `posed_vertices`; vertices outside `mask` stay at the origin.
    fn posed(&self, joint_world: &[Option<Mat4>], mask: Option<&[bool]>) -> Vec<Vec3> {
        // Unposed joints follow their nearest posed ancestor's bind offset.
        let mut skinning = vec![Mat4::IDENTITY; self.joint_names.len()];
        let mut resolved = vec![false; self.joint_names.len()];
        for j in 0..self.joint_names.len() {
            self.resolve(j, joint_world, &mut skinning, &mut resolved);
        }
        self.vertices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                if mask.is_some_and(|m| !m[i]) {
                    return Vec3::ZERO;
                }
                skin_point(&skinning, v.position, &v.joints, &v.weights)
            })
            .collect()
    }

    pub fn board_triangle_count(&self) -> usize {
        self.board.iter().filter(|b| **b).count()
    }

    pub fn inverse_bind_for(&self, name: &str) -> Option<Mat4> {
        self.joint_names.iter().position(|n| n == name)
            .and_then(|i| self.inverse_bind.get(i).copied())
    }

    /// Original shoe support planes in Skate world space. The normal follows
    /// each authored foot's skin transform; this also works airborne/held/bail
    /// and never assumes that the foot or board is aligned with the ground.
    pub fn foot_support(&self, world: &[Option<Mat4>]) -> [Option<(Vec3, f32)>; 2] {
        std::array::from_fn(|side| {
            let name = ["LEFTFOOT", "RIGHTFOOT"][side];
            let foot = self.joint_names.iter().position(|n| n == name)?;
            let transform = world.get(foot).copied().flatten()? * self.inverse_bind[foot];
            let normal = transform.transform_vector3(Vec3::Y).try_normalize()?;
            let mut plane = f32::INFINITY;
            for v in &self.foot_vertices[side] {
                let mut p = Vec3::ZERO;
                for k in 0..4 {
                    if v.weights[k] <= 0.0 { continue; }
                    let i = v.joints[k] as usize;
                    let skin = world.get(i).copied().flatten()? * *self.inverse_bind.get(i)?;
                    p += skin.transform_point3(v.position) * v.weights[k];
                }
                plane = plane.min(normal.dot(p));
            }
            plane.is_finite().then_some((normal, plane))
        })
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// Vertex clustering in bind pose, per material and skinning signature
    /// (two strongest joints + quantised top weight), growing the cell until
    /// the budget holds. Members share skinning, so a merged vertex cannot be
    /// stretched between joints; it sits at the members' mean position.
    fn simplify(&mut self, materials: &[usize], budget: usize) {
        let mut owner = vec![usize::MAX; self.vertices.len()];
        let mut board_vertex = vec![false; self.vertices.len()];
        for (t, tri) in self.triangles.iter().enumerate() {
            for &v in tri {
                owner[v as usize] = owner[v as usize].min(materials[t]);
                board_vertex[v as usize] |= self.board[t];
            }
        }
        let mut cell = 0.01f32;
        loop {
            let (map, first) = cluster(self.vertices.len(), |i| {
                let v = &self.vertices[i];
                (
                    owner[i],
                    (v.position / cell).floor().as_ivec3().to_array(),
                    signature(&v.joints, &v.weights),
                    board_vertex[i].then_some(i),
                )
            });
            let mut sums = vec![(Vec3::ZERO, [0.0f32; 3], 0.0f32); first.len()];
            for (i, v) in self.vertices.iter().enumerate() {
                let s = &mut sums[map[i] as usize];
                s.0 += v.position;
                for k in 0..3 {
                    s.1[k] += v.color[k];
                }
                s.2 += 1.0;
            }
            let out: Vec<Vertex> = first
                .iter()
                .zip(&sums)
                .map(|(&f, (p, c, n))| Vertex {
                    position: *p / *n,
                    color: [c[0] / n, c[1] / n, c[2] / n],
                    ..self.vertices[f]
                })
                .collect();
            let kept = remap(&self.triangles, &map);
            let board: Vec<bool> = kept.iter().map(|&(k, _)| self.board[k]).collect();
            let tris: Vec<[u32; 3]> = kept.into_iter().map(|(_, t)| t).collect();
            if board.iter().filter(|b| !**b).count() <= budget || cell > 0.2 {
                self.vertices = out;
                self.triangles = tris;
                self.board = board;
                return;
            }
            cell *= 1.25;
        }
    }

    /// Skins with world-space joint matrices (Skate space, one per skin
    /// joint, `None` where Skate publishes no such bone) and shades with a
    /// fixed key light. Returns Skate-space triangles.
    pub fn skin(
        &self,
        joint_world: &[Option<Mat4>],
        light: Vec3,
        board_only: bool,
    ) -> Vec<ShadedTri> {
        // Board and skater vertices are disjoint (per-primitive ranges, never
        // merged by `simplify`): board only skins and lights the board's alone.
        let board_tris: Vec<[u32; 3]>;
        let (lit_tris, mask) = if board_only {
            board_tris = self.triangles.iter().zip(&self.board).filter(|(_, b)| **b).map(|(t, _)| *t).collect();
            let mut mask = vec![false; self.vertices.len()];
            for t in &board_tris {
                for &i in t {
                    mask[i as usize] = true;
                }
            }
            (board_tris.as_slice(), Some(mask))
        } else {
            (self.triangles.as_slice(), None)
        };
        let positions = self.posed(joint_world, mask.as_deref());
        let light = light.normalize_or_zero();
        let smooth = smooth_light(&positions, lit_tris, light);
        self.triangles
            .iter()
            .zip(&self.board)
            .filter(|(_, b)| !board_only || **b)
            .filter_map(|(t, board)| {
                let [a, b, c] = t.map(|i| positions[i as usize]);
                let n = (b - a).cross(c - a).normalize_or_zero();
                if n == Vec3::ZERO || !a.is_finite() {
                    return None;
                }
                let lit = light_term(n, light);
                let color = t
                    .iter()
                    .fold([0.0f32; 3], |acc, &i| {
                        let c = self.vertices[i as usize].color;
                        [
                            acc[0] + c[0] / 3.0,
                            acc[1] + c[1] / 3.0,
                            acc[2] + c[2] / 3.0,
                        ]
                    })
                    .map(|c| (c * lit).clamp(0.0, 1.0));
                Some(ShadedTri {
                    points: [a, b, c],
                    rgba: [
                        (color[0] * 255.0) as u8,
                        (color[1] * 255.0) as u8,
                        (color[2] * 255.0) as u8,
                        255,
                    ],
                    vertex_rgba: t
                        .map(|i| to_rgba(self.vertices[i as usize].color, smooth[i as usize])),
                    uv: [[0.0; 2]; 3],
                    light: t.map(|i| light_byte(smooth[i as usize])),
                    texture: NO_TEXTURE,
                    cutout: false,
                    board: *board,
                })
            })
            .collect()
    }

    fn resolve(
        &self,
        j: usize,
        world: &[Option<Mat4>],
        out: &mut [Mat4],
        done: &mut [bool],
    ) -> Mat4 {
        if done[j] {
            return out[j];
        }
        let m = match world.get(j).copied().flatten() {
            Some(w) => w * self.inverse_bind[j],
            None => match self.joint_parents[j] {
                Some(p) => self.resolve(p, world, out, done),
                None => Mat4::IDENTITY,
            },
        };
        out[j] = m;
        done[j] = true;
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh() -> SkinMesh {
        // Two joints; a quad on joint 0 and a quad on joint 1, 1 m apart.
        let v = |x: f32, y: f32, j: u16| Vertex {
            position: Vec3::new(x, y, 0.0),
            joints: [j, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
            color: [1.0, 0.5, 0.25],
            alpha: 1.0,
        };
        SkinMesh {
            vertices: vec![
                v(0., 0., 0),
                v(1., 0., 0),
                v(0., 1., 0),
                v(5., 0., 1),
                v(6., 0., 1),
                v(5., 1., 1),
            ],
            triangles: vec![[0, 1, 2], [3, 4, 5]],
            board: vec![false, true],
            joint_names: vec!["HIPS".into(), "SKATEBOARD_ROOT".into()],
            joint_parents: vec![None, Some(0)],
            foot_vertices: Default::default(),
            inverse_bind: vec![
                Mat4::IDENTITY,
                Mat4::from_translation(Vec3::new(-5.0, 0.0, 0.0)),
            ],
        }
    }

    #[test]
    fn simplification_preserves_authored_board_geometry() {
        let mut m = mesh();
        // Thin wheel/truck details must not collapse into the skater budget.
        for i in 3..6 {m.vertices[i].position *= 0.0001;}
        let original = m.triangles[1].map(|i|m.vertices[i as usize].position);
        m.simplify(&[0,1],1);
        let board:Vec<_>=m.triangles.iter().zip(&m.board).filter(|(_,b)|**b).collect();
        assert_eq!(board.len(),1);
        assert_eq!(board[0].0.map(|i|m.vertices[i as usize].position),original);
    }

    #[test]
    fn skinning_follows_joint_world_matrices() {
        let m = mesh();
        let world = [
            Some(Mat4::from_translation(Vec3::new(0.0, 0.0, 10.0))),
            Some(Mat4::from_translation(Vec3::new(5.0, 0.0, -3.0))),
        ];
        let tris = m.skin(&world, Vec3::Z, false);
        assert_eq!(tris.len(), 2);
        assert!(tris[0].points[0].distance(Vec3::new(0.0, 0.0, 10.0)) < 1e-5);
        assert!(tris[1].points[0].distance(Vec3::new(5.0, 0.0, -3.0)) < 1e-5);
        assert_eq!(
            tris[0].rgba,
            [255, 140, 70, 255],
            "fully lit face: colour x 1.1 (0.6 ambient + 0.5 key), clamped"
        );
    }

    #[test]
    fn foot_support_follows_authored_foot_rotation_instead_of_world_up() {
        let mut m = mesh();
        m.joint_names[0] = "LEFTFOOT".into();
        m.foot_vertices[0] = m.vertices[..3].to_vec();
        let q = bevy_math::Quat::from_rotation_x(1.1) * bevy_math::Quat::from_rotation_z(0.7);
        let t = Vec3::new(3., 4., 5.);
        let world = [Some(Mat4::from_rotation_translation(q, t)), None];
        let (normal, plane) = m.foot_support(&world)[0].unwrap();
        assert!(normal.distance(q * Vec3::Y) < 1e-6);
        assert!((plane - normal.dot(t)).abs() < 1e-6);
        assert!(m.foot_support(&world)[1].is_none());
        assert!(m.foot_support(&[None, None])[0].is_none());
    }

    #[test]
    fn unposed_joint_follows_its_ancestor() {
        let m = mesh();
        let world = [Some(Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0))), None];
        let tris = m.skin(&world, Vec3::Z, false);
        // Joint 1 inherits joint 0's skinning matrix (translation +2 y).
        assert!(tris[1].points[0].distance(Vec3::new(5.0, 2.0, 0.0)) < 1e-5);
    }

    #[test]
    fn board_only_keeps_board_triangles() {
        let m = mesh();
        let world = [Some(Mat4::IDENTITY), Some(Mat4::IDENTITY)];
        assert_eq!(m.skin(&world, Vec3::Z, true).len(), 1);
    }

    #[test]
    fn faces_away_from_light_are_darker() {
        let m = mesh();
        let world = [
            Some(Mat4::IDENTITY),
            Some(Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0))),
        ];
        let up = m.skin(&world, Vec3::Z, false)[0].rgba[0];
        let down = m.skin(&world, -Vec3::Z, false)[0].rgba[0];
        assert!(down < up);
    }
}

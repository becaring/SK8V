//! GTA ped model -> SkateV ped cache, from the user's own extracted files.
//!
//!     skatev-ped-export <streamedpeds dir> <ped name> <out dir>
//!     skatev-ped-export --skeletons <dir of ped .yft files> <out dir>
//!
//! Reads `<ped>.yft` (skeleton), `<ped>/<comp>_<NNN>_<race>.ydd` (skinned
//! component meshes, highest LOD) and `<ped>/<comp>_diff_<NNN>_<letter>_*.ytd`
//! (diffuse variants). Writes `<out>/<ped>/skeleton.json` and one `.svpc` per
//! component drawable. Output is retail-derived and stays local (ignored).
//!
//! `.svpc` (little-endian):
//!   "SVPC" u32 version=4 u32 vertex_count u32 triangle_count u32 variant_count
//!   vertex: 3*f32 position (ped model space), 4*u16 skeleton bone index,
//!           4*f32 weight, 2*f32 texcoord0
//!
//! A ped vertex's blend indices address its geometry's bone-ID list
//! (geometry +0x68 / count +0x72), which maps to skeleton bones. The
//! high-level `parse_ydd` drops that list, so it is read through the pinned
//! low-level `blocks` reader and applied here.
//!   triangle: 3*u32
//!   variant: u8 letter, u8 n + n bytes texture dictionary (the .ytd stem),
//!            u8 n + n bytes texture name inside it, then vertex_count *
//!            4*u8 colour (RGBA; A is the diffuse alpha, so hair cards and
//!            other cutouts can be told apart)
use rage_formats::blocks::base::StringBlock;
use rage_formats::blocks::drawable::{
    Drawable as DrawableBlock, DrawableGeometry as GeometryBlock, DrawableModel as ModelBlock,
    DrawableModelsBlock,
};
use rage_formats::blocks::skeleton::{Bone, Skeleton, SkeletonBonesBlock};
use rage_formats::blocks::{Graph, Reader};
use rage_formats::resource::SYSTEM_BASE;
use rage_formats::texture_utils::decompress_texture;
use rage_formats::ydd::VertexSemantic;
use rage_formats::ydd::{DrawableEntry, parse_ydd};
use rage_formats::ytd::parse_ytd;
use serde_json::json;
use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

type R<T> = Result<T, Box<dyn Error>>;

fn skeleton(yft: &Path) -> R<serde_json::Value> {
    let bytes = fs::read(yft)?;
    let mut reader = Reader::open(&bytes)?;
    // FragType +0x30 -> FragDrawable; its DrawableBase header holds the
    // skeleton pointer at +0x18 (after vft, pages, shader group). The rest of
    // a FragDrawable differs from a plain Drawable, so only the skeleton is read.
    let drawable_va = reader.u64s(SYSTEM_BASE + 0x30, 1)?[0];
    let skeleton_va = reader.u64s(drawable_va + 0x18, 1)?[0];
    let mut g = Graph::new();
    let sk_id =
        Skeleton::read(&mut reader, &mut g, skeleton_va)?.ok_or("drawable has no skeleton")?;
    let bones_id = g
        .get::<Skeleton>(sk_id)
        .bones
        .ok_or("skeleton has no bones")?;
    let bones = g.get::<SkeletonBonesBlock>(bones_id).bones.clone();
    let out: Vec<serde_json::Value> = bones
        .iter()
        .map(|&b| {
            let bone = g.get::<Bone>(b);
            let name = bone
                .name
                .map(|n| g.get::<StringBlock>(n).0.clone())
                .unwrap_or_default();
            json!({
                "name": name,
                "tag": bone.tag,
                "parent": bone.parent,
                "t": [bone.translation.x, bone.translation.y, bone.translation.z],
                "r": [bone.rotation.x, bone.rotation.y, bone.rotation.z, bone.rotation.w],
                "s": [bone.scale.x, bone.scale.y, bone.scale.z],
            })
        })
        .collect();
    Ok(json!({ "source": yft.file_name().unwrap().to_string_lossy(), "bones": out }))
}

/// Per dictionary drawable (file order): for each high-LOD geometry, the bone
/// TAG each blend index refers to. Ped components carry their own small
/// skeleton; a vertex index goes geometry bone-ID list -> component bone ->
/// tag, and the tag names the bone in the full ped skeleton.
fn geometry_bone_tags(bytes: &[u8]) -> R<Vec<Vec<Vec<u16>>>> {
    let mut reader = Reader::open(bytes)?;
    // DrawableDictionary: +0x30 drawable pointer array, +0x38 u16 count.
    let array = reader.u64s(SYSTEM_BASE + 0x30, 1)?[0];
    let count = reader.u16s(SYSTEM_BASE + 0x38, 1)?[0] as usize;
    let pointers = reader.u64s(array, count)?;
    let mut g = Graph::new();
    let mut out = Vec::new();
    for va in pointers {
        let mut lists = Vec::new();
        if let Some(d) = DrawableBlock::read(&mut reader, &mut g, va)? {
            let tags: Vec<u16> = g
                .get::<DrawableBlock>(d)
                .skeleton
                .and_then(|sk| g.get::<Skeleton>(sk).bones)
                .map(|b| {
                    g.get::<SkeletonBonesBlock>(b)
                        .bones
                        .iter()
                        .map(|&b| g.get::<Bone>(b).tag)
                        .collect()
                })
                .unwrap_or_default();
            if let Some(models) = g.get::<DrawableBlock>(d).models {
                for &m in g.get::<DrawableModelsBlock>(models).high.iter().flatten() {
                    for &geo in &g.get::<ModelBlock>(m).geometries {
                        let ids = &g.get::<GeometryBlock>(geo).bone_ids;
                        // No bone-ID list: blend indices address the component skeleton directly.
                        let list: Vec<u16> = if ids.is_empty() {
                            tags.clone()
                        } else {
                            ids.iter()
                                .map(|&i| tags.get(i as usize).copied().unwrap_or(0))
                                .collect()
                        };
                        lists.push(list);
                    }
                }
            }
        }
        out.push(lists);
    }
    Ok(out)
}

struct Texture {
    name: String,
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

fn load_texture(ytd: &Path) -> Option<Texture> {
    let bytes = fs::read(ytd).ok()?;
    let tex = parse_ytd(&bytes).ok()?.into_iter().next()?;
    let rgba = decompress_texture(&tex).ok()?;
    let (w, h) = (tex.width as usize, tex.height as usize);
    (rgba.len() >= w * h * 4).then_some(Texture {
        name: tex.name.to_ascii_lowercase(),
        w,
        h,
        rgba,
    })
}

fn sample(t: &Texture, u: f32, v: f32) -> [u8; 4] {
    let x = ((u.rem_euclid(1.0) * t.w as f32) as usize).min(t.w - 1);
    let y = ((v.rem_euclid(1.0) * t.h as f32) as usize).min(t.h - 1);
    let i = (y * t.w + x) * 4;
    [t.rgba[i], t.rgba[i + 1], t.rgba[i + 2], t.rgba[i + 3]]
}

struct Mesh {
    /// Vertex belongs to a character-cloth geometry (its blend bytes drive
    /// GTA's cloth simulation, not the skeleton) and must be rebound.
    cloth: Vec<bool>,
    positions: Vec<[f32; 3]>,
    bones: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
    uvs: Vec<[f32; 2]>,
    triangles: Vec<[u32; 3]>,
}

fn mesh(
    entries: &[DrawableEntry],
    bone_ids: &[Vec<Vec<u16>>],
    tag_index: &BTreeMap<u16, u16>,
    bone_pos: &[[f32; 3]],
) -> R<Mesh> {
    let mut m = Mesh {
        cloth: vec![],
        positions: vec![],
        bones: vec![],
        weights: vec![],
        uvs: vec![],
        triangles: vec![],
    };
    for (e, entry) in entries.iter().enumerate() {
        let d = &entry.drawable;
        let Some(lod) = d.lod(rage_formats::ydd::LodLevel::High) else {
            continue;
        };
        let mut geo_index = 0usize;
        for model in &lod.models {
            for geo in &model.geometries {
                let ids = bone_ids
                    .get(e)
                    .and_then(|l| l.get(geo_index))
                    .cloned()
                    .unwrap_or_default();
                geo_index += 1;
                let (Some(vb), Some(ib)) = (&geo.vertex_buffer, &geo.index_buffer) else {
                    continue;
                };
                let verts = vb.to_unified_vertices()?;
                // Raw blend index bytes (same byte order the weights decode in).
                let stride = vb.vertex_stride as usize;
                let blend_at = vb
                    .declaration
                    .as_ref()
                    .and_then(|d| {
                        d.components
                            .iter()
                            .find(|c| c.semantic == VertexSemantic::BlendIndices)
                    })
                    .map(|c| c.offset as usize);
                let base = m.positions.len() as u32;
                // Character cloth (jacket bodies/tails): weighted blend bytes
                // beyond the geometry's bone list. Measured on Michael's
                // uppr_000: geometry 0 indexes 1..28 of 29 bones, geometries
                // 1-2 reach 255. Such geometries are rebound below.
                let cloth = blend_at.is_some_and(|o| {
                    let mut out_of_range = 0usize;
                    let mut total = 0usize;
                    for (i, v) in verts.iter().enumerate() {
                        let Some(raw) = vb.data.get(i * stride + o..i * stride + o + 4) else {
                            continue;
                        };
                        let w = [
                            v.blend_weights.x,
                            v.blend_weights.y,
                            v.blend_weights.z,
                            v.blend_weights.w,
                        ];
                        for k in 0..4 {
                            if w[k] > 0.0 {
                                total += 1;
                                if raw[k] as usize >= ids.len() {
                                    out_of_range += 1;
                                }
                            }
                        }
                    }
                    total > 0 && out_of_range * 20 > total
                });
                for (i, v) in verts.iter().enumerate() {
                    m.positions.push([v.position.x, v.position.y, v.position.z]);
                    let raw = blend_at
                        .and_then(|o| vb.data.get(i * stride + o..i * stride + o + 4))
                        .map(|b| [b[0], b[1], b[2], b[3]])
                        .unwrap_or([0; 4]);
                    // Inside a cloth geometry, a vertex keeps its own skinning
                    // (sleeves do) only if every weighted index is in range and
                    // its dominant bone lies near it in bind pose; cloth bytes
                    // that happen to look valid point at distant bones.
                    let vertex_cloth = cloth && {
                        let w = [
                            v.blend_weights.x,
                            v.blend_weights.y,
                            v.blend_weights.z,
                            v.blend_weights.w,
                        ];
                        let in_range = (0..4).all(|k| w[k] <= 0.0 || (raw[k] as usize) < ids.len());
                        let k = (0..4).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap();
                        let near = ids
                            .get(raw[k] as usize)
                            .and_then(|tag| tag_index.get(tag))
                            .and_then(|&b| bone_pos.get(b as usize))
                            .is_some_and(|bp| {
                                let d = [
                                    bp[0] - v.position.x,
                                    bp[1] - v.position.y,
                                    bp[2] - v.position.z,
                                ];
                                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < 0.6
                            });
                        !(in_range && near)
                    };
                    m.cloth.push(vertex_cloth);
                    let resolved = raw.map(|b| {
                        ids.get(b as usize)
                            .and_then(|tag| tag_index.get(tag))
                            .copied()
                    });
                    m.bones.push(resolved.map(|r| r.unwrap_or(0)));
                    // Weights pair with index bytes in raw order (verified by
                    // bending single bones in a render; a BGRA swizzle moved
                    // the wrong limbs). An influence whose tag is not in the
                    // ped skeleton is dropped, never bound to the root.
                    let mut w = [
                        v.blend_weights.x,
                        v.blend_weights.y,
                        v.blend_weights.z,
                        v.blend_weights.w,
                    ];
                    for (k, r) in resolved.iter().enumerate() {
                        if r.is_none() {
                            w[k] = 0.0;
                        }
                    }
                    let sum: f32 = w.iter().sum();
                    m.weights.push(if sum > 0.0 {
                        w.map(|x| x / sum)
                    } else {
                        [1.0, 0.0, 0.0, 0.0]
                    });
                    m.uvs.push([v.texcoord0.x, v.texcoord0.y]);
                }
                for t in ib.indices.as_chunks::<3>().0 {
                    if t.iter().all(|&i| (i as usize) < verts.len()) {
                        m.triangles.push([base + t[0], base + t[1], base + t[2]]);
                    }
                }
            }
        }
    }
    Ok(m)
}

/// Rebinds cloth vertices to the skinning of the nearest skinned vertex in
/// bind pose (same component), or to the nearest SKEL_ bone if the component
/// has none, so cloth follows the body instead of collapsing to the root.
fn rebind_cloth(m: &mut Mesh, skel_bones: &[(u16, [f32; 3])], arm_bones: &[u16]) -> usize {
    // Cloth (jacket bodies/tails) hangs off the torso and hips: never take
    // skinning from arm-chain vertices, or a hem beside a hanging wrist would
    // follow the hand (measured: 6 cm bind edges stretched to 49 cm).
    let dominant = |i: usize| {
        let w = m.weights[i];
        let k = (0..4).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap();
        m.bones[i][k]
    };
    let skinned: Vec<usize> = (0..m.positions.len())
        .filter(|&i| !m.cloth[i] && !arm_bones.contains(&dominant(i)))
        .collect();
    let d2 = |a: [f32; 3], b: [f32; 3]| {
        (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
    };
    let mut rebound = 0;
    for i in 0..m.positions.len() {
        if !m.cloth[i] {
            continue;
        }
        let p = m.positions[i];
        if let Some(&j) = skinned
            .iter()
            .min_by(|&&a, &&b| d2(p, m.positions[a]).total_cmp(&d2(p, m.positions[b])))
        {
            m.bones[i] = m.bones[j];
            m.weights[i] = m.weights[j];
        } else if let Some((bone, _)) = skel_bones
            .iter()
            .min_by(|a, b| d2(p, a.1).total_cmp(&d2(p, b.1)))
        {
            m.bones[i] = [*bone, 0, 0, 0];
            m.weights[i] = [1.0, 0.0, 0.0, 0.0];
        }
        rebound += 1;
    }
    rebound
}

/// Bone-name fragments of the arm chains (never cloth anchors).
const ARM_WORDS: [&str; 8] = [
    "Arm", "Hand", "Cuff", "Clavicle", "Elbow", "Finger", "Wrist", "Shoulder",
];

/// Bind-pose world positions of the SKEL_ bones (fallback cloth anchors).
fn bone_positions(sk: &serde_json::Value) -> Vec<(u16, [f32; 3])> {
    let bones = sk["bones"].as_array().cloned().unwrap_or_default();
    let f = |v: &serde_json::Value, i: usize| v[i].as_f64().unwrap_or(0.0) as f32;
    let mut world: Vec<([[f32; 3]; 3], [f32; 3])> = Vec::with_capacity(bones.len());
    for b in &bones {
        let (x, y, z, w) = (f(&b["r"], 0), f(&b["r"], 1), f(&b["r"], 2), f(&b["r"], 3));
        let r = [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ];
        let t = [f(&b["t"], 0), f(&b["t"], 1), f(&b["t"], 2)];
        let entry = match b["parent"]
            .as_i64()
            .filter(|&p| p >= 0)
            .and_then(|p| world.get(p as usize))
        {
            Some((pr, pt)) => {
                let mul = |a: &[[f32; 3]; 3], v: [f32; 3]| {
                    [0, 1, 2].map(|i| a[i][0] * v[0] + a[i][1] * v[1] + a[i][2] * v[2])
                };
                let rt = mul(pr, t);
                let mut rr = [[0.0; 3]; 3];
                for i in 0..3 {
                    for j in 0..3 {
                        rr[i][j] = (0..3).map(|k| pr[i][k] * r[k][j]).sum();
                    }
                }
                (rr, [pt[0] + rt[0], pt[1] + rt[1], pt[2] + rt[2]])
            }
            None => (r, t),
        };
        world.push(entry);
    }
    bones
        .iter()
        .enumerate()
        .map(|(i, _)| (i as u16, world[i].1))
        .collect()
}

/// One diffuse variant: letter, texture dictionary, texture name, colours.
type Variant = (u8, String, String, Vec<[u8; 4]>);

fn write_svpc(path: &Path, m: &Mesh, variants: &[Variant]) -> R<()> {
    let mut out = Vec::new();
    out.extend_from_slice(b"SVPC");
    for v in [
        4u32,
        m.positions.len() as u32,
        m.triangles.len() as u32,
        variants.len() as u32,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for i in 0..m.positions.len() {
        for x in m.positions[i] {
            out.extend_from_slice(&x.to_le_bytes());
        }
        for b in m.bones[i] {
            out.extend_from_slice(&b.to_le_bytes());
        }
        for w in m.weights[i] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        for x in m.uvs.get(i).copied().unwrap_or_default() {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
    for t in &m.triangles {
        for i in t {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
    for (letter, dict, texture, colors) in variants {
        out.push(*letter);
        for name in [dict, texture] {
            let bytes = &name.as_bytes()[..name.len().min(255)];
            out.push(bytes.len() as u8);
            out.extend_from_slice(bytes);
        }
        for c in colors {
            out.extend_from_slice(c);
        }
    }
    fs::File::create(path)?.write_all(&out)?;
    Ok(())
}

/// Every `.yft` under `dir`, base-game archives before DLC packs (`x64/...`)
/// so a DLC's newer copy of a model is written last.
fn yfts(dir: &Path, out: &mut Vec<PathBuf>) -> R<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            yfts(&path, out)?;
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("yft")) {
            out.push(path);
        }
    }
    Ok(())
}

/// `--skeletons <dir> <out>`: skeleton-only cache entries for every ped
/// model under `dir`. With Presentation=Ped GTA renders the ped itself, so
/// the runtime only needs its skeleton to pose it (Director Mode actors and
/// other non-protagonist player models). Full entries are left as they are.
fn export_skeletons(dir: &Path, out: &Path) -> R<()> {
    let mut files = Vec::new();
    yfts(dir, &mut files)?;
    files.sort_by_key(|p| (p.strip_prefix(dir).is_ok_and(|r| r.starts_with("x64")), p.clone()));
    let (mut written, mut kept, mut failed) = (0, 0, 0);
    for yft in files {
        let ped = yft.file_stem().unwrap().to_string_lossy().to_ascii_lowercase();
        let dest = out.join(&ped);
        let full = fs::read_dir(&dest).is_ok_and(|mut d| {
            d.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "svpc")))
        });
        if full {
            kept += 1;
            continue;
        }
        match skeleton(&yft) {
            Ok(sk) => {
                fs::create_dir_all(&dest)?;
                fs::write(dest.join("skeleton.json"), serde_json::to_string_pretty(&sk)?)?;
                written += 1;
            }
            Err(e) => {
                eprintln!("{}: {e}", yft.display());
                failed += 1;
            }
        }
    }
    eprintln!("skeletons: {written} written, {kept} full entries kept, {failed} failed");
    Ok(())
}

fn main() -> R<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 4 && args[1] == "--skeletons" {
        return export_skeletons(Path::new(&args[2]), Path::new(&args[3]));
    }
    if args.len() < 4 {
        return Err("usage: skatev-ped-export <streamedpeds dir> <ped name> <out dir>".into());
    }
    let (root, ped, out) = (
        PathBuf::from(&args[1]),
        args[2].clone(),
        PathBuf::from(&args[3]),
    );
    let dest = out.join(&ped);
    fs::create_dir_all(&dest)?;

    let sk = skeleton(&root.join(format!("{ped}.yft")))?;
    let bone_count = sk["bones"].as_array().map_or(0, |b| b.len());
    fs::write(
        dest.join("skeleton.json"),
        serde_json::to_string_pretty(&sk)?,
    )?;
    let tag_index: BTreeMap<u16, u16> = sk["bones"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, b)| Some((b["tag"].as_u64()? as u16, i as u16)))
        .collect();
    eprintln!("{ped}: skeleton {bone_count} bones");

    // Diffuse textures per (component, drawable): letter -> path.
    let comp_dir = root.join(&ped);
    let mut textures: BTreeMap<(String, String), Vec<(u8, PathBuf)>> = BTreeMap::new();
    let mut drawables = Vec::new();
    for entry in fs::read_dir(&comp_dir)? {
        let path = entry?.path();
        let name = path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase();
        let parts: Vec<&str> = name.split('_').collect();
        match path.extension().and_then(|e| e.to_str()) {
            Some("ytd") if parts.len() >= 4 && parts[1] == "diff" => {
                let letter = parts[3].bytes().next().unwrap_or(b'a');
                textures
                    .entry((parts[0].to_string(), parts[2].to_string()))
                    .or_default()
                    .push((letter, path));
            }
            Some("ydd") if parts.len() >= 3 => drawables.push((
                parts[0].to_string(),
                parts[1].to_string(),
                name.clone(),
                path,
            )),
            _ => {}
        }
    }
    let (mut written, mut tris) = (0usize, 0usize);
    let (mut cloth_vertices, mut cloth_files) = (0usize, 0usize);
    let all_bones = bone_positions(&sk);
    let bone_pos: Vec<[f32; 3]> = all_bones.iter().map(|b| b.1).collect();
    let skel_anchor: Vec<(u16, [f32; 3])> = all_bones
        .into_iter()
        .filter(|(i, _)| {
            let n = sk["bones"][*i as usize]["name"].as_str().unwrap_or("");
            n.starts_with("SKEL_") && !ARM_WORDS.iter().any(|w| n.contains(w))
        })
        .collect();
    let arm_bones: Vec<u16> = sk["bones"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, b)| {
            b["name"]
                .as_str()
                .is_some_and(|n| ARM_WORDS.iter().any(|w| n.contains(w)))
        })
        .map(|(i, _)| i as u16)
        .collect();
    for (comp, number, stem, path) in drawables {
        let bytes = fs::read(&path)?;
        let entries = match parse_ydd(&bytes) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("skip {stem}: {e:#}");
                continue;
            }
        };
        let ids = geometry_bone_tags(&bytes).unwrap_or_else(|e| {
            eprintln!("warn {stem}: no geometry bone tags ({e})");
            Vec::new()
        });
        let mut m = mesh(&entries, &ids, &tag_index, &bone_pos)?;
        let rebound = rebind_cloth(&mut m, &skel_anchor, &arm_bones);
        if rebound > 0 {
            cloth_vertices += rebound;
            cloth_files += 1;
        }
        if m.triangles.is_empty() {
            continue;
        }
        let mut variants: Vec<Variant> = Vec::new();
        for (letter, tex_path) in textures
            .get(&(comp.clone(), number.clone()))
            .cloned()
            .unwrap_or_default()
        {
            if variants.iter().any(|v| v.0 == letter) {
                continue;
            }
            if let Some(t) = load_texture(&tex_path) {
                let dict = tex_path
                    .file_stem()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                let colors = m.uvs.iter().map(|uv| sample(&t, uv[0], uv[1])).collect();
                variants.push((letter, dict, t.name, colors));
            }
        }
        if variants.is_empty() {
            variants.push((
                b'a',
                String::new(),
                String::new(),
                vec![[180, 180, 180, 255]; m.positions.len()],
            ));
        }
        variants.sort_by_key(|v| v.0);
        write_svpc(&dest.join(format!("{stem}.svpc")), &m, &variants)?;
        written += 1;
        tris += m.triangles.len();
    }
    eprintln!(
        "{ped}: {written} component drawables, {tris} triangles ({cloth_vertices} cloth vertices rebound in {cloth_files} files) -> {}",
        dest.display()
    );
    Ok(())
}

//! Interior (MLO) collision placed into world space.
//!
//! An interior's `.ybn` (under `levels/gta5/interiors`) is in MLO-local
//! space; the `.ymap` `CMloInstanceDef`s say where each MLO stands. The
//! interior bound is named after the MLO archetype (`v_michael.ybn` for
//! archetype `v_michael`). Owner run 07:01: activating inside Michael's house
//! fell through the world, as interiors were left out of the cache.
#[path = "interiors/doors.rs"]
pub mod doors;

use crate::primitives;
use rage_formats::math::Vec3;
use rage_formats::{Triangle, YmapEntity, parse_ymap_mlo_instances, parse_ymf, rage_joaat};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Stats {
    pub ymaps: usize,
    pub instances: usize,
    pub placed: usize,
    pub unmatched: usize,
    /// Placements whose bounds came from a manifest interior list.
    pub via_manifest: usize,
    /// (bound file stem, world position) of each placed manifest interior.
    pub manifest_placed: Vec<(String, [f32; 3])>,
    /// Every placement: (archetype hash, position, bound file stems found, names wanted).
    pub all: Vec<(u32, [f32; 3], Vec<String>, usize)>,
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    if let Some(files) = crate::vfs::files_under(dir, ext) {
        out.extend(files);
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, ext, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
            out.push(p);
        }
    }
}

fn key(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Files under `base` overridden by name by each layer of `layers_dir`
/// (`layers.txt`, lowest priority first), as the game streams them.
pub fn layered(
    base: &Path,
    layers_dir: Option<&Path>,
    ext: &str,
    filter: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(base, ext, &mut files);
    let mut by_name: BTreeMap<String, PathBuf> = files
        .into_iter()
        .filter(|p| filter(p))
        .map(|p| (key(&p), p))
        .collect();
    if let Some(dir) = layers_dir {
        // layers.txt sits in the layer directory or (gta-meta) beside it.
        let order = crate::vfs::read_to_string(dir.join("layers.txt"))
            .or_else(|_| crate::vfs::read_to_string(dir.parent().unwrap_or(dir).join("layers.txt")));
        if let Ok(order) = order {
            for layer in order.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let mut f = Vec::new();
                let root = dir.join(layer);
                walk(&root, ext, &mut f);
                let enabled = crate::packs::enabled(&root);
                for p in f.into_iter().filter(|p| filter(p) && crate::packs::allows(&root, &enabled, p)) {
                    by_name.insert(key(&p), p);
                }
            }
        }
    }
    by_name.into_values().collect()
}

/// Every `_manifest.ymf` under `meta_dir` (base, then layers).
fn manifests(meta_dir: &Path) -> Vec<rage_formats::Manifest> {
    let mut files = Vec::new();
    walk(&meta_dir.join("base"), "ymf", &mut files);
    if let Ok(order) = crate::vfs::read_to_string(meta_dir.join("layers.txt")) {
        for layer in order.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let root = meta_dir.join("layers").join(layer);
            let mut f = Vec::new();
            walk(&root, "ymf", &mut f);
            let enabled = crate::packs::enabled(&root);
            files.extend(f.into_iter().filter(|p| crate::packs::allows(&root, &enabled, p)));
        }
    }
    files
        .iter()
        .filter_map(|p| crate::vfs::read(p).ok())
        .filter_map(|b| parse_ymf(&b).ok().map(|(_, m)| m))
        .collect()
}

/// Ymaps that only stream when a script asks for them: members of manifest
/// map-data (IMAP) groups that are not time- or weather-driven. Alternate
/// interior states and Online-only buildings live here; baking them would
/// leave invisible walls in Story Mode.
pub fn script_toggled_ymaps(meta_dir: &Path) -> std::collections::HashSet<u32> {
    let mut out = std::collections::HashSet::new();
    for m in manifests(meta_dir) {
        for g in m.map_data_groups {
            if g.weather_types.is_empty() && g.hours_on_off == 0 {
                out.insert(g.name.hash);
                out.extend(g.bounds.iter().map(|b| b.hash));
            }
        }
    }
    out
}

/// Lowercase JOAAT of a file's stem.
pub fn stem_hash(p: &Path) -> u32 {
    let name = key(p);
    rage_joaat(name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s))
}

fn is_interior(p: &Path) -> bool {
    let s = p.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    s.contains("/interiors/") && is_bound(p)
}

/// Any collision bound file (no `hi@` vehicle detail, no `ma@` procedural
/// placement surface). Interior bounds also
/// live inside map archives and are reached through the manifests (Portola
/// Drive station: `_cityw/beverly_01/bh1_rd.rpf/bh1_rd2_portola_subway.ybn`).
fn is_bound(p: &Path) -> bool {
    !key(p).starts_with("hi@") && !key(p).starts_with("ma@")
}

/// Name hashes of every bound a manifest lists as interior (MLO-local)
/// collision; the world pass must not read these as world space.
pub fn interior_bound_hashes(meta_dir: &Path) -> std::collections::HashSet<u32> {
    manifests(meta_dir)
        .into_iter()
        .flat_map(|m| m.interiors)
        .flat_map(|i| i.bounds)
        .map(|b| b.hash)
        .collect()
}

/// MLO placement: the stored rotation is applied as stored (unlike plain
/// entities, whose stored rotation is the inverse); `invert` flips that, for
/// the diagnostic that checks the convention against the exterior shell.
pub fn place(e: &YmapEntity, local: Vec3, invert: bool) -> Vec3 {
    let [x, y, z, w] = e.rotation;
    let q = if invert {
        [-x, -y, -z, w]
    } else {
        [x, y, z, w]
    };
    let u = Vec3::new(q[0], q[1], q[2]);
    let w = q[3];
    u * (2.0 * u.dot(local)) + local * (w * w - u.dot(u)) + u.cross(local) * (2.0 * w) + e.position
}

/// Interior bound files and the MLO -> bound lists, for placing shells.
pub struct Shells {
    bounds: BTreeMap<u32, PathBuf>,
    interior_named: BTreeMap<u32, PathBuf>,
    by_mlo: BTreeMap<u32, Vec<u32>>,
    local_cache: BTreeMap<u32, Vec<Triangle>>,
}

impl Shells {
    pub fn load(meta_dir: &Path, ybn_dirs: &[PathBuf], ybn_layers: Option<&Path>) -> Self {
        // Bound files by name hash: every archive for manifest-listed bounds,
        // interiors only for the name-match fallback.
        let mut bounds: BTreeMap<u32, PathBuf> = BTreeMap::new();
        let mut interior_named: BTreeMap<u32, PathBuf> = BTreeMap::new();
        let mut add = |p: PathBuf| {
            let h = rage_joaat(&key(&p).trim_end_matches(".ybn").to_string());
            if is_interior(&p) {
                interior_named.insert(h, p.clone());
            }
            bounds.insert(h, p);
        };
        for dir in ybn_dirs {
            let mut f = Vec::new();
            walk(dir, "ybn", &mut f);
            for p in f.into_iter().filter(|p| is_bound(p)) {
                add(p);
            }
        }
        if let Some(dir) = ybn_layers {
            for p in layered(&dir.join("__none__"), Some(dir), "ybn", is_bound) {
                add(p);
            }
        }
        // MLO archetype -> its bound files, from the archives' `_manifest.ymf`
        // interior lists (the metro and sewer sections are named apart from
        // their bounds: `v_metro_sections` places `metro_30`, ...). Later
        // layers win. Without a manifest entry the bound named after the
        // archetype is used.
        let mut by_mlo: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for m in manifests(meta_dir) {
            for i in m.interiors {
                by_mlo.insert(i.name.hash, i.bounds.iter().map(|b| b.hash).collect());
            }
        }
        Shells { bounds, interior_named, by_mlo, local_cache: BTreeMap::new() }
    }

    /// The world-space shells of one ymap's MLO placements.
    pub fn place_ymap(
        &mut self,
        bytes: &[u8],
        options: primitives::Options,
        invert: bool,
        counts: &mut primitives::Counts,
        stats: &mut Stats,
        out: &mut Vec<Triangle>,
    ) {
        let Ok(instances) = parse_ymap_mlo_instances(bytes) else {
            return;
        };
        for inst in instances {
            stats.instances += 1;
            let e = inst.entity;
            let (names, paths): (Vec<u32>, Vec<&PathBuf>) = match self.by_mlo.get(&e.archetype_hash) {
                Some(listed) => (
                    listed.clone(),
                    listed.iter().filter_map(|h| self.bounds.get(h)).collect(),
                ),
                None => (
                    vec![e.archetype_hash],
                    self.interior_named.get(&e.archetype_hash).into_iter().collect(),
                ),
            };
            stats.all.push((
                e.archetype_hash,
                [e.position.x, e.position.y, e.position.z],
                paths
                    .iter()
                    .map(|p| {
                        p.file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default()
                    })
                    .collect(),
                names.len(),
            ));
            if paths.is_empty() {
                stats.unmatched += 1;
                continue;
            }
            let local = self.local_cache.entry(e.archetype_hash).or_insert_with(|| {
                paths
                    .iter()
                    .filter_map(|p| crate::vfs::read(p).ok())
                    .filter_map(|b| primitives::triangles(&b, options, counts).ok())
                    .flatten()
                    .collect()
            });
            stats.placed += 1;
            if self.by_mlo.contains_key(&e.archetype_hash) {
                stats.via_manifest += 1;
                let stem = paths[0]
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                stats
                    .manifest_placed
                    .push((stem, [e.position.x, e.position.y, e.position.z]));
            }
            out.extend(local.iter().map(|t| Triangle {
                vertices: t.vertices.map(|v| place(&e, v, invert)),
                material: t.material,
            }));
        }
    }
}

pub fn empty_stats(ymaps: usize) -> Stats {
    Stats {
        ymaps,
        instances: 0,
        placed: 0,
        unmatched: 0,
        via_manifest: 0,
        manifest_placed: Vec::new(),
        all: Vec::new(),
    }
}

/// World-space interior triangles for every MLO placed by the ymaps.
pub fn triangles(
    meta_dir: &Path,
    ybn_dirs: &[PathBuf],
    ybn_layers: Option<&Path>,
    options: primitives::Options,
    invert: bool,
    counts: &mut primitives::Counts,
) -> (Vec<Triangle>, Stats) {
    let mut shells = Shells::load(meta_dir, ybn_dirs, ybn_layers);
    let toggled = script_toggled_ymaps(meta_dir);
    let ymaps = layered(
        &meta_dir.join("base"),
        Some(&meta_dir.join("layers")),
        "ymap",
        |p| !toggled.contains(&stem_hash(p)),
    );
    let mut stats = empty_stats(ymaps.len());
    let mut out = Vec::new();
    for ymap in &ymaps {
        let Ok(bytes) = crate::vfs::read(ymap) else {
            continue;
        };
        shells.place_ymap(&bytes, options, invert, counts, &mut stats, &mut out);
    }
    (out, stats)
}

/// Diagnostic: MLO placements within `r` of (x, y) in every ymap of every
/// layer (not only the winning copy), and the manifest groups holding them.
pub fn near(meta_dir: &Path, x: f32, y: f32, r: f32) {
    let toggled = script_toggled_ymaps(meta_dir);
    let mut groups: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for m in manifests(meta_dir) {
        for g in m.map_data_groups {
            let kind = format!("group {:08x} weather {} hours {:#x}", g.name.hash, g.weather_types.len(), g.hours_on_off);
            for b in &g.bounds {
                groups.entry(b.hash).or_default().push(kind.clone());
            }
            groups.entry(g.name.hash).or_default().push(format!("{kind} (itself)"));
        }
    }
    let mut files = Vec::new();
    walk(&meta_dir.join("base"), "ymap", &mut files);
    walk(&meta_dir.join("layers"), "ymap", &mut files);
    for p in files {
        let Ok(bytes) = crate::vfs::read(&p) else { continue };
        let Ok(instances) = parse_ymap_mlo_instances(&bytes) else { continue };
        for inst in instances {
            let e = inst.entity;
            let d = ((e.position.x - x).powi(2) + (e.position.y - y).powi(2)).sqrt();
            if d < r {
                let h = stem_hash(&p);
                println!(
                    "MLO {:08x} at ({:.1}, {:.1}, {:.1}) d={d:.0} in {} toggled={} {:?}",
                    e.archetype_hash, e.position.x, e.position.y, e.position.z,
                    p.display(), toggled.contains(&h), groups.get(&h)
                );
            }
        }
    }
}

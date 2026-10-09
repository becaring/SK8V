//! Loose GTA `.ybn` collision -> SkateV world cache (SVWC v2, see `rust/svwc`).
//!
//!     skatev-world-cache OUT.svwc [--include-hi] [--census] [--layers DIR] INPUT.ybn|DIR [...]
//!
//! `--layers DIR` applies GTA's own override order on top of the base inputs:
//! `DIR/layers.txt` lists update/patch layers lowest priority first (written by
//! tools/extract-gta-collision-updates.ps1). A later layer's `levels/gta5`
//! YBN replaces a base or earlier file of the same name, as the game does,
//! but only from archives the pack enables (`packs.rs`).
//!
//! `hi@` bounds are GTA's vehicle-only high-detail collision; by default only
//! the regular (ped-facing) bounds are used, matching what a skater stands on.
//! `ma@` bounds are never physics: they are the surfaces procedural grass and
//! litter spawn on, copies of the ground (2.08 M triangles; the collision
//! audit found them doubling every street, 2026-10-05).
//!
//! Primitive polygons (boxes, spheres, capsules, cylinders: building walls,
//! posts, bollards) are triangulated by `primitives.rs`, and only bound
//! children a ped collides with are kept (no foliage, no weapon-, vehicle-,
//! cover- or animal-only bounds). `--no-primitives` and `--all-flags` restore
//! the old behaviour.
mod interiors;
mod packs;
mod primitives;
mod props;
mod vegetation;
mod vehicles;
mod vfs;

use rage_formats::parse_ybn;
use std::{
    env,
    error::Error,
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

fn collect_ybn(
    path: &Path,
    include_hi: bool,
    out: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            collect_ybn(&entry?.path(), include_hi, out)?;
        }
    } else if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ybn"))
    {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if (include_hi || !name.starts_with("hi@")) && !name.starts_with("ma@") {
            out.push(path.to_path_buf());
        }
    }
    Ok(())
}

/// Drops a triangle that repeats an earlier one: every corner within 1 cm
/// of the earlier one's, same winding (a reversed copy is the other side of a
/// thin wall and stays). GTA places some trees and props twice a few
/// millimetres apart and some ground sits in two bound files (collision
/// audit, 2026-10-05). A grid snap missed most of them: copies a few
/// millimetres apart straddle a grid line in one of nine coordinates.
// ponytail: compared within 64 m blocks by centroid, so a pair straddling a
// block edge survives; harmless (it is only a repeat).
use svwc::clean::drop_repeats;

fn file_key(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// World-space map collision inside an update layer: `levels/gta5/...`,
/// never MLO-local interiors.
fn is_map_collision(path: &Path) -> bool {
    let p = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    p.contains("levels/gta5/") && !p.contains("/interiors/")
}

/// Base files keyed by name, then each layer in order replaces or adds.
fn apply_layers(
    base: Vec<PathBuf>,
    dir: &Path,
    include_hi: bool,
) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut by_name: std::collections::BTreeMap<String, PathBuf> =
        base.into_iter().map(|p| (file_key(&p), p)).collect();
    let order = fs::read_to_string(dir.join("layers.txt"))?;
    let (mut replaced, mut added) = (0usize, 0usize);
    for layer in order.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut files = Vec::new();
        let root = dir.join(layer);
        collect_ybn(&root, include_hi, &mut files)?;
        let enabled = packs::enabled(&root);
        let shipped = files.len();
        files.retain(|f| packs::allows(&root, &enabled, f));
        if files.len() < shipped {
            eprintln!("layer {layer}: {} files in archives the pack never mounts", shipped - files.len());
        }
        files.sort();
        let (mut r, mut a) = (0usize, 0usize);
        for f in files.into_iter().filter(|f| is_map_collision(f)) {
            if by_name.insert(file_key(&f), f).is_some() {
                r += 1;
            } else {
                a += 1;
            }
        }
        if r + a > 0 {
            eprintln!("layer {layer}: {r} replaced, {a} added");
        }
        replaced += r;
        added += a;
    }
    eprintln!("update layers: {replaced} base files replaced, {added} added");
    Ok(by_name.into_values().collect())
}

/// Winding evidence: among near-horizontal triangles, how many have a +Z
/// normal under (b - a) x (c - a). Skate expects counterclockwise (outward).
#[derive(Default)]
struct Census {
    horizontal: usize,
    up: usize,
    degenerate: usize,
}

impl Census {
    fn add(&mut self, v: &[[f32; 3]; 3]) {
        let e1 = [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]];
        let e2 = [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-6 {
            self.degenerate += 1;
            return;
        }
        if (n[2] / len).abs() > 0.9 {
            self.horizontal += 1;
            if n[2] > 0.0 {
                self.up += 1;
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // Collision coverage audit of every placement (no cache is written):
    //   skatev-world-cache --audit-props META [x y z radius]
    {
        let mut a: Vec<String> = env::args().skip(1).collect();
        // --templates / --vehicle-bounds reading GTA's archives in place
        // (vfs.rs) instead of an extracted META / FRAGMENTS folder:
        //   skatev-world-cache --templates-gta CACHE.svwc GTA KEYS [VEHICLE_MASSES]
        //   skatev-world-cache --vehicle-bounds-gta CACHE.svwc GTA KEYS VEHICLE_MASSES
        if let Some(mode) = a.first().and_then(|f| f.strip_suffix("-gta")).map(str::to_string) {
            let (Some(gta), Some(keys)) = (a.get(2).map(PathBuf::from), a.get(3).map(PathBuf::from)) else {
                return Err(format!("{mode}-gta CACHE.svwc GTA KEYS ...").into());
            };
            let keys = gta_archives::keys(&keys).map_err(|e| e.to_string())?;
            match mode.as_str() {
                "--templates" => vfs::mount_placements(&gta, keys)?,
                "--vehicle-bounds" => vfs::mount_vehicles(&gta, keys)?,
                _ => return Err(format!("unknown mode {mode}-gta").into()),
            }
            a.splice(0..4, [mode, a[1].clone(), vfs::ROOT.to_string()]);
        }
        if a.first().is_some_and(|f| f == "--fragment-masses") {
            // skatev-world-cache --fragment-masses FILE.yft...
            for f in &a[1..] {
                match props::fragment_masses(std::path::Path::new(f)) {
                    Some((n, p, d)) => println!("{f}: {n} children, pristine {p:.3} damaged {d:.3}"),
                    None => println!("{f}: unreadable"),
                }
            }
            return Ok(());
        }
        if a.first().is_some_and(|f| f == "--vehicle-bounds") {
            // Exact vehicle collision for the contact exchange, added to an
            // existing cache's model templates (CACHE.prop-models + index):
            //   skatev-world-cache --vehicle-bounds CACHE.svwc FRAGMENTS VEHICLE_MASSES
            let [cache, fragments, masses] = [1, 2, 3].map(|i| a.get(i).map(PathBuf::from));
            let (Some(cache), Some(fragments), Some(masses)) = (cache, fragments, masses) else {
                return Err("--vehicle-bounds CACHE.svwc FRAGMENTS VEHICLE_MASSES".into());
            };
            return vehicles::write(&cache, &fragments, &masses);
        }
        if a.first().is_some_and(|f| f == "--templates") {
        // Only what the runtime reads beside the cache: the prop collision
        // templates (CACHE.prop-models + index) and masses, no static map
        // (live collision replaced it) and no .ybn input:
        //   skatev-world-cache --templates CACHE.svwc META [VEHICLE_MASSES]
        let usage = "--templates CACHE.svwc META [VEHICLE_MASSES]";
        let (Some(cache), Some(meta)) = (a.get(1), a.get(2)) else { return Err(usage.into()) };
        let mut counts = primitives::Counts::default();
        let start = std::time::Instant::now();
        let (_, _, st) = props::triangles(Path::new(meta), primitives::Options { primitives: true, all_flags: false }, &mut counts);
        let read = start.elapsed();
        write_templates(Path::new(cache), &st.templates, &st.masses, a.get(3).map(Path::new))?;
        eprintln!("templates: read {:.1} s, write {:.1} s", read.as_secs_f32(), (start.elapsed() - read).as_secs_f32());
        return Ok(());
    }
    if a.first().is_some_and(|f| f == "--which") {
            // Which collision files hold triangles near a point:
            //   skatev-world-cache --which x y z radius DIR...
            let f = |i: usize| a.get(i).and_then(|v| v.parse::<f32>().ok()).ok_or("--which x y z radius DIR...");
            let (p, r) = ([f(1)?, f(2)?, f(3)?], f(4)?);
            let mut files = Vec::new();
            for d in &a[5..] {
                collect_ybn(std::path::Path::new(d), true, &mut files)?;
            }
            let mut prims = primitives::Counts::default();
            for path in &files {
                let Ok(bytes) = fs::read(path) else { continue };
                let Ok(tris) = primitives::triangles(&bytes, primitives::Options { primitives: true, all_flags: true }, &mut prims) else { continue };
                let mut hits = 0;
                let mut mats = std::collections::BTreeMap::new();
                for t in &tris {
                    let c = t.vertices.iter().fold([0.0f32; 3], |acc, v| [acc[0] + v.x / 3.0, acc[1] + v.y / 3.0, acc[2] + v.z / 3.0]);
                    if (c[0] - p[0]).hypot(c[1] - p[1]) < r && (c[2] - p[2]).abs() < r {
                        hits += 1;
                        *mats.entry(t.material).or_insert(0) += 1;
                    }
                }
                if hits > 0 {
                    println!("{hits:6} triangles {mats:?} {}", path.display());
                }
            }
            return Ok(());
        }
        if a.first().is_some_and(|f| f == "--trace") {
            // Every triangle of one file inside a box, with its material and
            // the owning composite child's (type, include) flags:
            //   skatev-world-cache --trace FILE.ybn x0 y0 z0 x1 y1 z1
            let usage = "--trace FILE.ybn x0 y0 z0 x1 y1 z1";
            let f = |i: usize| a.get(i).and_then(|v| v.parse::<f32>().ok()).ok_or(usage);
            let (lo, hi) = ([f(2)?, f(3)?, f(4)?], [f(5)?, f(6)?, f(7)?]);
            let bytes = fs::read(a.get(1).ok_or(usage)?)?;
            let mut counts = primitives::Counts { record_spans: true, ..Default::default() };
            let tris = primitives::triangles(&bytes, primitives::Options { primitives: true, all_flags: true }, &mut counts)?;
            for (i, t) in tris.iter().enumerate() {
                let v = t.vertices.map(|v| [v.x, v.y, v.z]);
                let inside = (0..3).all(|k| v.iter().any(|p| p[k] >= lo[k]) && v.iter().any(|p| p[k] <= hi[k]));
                if !inside {
                    continue;
                }
                let span = counts.spans.partition_point(|s| s.0 <= i).checked_sub(1);
                let (kind, include) = span.map(|s| counts.spans[s].1).unwrap_or((0, 0));
                let n = t.normal();
                println!(
                    "tri {i} mat {} type {kind:#010x} include {include:#010x} keep {} n ({:.2},{:.2},{:.2}) v {:?}",
                    t.material, primitives::skater_collides((kind, include)), n.x, n.y, n.z,
                    v.map(|p| p.map(|c| (c * 100.0).round() / 100.0))
                );
            }
            return Ok(());
        }
        if a.first().is_some_and(|f| f == "--trace-frag") {
            // One vehicle fragment's physics bound, child by child: triangles,
            // extent and the composite child's (type, include) flags:
            //   skatev-world-cache --trace-frag FILE.yft
            let mut counts = primitives::Counts { record_spans: true, ..Default::default() };
            let tris = props::fragment_bound(Path::new(a.get(1).ok_or("--trace-frag FILE.yft")?), primitives::Options { primitives: true, all_flags: true }, &mut counts).ok_or("no fragment bound")?;
            for (s, &(start, (kind, include))) in counts.spans.iter().enumerate() {
                let end = counts.spans.get(s + 1).map_or(tris.len(), |n| n.0);
                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for t in &tris[start..end] {
                    for v in t.vertices {
                        for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                            lo[k] = lo[k].min(c);
                            hi[k] = hi[k].max(c);
                        }
                    }
                }
                println!("child {s}: tris {start}..{end} ({}) type {kind:#010x} include {include:#010x} keep {} extent {:.1} x {:.1} x {:.1}", end - start, primitives::skater_collides((kind, include)), hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]);
            }
            return Ok(());
        }
        if a.first().is_some_and(|f| f == "--map-states") {
            // Script-toggled map states (IPLs) beside an existing cache:
            //   skatev-world-cache --map-states CACHE.svwc META YBN_DIR [YBN_LAYERS]
            let usage = "--map-states CACHE.svwc META YBN_DIR [YBN_LAYERS]";
            let [cache, meta, ybn] = [1, 2, 3].map(|i| a.get(i).map(PathBuf::from));
            let (Some(cache), Some(meta), Some(ybn)) = (cache, meta, ybn) else {
                return Err(usage.into());
            };
            let layers = a.get(4).map(PathBuf::from);
            let options = primitives::Options { primitives: true, all_flags: false };
            let mut counts = primitives::Counts::default();
            let (states, _) = props::map_states(&meta, &[ybn], layers.as_deref(), options, &mut counts);
            return write_map_states(&cache, &states);
        }
        if a.first().is_some_and(|f| f == "--mlo-near") {
            // Every MLO placement near a point, in every ymap (script-toggled
            // map groups included), with the ymap, its layer and whether the
            // interior pass skips it:  skatev-world-cache --mlo-near META x y radius
            let meta = PathBuf::from(a.get(1).ok_or("--mlo-near META x y radius")?);
            let f = |i: usize| a.get(i).and_then(|v| v.parse::<f32>().ok()).ok_or("--mlo-near META x y radius");
            let (x, y, r) = (f(2)?, f(3)?, f(4)?);
            interiors::near(&meta, x, y, r);
            return Ok(());
        }
        if a.first().is_some_and(|f| f == "--audit-props") {
            let meta = PathBuf::from(a.get(1).ok_or("--audit-props META [x y z radius]")?);
            let near = (a.len() >= 6).then(|| {
                let f = |i: usize| a[i].parse::<f32>().unwrap_or(0.0);
                ([f(2), f(3), f(4)], f(5))
            });
            props::audit(&meta, near);
            return Ok(());
        }
    }
    let mut args = env::args_os().skip(1);
    let output = args.next().ok_or(
        "usage: skatev-world-cache OUT.svwc [--include-hi] [--census] [--no-primitives] [--all-flags] INPUT.ybn|DIR [...]",
    )?;
    let mut include_hi = false;
    let mut census_only = false;
    let mut with_primitives = true;
    let mut all_flags = false;
    let mut layers_dir: Option<PathBuf> = None;
    let mut interiors_meta: Option<PathBuf> = None;
    let mut props_meta: Option<PathBuf> = None;
    let mut vehicle_masses: Option<PathBuf> = None;
    let mut interior_ybn: Vec<PathBuf> = Vec::new();
    let mut interior_invert = false;
    let mut probe: Option<(f32, f32)> = None;
    let mut roots = Vec::new();
    let mut args = args.peekable();
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--include-hi") => include_hi = true,
            Some("--census") => census_only = true,
            Some("--no-primitives") => with_primitives = false,
            Some("--all-flags") => all_flags = true,
            Some("--layers") => layers_dir = args.next().map(PathBuf::from),
            Some("--interiors") => interiors_meta = args.next().map(PathBuf::from),
            Some("--props") => props_meta = args.next().map(PathBuf::from),
            Some("--vehicle-masses") => vehicle_masses = args.next().map(PathBuf::from),
            Some("--interior-ybn") => interior_ybn.extend(args.next().map(PathBuf::from)),
            Some("--interior-invert") => interior_invert = true,
            Some("--interior-probe") => {
                let x = args.next().and_then(|v| v.to_str().and_then(|v| v.parse().ok()));
                let y = args.next().and_then(|v| v.to_str().and_then(|v| v.parse().ok()));
                probe = x.zip(y);
            }
            _ => roots.push(PathBuf::from(a)),
        }
    }
    if roots.is_empty() {
        return Err("at least one .ybn file or directory is required".into());
    }

    let mut inputs = Vec::new();
    for root in &roots {
        collect_ybn(root, include_hi, &mut inputs)?;
    }
    inputs.sort();
    inputs.dedup();
    if let Some(dir) = &layers_dir {
        inputs = apply_layers(inputs, dir, include_hi)?;
    }
    if inputs.is_empty() {
        return Err("no .ybn files found under the supplied inputs".into());
    }
    // Interior (MLO-local) bounds named by the manifests are placed by the
    // interior pass; read as world space they landed near the map origin and
    // the real spot had no floor (Portola Drive station, owner screenshot).
    if let Some(meta) = &interiors_meta {
        let local = interiors::interior_bound_hashes(meta);
        let before = inputs.len();
        inputs.retain(|p| !local.contains(&interiors::stem_hash(p)));
        eprintln!("{} interior bound files moved from the world pass to the interior pass", before - inputs.len());
    }

    let mut triangles = Vec::new();
    let mut census = Census::default();
    let mut failed = 0usize;
    let mut prims = primitives::Counts::default();
    let mut prim_triangles = 0usize;
    for path in &inputs {
        let bytes = fs::read(path)?;
        // rage-formats validates the file; the walk below emits its triangles.
        if let Err(e) = parse_ybn(&bytes) {
            failed += 1;
            eprintln!("skip {}: {e:#}", path.display());
            continue;
        }
        let options = primitives::Options { primitives: with_primitives, all_flags };
        let tris = match primitives::triangles(&bytes, options, &mut prims) {
            Ok(t) => t,
            Err(e) => {
                failed += 1;
                eprintln!("skip {}: {e}", path.display());
                continue;
            }
        };
        prim_triangles += tris.len();
        for t in tris {
            let v = t.vertices.map(|p| [p.x, p.y, p.z]);
            census.add(&v);
            triangles.push(svwc::Tri {
                v,
                material: t.material,
            });
        }
    }
    eprintln!(
        "{} YBN files ({} failed), {} triangles; near-horizontal {}: {} +Z ({:.1}%), degenerate {}",
        inputs.len(),
        failed,
        triangles.len(),
        census.horizontal,
        census.up,
        100.0 * census.up as f64 / census.horizontal.max(1) as f64,
        census.degenerate,
    );
    eprintln!(
        "polygons kept {} dropped {} by child flags; primitives {} boxes, {} spheres, {} capsules, {} cylinders, {} skipped, {} vegetation volumes left out; {prim_triangles} triangles out",
        prims.kept_polygons,
        prims.dropped_polygons,
        prims.boxes,
        prims.spheres,
        prims.capsules,
        prims.cylinders,
        prims.skipped,
        prims.vegetation
    );
    let mut flags: Vec<_> = prims.by_flags.iter().collect();
    flags.sort_by(|a, b| b.1.cmp(a.1));
    for ((t, i), n) in flags.iter().take(16) {
        let verdict = if primitives::skater_collides((*t, *i)) { "kept" } else { "dropped" };
        eprintln!("  polygons with child flags type {t:#010x} include {i:#010x}: {n} ({verdict})");
    }
    if let Some(meta) = &interiors_meta {
        let options = primitives::Options { primitives: with_primitives, all_flags };
        let mut icounts = primitives::Counts::default();
        let (tris, st) = interiors::triangles(
            meta,
            &interior_ybn,
            layers_dir.as_deref(),
            options,
            interior_invert,
            &mut icounts,
        );
        eprintln!(
            "interiors: {} ymaps, {} MLO placements, {} placed, {} without a bound; {} triangles",
            st.ymaps,
            st.instances,
            st.placed,
            st.unmatched,
            tris.len()
        );
        eprintln!("  {} placed through manifest interior lists", st.via_manifest);
        for (stem, at) in st.manifest_placed.iter().filter(|(s, _)| s.contains("metro") || s.contains("sewer") || s.contains("tunnel")).take(40) {
            eprintln!("    {stem} at ({:.0}, {:.0}, {:.0})", at[0], at[1], at[2]);
        }
        if let Some((px, py)) = probe {
            for (h, at, found, wanted) in &st.all {
                let d = ((at[0] - px).powi(2) + (at[1] - py).powi(2)).sqrt();
                if d < 80.0 {
                    eprintln!("  MLO {h:08x} at ({:.0}, {:.0}, {:.0}) d={d:.0}: bounds {found:?} of {wanted} listed", at[0], at[1], at[2]);
                }
            }
            // Upward faces under (px, py): their heights show whether the
            // interior floor lines up with the exterior around it.
            let mut zs: Vec<f32> = tris
                .iter()
                .filter_map(|t| {
                    let [a, b, c] = t.vertices;
                    let n = (b - a).cross(c - a);
                    if n.z <= 0.0 {
                        return None;
                    }
                    // Barycentric containment in XY.
                    let d = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
                    if d.abs() < 1e-9 {
                        return None;
                    }
                    let u = ((px - a.x) * (c.y - a.y) - (c.x - a.x) * (py - a.y)) / d;
                    let v = ((b.x - a.x) * (py - a.y) - (px - a.x) * (b.y - a.y)) / d;
                    (u >= 0.0 && v >= 0.0 && u + v <= 1.0).then(|| a.z + u * (b.z - a.z) + v * (c.z - a.z))
                })
                .collect();
            zs.sort_by(f32::total_cmp);
            eprintln!("interior floors under ({px}, {py}): {zs:?}");
        }
        for t in tris {
            let v = t.vertices.map(|p| [p.x, p.y, p.z]);
            census.add(&v);
            triangles.push(svwc::Tri { v, material: t.material });
        }
        if !census_only {
            let mut scounts = primitives::Counts::default();
            let (states, _) = props::map_states(meta, &interior_ybn, layers_dir.as_deref(), options, &mut scounts);
            write_map_states(std::path::Path::new(&output), &states)?;
        }
    }
    let mut baked_props = std::collections::BTreeSet::new();
    let mut baked_instances = Vec::new();
    let mut prop_templates = std::collections::BTreeMap::new();
    let mut prop_masses = std::collections::BTreeMap::new();
    if let Some(meta) = &props_meta {
        let options = primitives::Options { primitives: with_primitives, all_flags };
        let mut pcounts = primitives::Counts::default();
        let (tris, baked, st) = props::triangles(meta, options, &mut pcounts);
        eprintln!(
            "props: {} archetypes, {} prop models, {} entities, {} placed + {} interior furniture, {} prop models without a bound; {} archetypes baked, {} triangles",
            st.archetypes,
            st.models,
            st.entities,
            st.placed,
            st.mlo_furniture,
            st.without_bound,
            baked.len(),
            tris.len()
        );
        for t in tris {
            let v = t.vertices.map(|p| [p.x, p.y, p.z]);
            census.add(&v);
            triangles.push(svwc::Tri { v, material: t.material });
        }
        baked_props = baked;
        baked_instances = st.instances;
        prop_templates = st.templates;
        prop_masses = st.masses;
    }
    if census_only {
        return Ok(());
    }
    if !all_flags {
        println!("{} foliage mesh triangles over ground left out", vegetation::drop_over_ground(&mut triangles));
    }
    println!("{} repeated triangles left out", drop_repeats(&mut triangles));
    let file = fs::File::create(&output)?;
    let mut out = BufWriter::new(file);
    let records = svwc::write(&mut out, &triangles, svwc::DEFAULT_TILE_SIZE)?;
    out.flush()?;
    if props_meta.is_some() {
        // Retain the old model inventory for diagnostics. Suppression must use
        // the exact instance sidecar: same-model moved objects still collide.
        let list = PathBuf::from(&output).with_extension("props.txt");
        let text: String = baked_props.iter().map(|h| format!("{h:08x}\n")).collect();
        fs::write(&list, text)?;
        eprintln!("baked prop archetypes -> {}", list.display());
        let list = PathBuf::from(&output).with_extension("prop-instances.txt");
        let mut text = String::from("# SkateV baked prop instances v1: model px py pz right.xyz forward.xyz up.xyz\n");
        for instance in &baked_instances { text.push_str(&instance.line()); }
        fs::write(&list, text)?;
        eprintln!("{} baked prop instances -> {}", baked_instances.len(), list.display());
        write_templates(Path::new(&output), &prop_templates, &prop_masses, vehicle_masses.as_deref())?;
    }
    eprintln!(
        "wrote {records} tile records to {}",
        PathBuf::from(output).display()
    );
    Ok(())
}

/// The prop collision model templates (`CACHE.prop-models/<hash>.svwc` and
/// the index `CACHE.prop-models.txt`) and `CACHE.prop-masses.txt`.
fn write_templates(
    cache: &Path,
    prop_templates: &std::collections::BTreeMap<u32, Vec<rage_formats::Triangle>>,
    prop_masses: &std::collections::BTreeMap<u32, f32>,
    vehicle_masses: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let directory = cache.to_path_buf().with_extension("prop-models");
    fs::create_dir_all(&directory)?;
    let mut index = String::from("# SkateV local collision model templates v1: archetype hash\n");
    for model in prop_templates.keys() {
        index.push_str(&format!("{model:08x}\n"));
    }
    // Several writers: creating each new small file is slow on Windows (79 s
    // for 7k files into a fresh directory, 25 s overwriting), not the encoding.
    let models: Vec<_> = prop_templates.iter().collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(16));
    std::thread::scope(|s| {
        let workers: Vec<_> = models
            .chunks(models.len().div_ceil(threads).max(1))
            .map(|chunk| {
                let directory = &directory;
                s.spawn(move || -> std::io::Result<()> {
                    for (model, local) in chunk {
                        let local: Vec<_> = local.iter().map(|t| svwc::Tri {
                            v: t.vertices.map(|p| [p.x, p.y, p.z]), material: t.material,
                        }).collect();
                        let mut writer = BufWriter::new(fs::File::create(directory.join(format!("{model:08x}.svwc")))?);
                        svwc::write(&mut writer, &local, svwc::DEFAULT_TILE_SIZE)?;
                        writer.flush()?;
                    }
                    Ok(())
                })
            })
            .collect();
        workers.into_iter().try_for_each(|w| w.join().expect("template writer panicked"))
    })?;
    fs::write(cache.to_path_buf().with_extension("prop-models.txt"), index)?;
    eprintln!("{} collision model templates -> {}", prop_templates.len(), directory.display());
    // Masses for the contact exchange: authored fragment masses, then
    // vehicles from handling.meta (tools/build-vehicle-masses.py), which win.
    let mut text = String::from("# SkateV model masses v1: model mass_kg inertia_multiplier.xyz centre_of_mass_offset.xyz\n");
    let mut rows: std::collections::BTreeMap<u32, String> = prop_masses
        .iter()
        .map(|(m, kg)| (*m, format!("{m:08x} {kg:.3} 1 1 1 0 0 0\n")))
        .collect();
    if let Some(path) = vehicle_masses {
        for line in fs::read_to_string(path)?.lines().filter(|l| !l.starts_with('#')) {
            let fields: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
            if fields.len() == 8 {
                if let Ok(model) = u32::from_str_radix(fields[0], 16) {
                    rows.insert(model, format!("{}\n", fields.join(" ")));
                }
            }
        }
    }
    for row in rows.values() { text.push_str(row); }
    fs::write(cache.to_path_buf().with_extension("prop-masses.txt"), text)?;
    eprintln!("{} model masses ({} fragments) -> prop-masses.txt", rows.len(), prop_masses.len());
    Ok(())
}

/// Script-toggled map states beside the cache: one small cache per state
/// in `CACHE.map-states/NAME.svwc` and the index `CACHE.map-states.txt`
/// (name, triangle count, world-space bounding box). The runtime adds a
/// state's triangles only while the host reports GTA has it active.
fn write_map_states(cache: &std::path::Path, states: &[props::MapState]) -> Result<(), Box<dyn std::error::Error>> {
    let directory = cache.with_extension("map-states");
    fs::create_dir_all(&directory)?;
    let mut index = String::from("# SkateV map states v1: name triangles min.xyz max.xyz\n");
    let mut total = 0;
    for s in states {
        let tris: Vec<_> = s.triangles.iter().map(|t| svwc::Tri {
            v: t.vertices.map(|p| [p.x, p.y, p.z]), material: t.material,
        }).collect();
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in tris.iter().flat_map(|t| t.v) {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        let mut writer = BufWriter::new(fs::File::create(directory.join(format!("{}.svwc", s.name)))?);
        svwc::write(&mut writer, &tris, svwc::DEFAULT_TILE_SIZE)?;
        writer.flush()?;
        index.push_str(&format!("{} {} {} {} {} {} {} {}\n", s.name, tris.len(), lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]));
        total += tris.len();
    }
    fs::write(cache.with_extension("map-states.txt"), index)?;
    eprintln!("{} map states with collision ({total} triangles) -> {}", states.len(), directory.display());
    Ok(())
}

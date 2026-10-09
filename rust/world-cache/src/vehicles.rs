//! Exact vehicle collision for the contact exchange (owner run 2026-10-03:
//! cars had no model template, so the runtime fell back to the entity's
//! bounding box and the board struck its square corners and its underside,
//! where a real car has ground clearance).
//!
//! Input: the user's vehicle fragments, extracted by
//! tools/extract-gta-vehicles.ps1 into FRAGMENTS/<layer>/... (00_common =
//! x64e.rpf, 50_<pack> = DLC packs, 99_update* = update archives). Layers
//! apply in name order, a later one replacing a model, as the game loads
//! them. Only models listed in VEHICLE_MASSES (tools/build-vehicle-masses.py,
//! every vehicles.meta modelName) are written; render-only `_hi` fragments are
//! never used. Each model's pristine fragment physics bound (the bound GTA
//! itself collides with, props::fragment_bound) is written in model space as
//! CACHE.prop-models/<model>.svwc and listed in CACHE.prop-models.txt, where
//! the runtime already looks for dynamic model templates.
use crate::primitives::{Counts, Options};
use rage_formats::rage_joaat;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

fn fragments(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            fragments(&p, out)?;
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("yft")) {
            out.push(p);
        }
    }
    Ok(())
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// A vehicle's fragment carries one closed box (12 triangles) spanning its
/// whole footprint, which collided as a solid brick around the real shapes:
/// a plane's wings and fuselage (owner 2026-10-06: "the plane had a big
/// invisible box around it"), a car's hood under a roof-high lid (owner
/// 2026-10-07: could not jump on the hood). Left out when the other parts
/// span the vehicle too (a body that is only a box keeps it).
type Part = Vec<rage_formats::Triangle>;

fn drop_covering_box(parts: Vec<Part>) -> (Vec<Part>, usize) {
    let extent = |tris: &[rage_formats::Triangle]| {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for t in tris {
            for v in t.vertices {
                for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                    lo[k] = lo[k].min(c);
                    hi[k] = hi[k].max(c);
                }
            }
        }
        [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]
    };
    let whole = extent(&parts.concat());
    let covers = |e: [f32; 3]| e[0] >= 0.85 * whole[0] && e[1] >= 0.85 * whole[1] && e[2] >= 0.5 * whole[2];
    let is_box: Vec<bool> = parts.iter().map(|p| p.len() == 12 && covers(extent(p)) && parts.len() > 3).collect();
    let rest: Vec<_> = parts.iter().zip(&is_box).filter(|(_, b)| !**b).flat_map(|(p, _)| p.iter().cloned()).collect();
    let boxes = is_box.iter().filter(|&&b| b).count();
    let r = extent(&rest);
    if boxes == 0 || !(r[0] >= 0.85 * whole[0] && r[1] >= 0.85 * whole[1]) {
        return (parts, 0);
    }
    (parts.into_iter().zip(is_box).filter(|(_, b)| !b).map(|(p, _)| p).collect(), boxes)
}

/// Breakable panels (bonnet, boot lid) are closed slabs inside the body,
/// their tops flush with its surface, and some parts come twice; their edges
/// cross the body's surface where the wheels catch (owner 2026-10-07: the
/// board stuck on a Futo's hood, whose bonnet top is the body's at 0.38 m).
/// Largest first, a part goes when every corner, edge middle and centroid of
/// its triangles is inside, or within 2 cm of, a part already kept (generalized winding number >= 0.5).
fn drop_contained(parts: Vec<Part>) -> (Vec<Part>, usize) {
    type P = [f64; 3];
    let v = |p: rage_formats::Vec3| [p.x as f64, p.y as f64, p.z as f64];
    let sub = |a: P, b: P| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: P, b: P| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: P, b: P| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let len = |a: P| dot(a, a).sqrt();
    // Winding number of `part` around `q` (van Oosterom-Strackee solid angles).
    let winding = |part: &Part, q: P| -> f64 {
        part.iter().map(|t| {
            let [a, b, c] = t.vertices.map(|p| sub(v(p), q));
            let (la, lb, lc) = (len(a), len(b), len(c));
            let num = dot(a, cross(b, c));
            let den = la * lb * lc + dot(a, b) * lc + dot(b, c) * la + dot(c, a) * lb;
            2.0 * num.atan2(den)
        }).sum::<f64>() / (4.0 * std::f64::consts::PI)
    };
    // Whether `q` is within 2 cm of `part`'s surface (plane distance inside
    // the triangle's slab, a cheap stand-in for exact point-triangle distance).
    let near = |part: &Part, q: P| part.iter().any(|t| {
        let [a, b, c] = t.vertices.map(v);
        let n = cross(sub(b, a), sub(c, a));
        let l = len(n);
        if l < 1e-12 { return false; }
        let d = dot(n, sub(q, a)) / l;
        let inside = [(a, b), (b, c), (c, a)].iter().all(|&(e0, e1)| dot(cross(sub(e1, e0), sub(q, e0)), n) >= -0.02 * l * len(sub(e1, e0)));
        d.abs() <= 0.02 && inside
    });
    let volume = |p: &Part| {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for t in p { for q in t.vertices { for (k, c) in [q.x, q.y, q.z].into_iter().enumerate() { lo[k] = lo[k].min(c); hi[k] = hi[k].max(c); } } }
        (hi[0] - lo[0]) * (hi[1] - lo[1]) * (hi[2] - lo[2])
    };
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by(|&a, &b| volume(&parts[b]).total_cmp(&volume(&parts[a])));
    let mut keep = vec![false; parts.len()];
    for &i in &order {
        let contained = order.iter().filter(|&&j| keep[j]).any(|&j| {
            parts[i].iter().flat_map(|t| {
                let [a, b, c] = t.vertices.map(v);
                let mid = |p: P, q: P| [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0, (p[2] + q[2]) / 2.0];
                [a, b, c, mid(a, b), mid(b, c), mid(c, a), [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0]]
            }).all(|q| winding(&parts[j], q) >= 0.5 || near(&parts[j], q))
        });
        keep[i] = !contained;
    }
    let dropped = keep.iter().filter(|&&k| !k).count();
    (parts.into_iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| p).collect(), dropped)
}

pub fn write(cache: &Path, root: &Path, masses: &Path) -> Result<(), Box<dyn Error>> {
    let wanted: BTreeMap<u32, String> = std::fs::read_to_string(masses)?
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let model = u32::from_str_radix(l.split_whitespace().next()?, 16).ok()?;
            let name = l.split('#').nth(1).and_then(|c| c.split_whitespace().next()).unwrap_or("?");
            Some((model, name.to_string()))
        })
        .collect();
    // Layer order: directory names sort as the game applies them.
    let mut layers: Vec<_> = std::fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
    layers.retain(|e| e.path().is_dir());
    layers.sort_by_key(|e| e.file_name());
    let mut chosen: BTreeMap<u32, PathBuf> = BTreeMap::new();
    for layer in &layers {
        let mut files = Vec::new();
        fragments(&layer.path(), &mut files)?;
        for f in files {
            let name = stem(&f);
            if name.ends_with("_hi") {
                continue;
            }
            let model = rage_joaat(&name);
            if wanted.contains_key(&model) {
                chosen.insert(model, f);
            }
        }
    }
    let directory = cache.with_extension("prop-models");
    std::fs::create_dir_all(&directory)?;
    let index_path = cache.with_extension("prop-models.txt");
    let mut index: BTreeSet<u32> = std::fs::read_to_string(&index_path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| u32::from_str_radix(l.trim(), 16).ok())
        .collect();
    let (mut written, mut unreadable, mut boxes, mut inner) = (0usize, Vec::new(), 0usize, 0usize);
    for (model, path) in &chosen {
        let options = Options { primitives: true, all_flags: false };
        let mut parts = Counts { record_spans: true, ..Default::default() };
        let Some(triangles) = crate::props::fragment_bound(path, options, &mut parts) else {
            unreadable.push(stem(path));
            continue;
        };
        let split: Vec<Part> = (0..parts.spans.len())
            .map(|s| triangles[parts.spans[s].0..parts.spans.get(s + 1).map_or(triangles.len(), |n| n.0)].to_vec())
            .collect();
        let (split, dropped) = drop_covering_box(split);
        boxes += dropped;
        let (split, n) = drop_contained(split);
        inner += n;
        let triangles = split.concat();
        let local: Vec<_> = triangles
            .iter()
            .map(|t| svwc::Tri { v: t.vertices.map(|p| [p.x, p.y, p.z]), material: t.material })
            .collect();
        let mut writer = BufWriter::new(std::fs::File::create(directory.join(format!("{model:08x}.svwc")))?);
        svwc::write(&mut writer, &local, svwc::DEFAULT_TILE_SIZE)?;
        writer.flush()?;
        index.insert(*model);
        written += 1;
    }
    let mut text = String::from("# SkateV local collision model templates v1: archetype hash\n");
    for model in &index {
        text.push_str(&format!("{model:08x}\n"));
    }
    std::fs::write(&index_path, text)?;
    let missing: Vec<&str> = wanted
        .iter()
        .filter(|(m, _)| !chosen.contains_key(m))
        .map(|(_, n)| n.as_str())
        .collect();
    eprintln!(
        "vehicle bounds: {written} written ({boxes} covering boxes, {inner} parts inside others left out), {} unreadable {:?}, {} vehicles.meta models without a fragment{} -> {}",
        unreadable.len(),
        unreadable,
        missing.len(),
        if missing.is_empty() { String::new() } else { format!(" (e.g. {:?})", &missing[..missing.len().min(12)]) },
        directory.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cuboid(lo: [f32; 3], hi: [f32; 3]) -> Vec<rage_formats::Triangle> {
        let p = |x: f32, y: f32, z: f32| rage_formats::Vec3 { x, y, z };
        let t = |a, b, c| rage_formats::Triangle { vertices: [a, b, c], material: 116 };
        let c: Vec<_> = (0..8).map(|i| p([lo[0], hi[0]][i & 1], [lo[1], hi[1]][i >> 1 & 1], [lo[2], hi[2]][i >> 2 & 1])).collect();
        [[0, 1, 3], [0, 3, 2], [4, 6, 7], [4, 7, 5], [0, 4, 5], [0, 5, 1], [2, 3, 7], [2, 7, 6], [0, 2, 6], [0, 6, 4], [1, 5, 7], [1, 7, 3]]
            .iter()
            .map(|&[a, b, d]| t(c[a], c[d], c[b]))
            .collect()
    }

    #[test]
    fn covering_box_goes_unless_it_is_the_body() {
        // A car: hull (here a long low slab and a cabin), a wheel and a roof-high lid.
        let mut tris = cuboid([-0.9, -2.4, -0.4], [0.9, 2.4, 0.1]);
        tris.extend(cuboid([-0.8, -1.2, 0.1], [0.8, 0.4, 0.8]));
        tris.extend(cuboid([0.7, 1.0, -0.6], [0.9, 1.6, 0.0]));
        let lid = tris.len();
        tris.extend(cuboid([-0.92, -2.38, -0.42], [0.92, 2.34, 0.78]));
        let (kept, dropped) = drop_covering_box(tris.chunks(12).map(|c| c.to_vec()).collect());
        assert_eq!((kept.concat().len(), dropped), (lid, 1), "the lid goes");
        // A trailer whose body is the box: kept.
        let mut trailer = cuboid([-1.2, -6.0, 0.0], [1.2, 6.0, 3.0]);
        for x in [-1.0, 0.8] {
            trailer.extend(cuboid([x, -5.0, -0.6], [x + 0.2, -4.0, 0.0]));
        }
        trailer.extend(cuboid([-0.1, 5.0, -0.6], [0.1, 6.0, 0.0]));
        let (kept, dropped) = drop_covering_box(trailer.chunks(12).map(|c| c.to_vec()).collect());
        assert_eq!((kept.concat(), dropped), (trailer, 0));
    }

    #[test]
    fn flush_bonnet_and_copies_go_wheels_and_doors_stay() {
        let body = cuboid([-0.9, -2.0, -0.3], [0.9, 2.0, 0.38]);
        let bonnet = cuboid([-0.75, 0.7, 0.2], [0.75, 2.0, 0.38]);
        let wheel = cuboid([0.7, 1.0, -0.5], [0.9, 1.6, 0.1]);
        let door = cuboid([0.85, -0.5, -0.2], [0.95, 0.7, 0.3]);
        let (kept, dropped) = drop_contained(vec![bonnet, body.clone(), wheel.clone(), body.clone(), door.clone()]);
        assert_eq!(dropped, 2, "the bonnet and the second body go");
        assert_eq!(kept, vec![body, wheel, door]);
    }
}

//! Static prop collision placed into world space.
//!
//! Props (`levels/gta5/props`: ramps, benches, planters, rails) carry their
//! collision as the bound embedded in their drawable (`gtaDrawable` +0xC8);
//! the `.ymap` entities and each MLO's furniture (the `.ytyp` MLO entity
//! lists, placed by the `CMloInstanceDef`) say where they stand. Owner run
//! 07:08: the Koreatown skate ramps (`prop_skate_halfpipe`,
//! `prop_skate_quartpipe` from `v_sports`) had no collision.
//!
//! Fragments (`.yft`) supply pristine model-space collision templates only;
//! they are never permanently baked. Exact baked instance transforms are written
//! next to the cache so only the matching static instance suppresses a host box.
//! A moved/spawned object with the same archetype must keep its dynamic collision.
use crate::interiors::{self, layered};
use crate::primitives::{self, Counts, Options};
use rage_formats::resource::{prepare_rsc7, u64_le, vec3_le, ResReader, SYSTEM_BASE};
use rage_formats::{parse_ymap_entities, parse_ymap_mlo_instances, parse_ytyp, rage_joaat, Archetype, Triangle, YmapEntity};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(Default, Debug)]
pub struct Stats {
    pub archetypes: usize,
    pub models: usize,
    pub entities: usize,
    pub placed: usize,
    pub mlo_furniture: usize,
    pub without_bound: usize,
    pub dynamic_doors: usize,
    pub instances: Vec<BakedInstance>,
    /// Owned model-space geometry keyed by GTA archetype/model identity.
    pub templates: BTreeMap<u32, Vec<Triangle>>,
    /// Authored fragment mass (sum of physics LOD1 children's pristine
    /// masses), keyed like `templates`. Vehicles use handling.meta instead.
    pub masses: BTreeMap<u32, f32>,
}

/// The transform of an instance actually emitted into the collision cache.
/// GTA model identity is the placed archetype hash, not a shared drawable hash.
#[derive(Clone, Debug)]
pub struct BakedInstance {
    pub model: u32,
    pub position: rage_formats::math::Vec3,
    pub axes: [rage_formats::math::Vec3; 3],
}
impl BakedInstance {
    fn from_placement(model: u32, world: &dyn Fn(rage_formats::math::Vec3) -> rage_formats::math::Vec3) -> Self {
        use rage_formats::math::Vec3;
        let position = world(Vec3::new(0.0, 0.0, 0.0));
        Self {
            model, position,
            axes: [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)]
                .map(|v| world(v) - position),
        }
    }
    /// Locale-independent, round-trip f32 text. Position, then right/forward/up.
    pub fn line(&self) -> String {
        let [r, f, u] = self.axes;
        let p = self.position;
        format!("{:08x} {} {} {} {} {} {} {} {} {} {} {} {}\n",
            self.model, p.x, p.y, p.z, r.x, r.y, r.z, f.x, f.y, f.z, u.x, u.y, u.z)
    }
}

/// Drawable bound pointer in the 208-byte `gtaDrawable`.
const DRAWABLE_BOUND: usize = 0xC8;

fn stem_hash(p: &Path) -> u32 {
    let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    rage_joaat(name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s))
}

fn drawable_bound(path: &Path, options: Options, counts: &mut Counts) -> Option<Vec<Triangle>> {
    let bytes = crate::vfs::read(path).ok()?;
    let (system, graphics) = prepare_rsc7(&bytes).ok()?;
    let r = ResReader { system: &system, graphics: &graphics };
    let head = r.resolve(SYSTEM_BASE, 0xD0)?;
    let bound = u64_le(head, DRAWABLE_BOUND);
    if bound == 0 {
        return None;
    }
    let tris = primitives::triangles_at(&r, bound, options, counts);
    (!tris.is_empty()).then_some(tris)
}

/// Pristine fragment physics in entity/model space. Format authority:
/// pinned rage-formats yft.rs (root/LOD pointers), CodeWalker Frag.cs
/// (PhysicsLOD.BoundPointer +0xE8, FragDrawable.BoundPointer +0xF0).
/// Owned trailers/bins confirm the composite's child transforms already
/// include their model-space placement: adding LOD.PositionOffset again
/// would double-translate them. Broken-off/articulated parts are not tracked.
pub fn fragment_bound(path: &Path, options: Options, counts: &mut Counts) -> Option<Vec<Triangle>> {
    let bytes = crate::vfs::read(path).ok()?;
    let (system, graphics) = prepare_rsc7(&bytes).ok()?;
    fragment_bound_reader(&ResReader { system: &system, graphics: &graphics }, options, counts)
}

fn fragment_bound_reader(r: &ResReader<'_>, options: Options, counts: &mut Counts) -> Option<Vec<Triangle>> {
    let root = r.resolve(SYSTEM_BASE, 0xf8)?;
    let bound = r.resolve(u64_le(root, 0xf0), 0x18)
        .and_then(|group| r.resolve(u64_le(group, 0x10), 0xf0))
        .map_or(0, |lod| u64_le(lod, 0xe8));
    if bound != 0 {
        let triangles = primitives::triangles_at(r, bound, options, counts);
        return (!triangles.is_empty()).then_some(triangles);
    }
    // Some non-physical fragments keep the bound on the main drawable.
    // Only the verified identity frame is accepted: no guessed transform
    // for extra/damaged drawables with a nontrivial fragment matrix.
    let drawable = r.resolve(u64_le(root, 0x30), 0xf8)?;
    let frame = rage_formats::BoundTransform {
        columns: [vec3_le(drawable,0xb0),vec3_le(drawable,0xc0),
            vec3_le(drawable,0xd0),vec3_le(drawable,0xe0)],
    };
    if !frame.is_identity() { return None; }
    let triangles = primitives::triangles_at(r, u64_le(drawable,0xf0), options, counts);
    (!triangles.is_empty()).then_some(triangles)
}

/// Fragment physics mass candidates for layout verification (diagnostic):
/// per physics LOD1 child the f32 words at +0x08/+0x0C (PristineMass /
/// DamagedMass in the CodeWalker reference layout, before the group index the
/// pinned reader uses at +0x10), summed over children.
pub fn fragment_masses(path: &Path) -> Option<(usize, f32, f32)> {
    let bytes = crate::vfs::read(path).ok()?;
    let (system, graphics) = prepare_rsc7(&bytes).ok()?;
    let r = ResReader { system: &system, graphics: &graphics };
    let root = r.resolve(SYSTEM_BASE, 0xf8)?;
    let group = r.resolve(u64_le(root, 0xf0), 0x18)?;
    let lod = r.resolve(u64_le(group, 0x10), 0x130)?;
    let children = u64_le(lod, 0xD0);
    let count = lod[0x11D] as usize;
    let mut pristine = 0.0f32;
    let mut damaged = 0.0f32;
    for i in 0..count {
        let list = r.resolve(children + 8 * i as u64, 8)?;
        let child = r.resolve(u64_le(list, 0), 0x18)?;
        pristine += f32::from_le_bytes(child[0x08..0x0C].try_into().ok()?);
        damaged += f32::from_le_bytes(child[0x0C..0x10].try_into().ok()?);
    }
    Some((count, pristine, damaged))
}

/// Plain entity placement (scale, stored-inverse rotation, translation).
fn entity_world(e: &YmapEntity, v: rage_formats::math::Vec3) -> rage_formats::math::Vec3 {
    e.to_world(v)
}

/// Archetypes, MLO definitions and prop drawables (later layers override by
/// file name), shared by the static bake and the map-state layers.
pub struct Library {
    pub arch: HashMap<u32, Archetype>,
    pub mlos: HashMap<u32, Vec<YmapEntity>>,
    door_archetypes: std::collections::HashSet<u32>,
    mlo_doors: HashMap<u32, interiors::doors::Metadata>,
    pub models: BTreeMap<u32, PathBuf>,
    /// Model bounds decoded so far (None: the drawable has no bound).
    bounds: HashMap<u32, Option<Vec<Triangle>>>,
}

impl Library {
    pub fn load(meta_dir: &Path) -> Self {
        let base = meta_dir.join("base");
        let layers = meta_dir.join("layers");
        let mut arch: HashMap<u32, Archetype> = HashMap::new();
        let mut mlos: HashMap<u32, Vec<YmapEntity>> = HashMap::new();
        let mut door_archetypes = std::collections::HashSet::new();
        let mut mlo_doors = HashMap::new();
        for p in layered(&base, Some(&layers), "ytyp", |_| true) {
            let Ok(bytes) = crate::vfs::read(&p) else { continue };
            let Ok(y) = parse_ytyp(&bytes) else { continue };
            let doors = interiors::doors::read(&bytes).unwrap_or_else(|e| panic!("door metadata {}: {e}", p.display()));
            for a in y.archetypes {
                if doors.archetypes.contains(&a.name_hash) { door_archetypes.insert(a.name_hash); }
                else { door_archetypes.remove(&a.name_hash); }
                arch.insert(a.name_hash, a);
            }
            for m in y.mlos {
                mlo_doors.insert(m.name_hash, doors.clone());
                mlos.insert(m.name_hash, m.entities);
            }
        }
        let models: BTreeMap<u32, PathBuf> =
            layered(&base, Some(&layers), "ydr", |_| true).into_iter().map(|p| (stem_hash(&p), p)).collect();
        Library { arch, mlos, door_archetypes, mlo_doors, models, bounds: HashMap::new() }
    }

    /// One placement's world-space collision, if it is a static drawable
    /// with a bound. Moving door leaves remain model templates; neither their
    /// closed triangles nor baked-instance suppression records may be emitted.
    fn place(&mut self, e: &YmapEntity, entity_door: bool,
             to_world: &dyn Fn(rage_formats::math::Vec3) -> rage_formats::math::Vec3,
             options: Options, counts: &mut Counts, out: &mut Vec<Triangle>, stats: &mut Stats,
             baked: &mut BTreeSet<u32>) -> bool {
        if entity_door || self.door_archetypes.contains(&e.archetype_hash) {
            stats.dynamic_doors += 1;
            return false;
        }
        let Some(a) = self.arch.get(&e.archetype_hash) else { return false };
        if a.asset_type != Archetype::ASSET_TYPE_DRAWABLE {
            return false;
        }
        let Some(path) = self.models.get(&a.model_hash()) else { return false };
        let tris = self.bounds.entry(a.model_hash()).or_insert_with(|| drawable_bound(path, options, counts));
        let Some(tris) = tris else {
            stats.without_bound += 1;
            return false;
        };
        baked.insert(e.archetype_hash);
        stats.instances.push(BakedInstance::from_placement(e.archetype_hash, to_world));
        out.extend(tris.iter().map(|t| Triangle { vertices: t.vertices.map(|v| to_world(v)), material: t.material }));
        true
    }

    /// Every static placement of one ymap: plain entities, then interior
    /// furniture (MLO-local entities, then the MLO placement).
    pub fn bake_ymap(&mut self, bytes: &[u8], path: &Path, options: Options, counts: &mut Counts,
                     out: &mut Vec<Triangle>, stats: &mut Stats, baked: &mut BTreeSet<u32>) {
        let Ok(entities) = parse_ymap_entities(bytes) else { return };
        let doors = interiors::doors::read(bytes).unwrap_or_else(|e| panic!("door metadata {}: {e}", path.display()));
        for e in entities.iter().filter(|e| !e.is_mlo_instance) {
            stats.entities += 1;
            if self.place(e, doors.entity_is_door(e), &|v| entity_world(e, v), options, counts, out, stats, baked) {
                stats.placed += 1;
            }
        }
        if let Ok(instances) = parse_ymap_mlo_instances(bytes) {
            for inst in instances {
                let Some(children) = self.mlos.get(&inst.entity.archetype_hash).cloned() else { continue };
                let mlo_door = self.mlo_doors.get(&inst.entity.archetype_hash).cloned();
                for c in &children {
                    let world = |v| interiors::place(&inst.entity, c.to_world(v), false);
                    if self.place(c, mlo_door.as_ref().is_some_and(|d| d.entity_is_door(c)), &world, options, counts, out, stats, baked) {
                        stats.mlo_furniture += 1;
                    }
                }
            }
        }
    }
}

pub fn triangles(meta_dir: &Path, options: Options, counts: &mut Counts) -> (Vec<Triangle>, BTreeSet<u32>, Stats) {
    let mut stats = Stats::default();
    let base = meta_dir.join("base");
    let layers = meta_dir.join("layers");
    let mut lib = Library::load(meta_dir);
    stats.archetypes = lib.arch.len();
    stats.models = lib.models.len();
    let mut baked = BTreeSet::new();
    let mut out = Vec::new();
    let toggled = interiors::script_toggled_ymaps(meta_dir);
    for p in layered(&base, Some(&layers), "ymap", |p| !toggled.contains(&interiors::stem_hash(p))) {
        let Ok(bytes) = crate::vfs::read(&p) else { continue };
        lib.bake_ymap(&bytes, &p, options, counts, &mut out, &mut stats, &mut baked);
    }
    let Library { arch, models, mut bounds, .. } = lib;
    // Also retain bound templates for unplaced/spawnable drawable archetypes.
    // Model-space geometry uses the same decoding/filter as the static bake;
    // the runtime applies the actual current entity matrix, not a proxy box.
    for a in arch.values().filter(|a| a.asset_type == Archetype::ASSET_TYPE_DRAWABLE) {
        let Some(path) = models.get(&a.model_hash()) else { continue };
        let triangles = bounds.entry(a.model_hash()).or_insert_with(|| drawable_bound(path, options, counts));
        if let Some(triangles) = triangles {
            stats.templates.insert(a.name_hash, triangles.clone());
        }
    }
    // Vehicles (including trailers) need no YTYP archetype: their model name
    // is the fragment filename. Ignore render-only _hi variants. Later
    // archive layers win through the same filename precedence as drawables.
    let fragments: BTreeMap<u32, PathBuf> = layered(&base, Some(&layers), "yft", |p|
        !p.file_stem().is_some_and(|s| s.to_string_lossy().to_ascii_lowercase().ends_with("_hi")))
        .into_iter().map(|p| (stem_hash(&p),p)).collect();
    let mut fragment_templates = HashMap::new();
    let mut fragment_mass = HashMap::new();
    for (model,path) in &fragments {
        if let Some(triangles) = fragment_bound(path, options, counts) {
            stats.templates.insert(*model,triangles.clone());
            fragment_templates.insert(*model,triangles);
            if let Some((_, mass, _)) = fragment_masses(path).filter(|m| m.1.is_finite() && m.1 > 0.0) {
                stats.masses.insert(*model, mass);
                fragment_mass.insert(*model, mass);
            }
        }
    }
    // Prop archetypes may alias an asset name. GET_ENTITY_MODEL returns the
    // archetype hash, so also publish the template under that runtime identity.
    for a in arch.values().filter(|a| a.asset_type == Archetype::ASSET_TYPE_FRAGMENT) {
        if let Some(triangles) = fragment_templates.get(&a.model_hash()) {
            stats.templates.insert(a.name_hash,triangles.clone());
        }
        if let Some(&mass) = fragment_mass.get(&a.model_hash()) {
            stats.masses.insert(a.name_hash, mass);
        }
    }
    eprintln!("fragment templates: {} decoded / {} models (pristine dynamic templates only)",
        fragment_templates.len(),fragments.len());
    eprintln!("explicit door instances kept dynamic: {}", stats.dynamic_doors);
    (out, baked, stats)
}

/// A script-toggled map state (IPL) that has collision: its ymap name (the
/// name GTA's IPL natives take) and its world-space triangles.
pub struct MapState {
    pub name: String,
    pub triangles: Vec<Triangle>,
}

/// The collision of every script-toggled ymap (alternate interior states,
/// mission states): MLO shells, interior furniture and the ymap's own static
/// props. The static bake leaves these out; the runtime adds a state only
/// while GTA reports it active (`IS_IPL_ACTIVE`).
pub fn map_states(
    meta_dir: &Path,
    ybn_dirs: &[PathBuf],
    ybn_layers: Option<&Path>,
    options: Options,
    counts: &mut Counts,
) -> (Vec<MapState>, interiors::Stats) {
    let mut shells = interiors::Shells::load(meta_dir, ybn_dirs, ybn_layers);
    let mut lib = Library::load(meta_dir);
    let toggled = interiors::script_toggled_ymaps(meta_dir);
    let ymaps = layered(
        &meta_dir.join("base"),
        Some(&meta_dir.join("layers")),
        "ymap",
        |p| toggled.contains(&interiors::stem_hash(p)),
    );
    let mut stats = interiors::empty_stats(ymaps.len());
    let mut prop_stats = Stats::default();
    let mut baked = BTreeSet::new();
    let mut states = Vec::new();
    for ymap in &ymaps {
        let Ok(bytes) = crate::vfs::read(ymap) else {
            continue;
        };
        let mut triangles = Vec::new();
        shells.place_ymap(&bytes, options, false, counts, &mut stats, &mut triangles);
        lib.bake_ymap(&bytes, ymap, options, counts, &mut triangles, &mut prop_stats, &mut baked);
        if triangles.is_empty() {
            continue;
        }
        let name = ymap.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        states.push(MapState { name, triangles });
    }
    (states, stats)
}

/// What happens to a placement's collision in the static bake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Coverage {
    /// Embedded drawable bound baked into the cache.
    Baked,
    /// Door leaf: dynamic template only (by design).
    Door,
    /// Fragment archetype: dynamic entity template only (by design).
    Fragment,
    /// Drawable whose `.ydr` has no embedded bound (no collision in GTA either,
    /// unless supplied elsewhere).
    NoBound,
    /// Drawable whose `.ydr` was not extracted (outside levels/gta5/props).
    ModelMissing,
    /// Archetype in a drawable dictionary (`.ydd`).
    DrawableDictionary,
    /// Assetless/uninitialized archetype (no model).
    Assetless,
    /// No archetype definition among the extracted `.ytyp` files.
    ArchetypeMissing,
}

/// Collision coverage audit: classifies every ymap entity and MLO furniture
/// placement exactly as `triangles` decides, ranks the archetypes that end up
/// without collision, and lists placements within `near` (centre, radius).
pub fn audit(meta_dir: &Path, near: Option<([f32; 3], f32)>) {
    let base = meta_dir.join("base");
    let layers = meta_dir.join("layers");
    let mut arch: HashMap<u32, Archetype> = HashMap::new();
    let mut mlos: HashMap<u32, Vec<YmapEntity>> = HashMap::new();
    let mut door_archetypes = std::collections::HashSet::new();
    let mut mlo_doors = HashMap::new();
    for p in layered(&base, Some(&layers), "ytyp", |_| true) {
        let Ok(bytes) = crate::vfs::read(&p) else { continue };
        let Ok(y) = parse_ytyp(&bytes) else { continue };
        let doors = interiors::doors::read(&bytes).unwrap_or_default();
        for a in y.archetypes {
            if doors.archetypes.contains(&a.name_hash) { door_archetypes.insert(a.name_hash); }
            else { door_archetypes.remove(&a.name_hash); }
            arch.insert(a.name_hash, a);
        }
        for m in y.mlos {
            mlo_doors.insert(m.name_hash, doors.clone());
            mlos.insert(m.name_hash, m.entities);
        }
    }
    let models: BTreeMap<u32, PathBuf> =
        layered(&base, Some(&layers), "ydr", |_| true).into_iter().map(|p| (stem_hash(&p), p)).collect();
    let mut has_bound: HashMap<u32, bool> = HashMap::new();
    let mut counts = Counts::default();
    let mut classify = |e: &YmapEntity, door: bool| -> Coverage {
        if door || door_archetypes.contains(&e.archetype_hash) {
            return Coverage::Door;
        }
        let Some(a) = arch.get(&e.archetype_hash) else { return Coverage::ArchetypeMissing };
        match a.asset_type {
            Archetype::ASSET_TYPE_FRAGMENT => return Coverage::Fragment,
            Archetype::ASSET_TYPE_DRAWABLEDICTIONARY => return Coverage::DrawableDictionary,
            Archetype::ASSET_TYPE_DRAWABLE => {}
            _ => return Coverage::Assetless,
        }
        let Some(path) = models.get(&a.model_hash()) else { return Coverage::ModelMissing };
        let bound = *has_bound.entry(a.model_hash())
            .or_insert_with(|| drawable_bound(path, Options { primitives: true, all_flags: false }, &mut counts).is_some());
        if bound { Coverage::Baked } else { Coverage::NoBound }
    };
    let mut totals: BTreeMap<Coverage, usize> = BTreeMap::new();
    let mut by_arch: HashMap<(Coverage, u32), usize> = HashMap::new();
    let mut nearby: Vec<(f32, Coverage, u32, [f32; 3], bool)> = Vec::new();
    let mut note = |c: Coverage, e: &YmapEntity, pos: rage_formats::math::Vec3, mlo: bool| {
        *totals.entry(c).or_default() += 1;
        *by_arch.entry((c, e.archetype_hash)).or_default() += 1;
        if let Some((centre, radius)) = near {
            let d = ((pos.x - centre[0]).powi(2) + (pos.y - centre[1]).powi(2) + (pos.z - centre[2]).powi(2)).sqrt();
            if d <= radius {
                nearby.push((d, c, e.archetype_hash, [pos.x, pos.y, pos.z], mlo));
            }
        }
    };
    let toggled = interiors::script_toggled_ymaps(meta_dir);
    for p in layered(&base, Some(&layers), "ymap", |p| !toggled.contains(&interiors::stem_hash(p))) {
        let Ok(bytes) = crate::vfs::read(&p) else { continue };
        let Ok(entities) = parse_ymap_entities(&bytes) else { continue };
        let doors = interiors::doors::read(&bytes).unwrap_or_default();
        for e in entities.iter().filter(|e| !e.is_mlo_instance) {
            let c = classify(e, doors.entity_is_door(e));
            note(c, e, entity_world(e, rage_formats::math::Vec3::new(0.0, 0.0, 0.0)), false);
        }
        if let Ok(instances) = parse_ymap_mlo_instances(&bytes) {
            for inst in instances {
                let Some(children) = mlos.get(&inst.entity.archetype_hash) else { continue };
                let mlo_door = mlo_doors.get(&inst.entity.archetype_hash);
                for ch in children {
                    let c = classify(ch, mlo_door.is_some_and(|d| d.entity_is_door(ch)));
                    let pos = interiors::place(&inst.entity, ch.to_world(rage_formats::math::Vec3::new(0.0, 0.0, 0.0)), false);
                    note(c, ch, pos, true);
                }
            }
        }
    }
    println!("placements by collision coverage:");
    for (c, n) in &totals {
        println!("  {c:?}: {n}");
    }
    for c in [Coverage::ModelMissing, Coverage::DrawableDictionary, Coverage::ArchetypeMissing, Coverage::NoBound, Coverage::Assetless] {
        let mut v: Vec<_> = by_arch.iter().filter(|((k, _), _)| *k == c).map(|((_, h), n)| (*n, *h)).collect();
        v.sort_by(|a, b| b.cmp(a));
        println!("top {c:?} archetypes ({} distinct):", v.len());
        for (n, h) in v.iter().take(25) {
            println!("  {h:08x} x{n}");
        }
    }
    if near.is_some() {
        nearby.sort_by(|a, b| a.0.total_cmp(&b.0));
        println!("placements near the point:");
        for (d, c, h, p, mlo) in nearby {
            println!("  {d:6.1} m {c:?} {h:08x} at ({:.2}, {:.2}, {:.2}){}", p[0], p[1], p[2], if mlo { " [MLO]" } else { "" });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rage_formats::math::Vec3;
    fn put_u64(b: &mut [u8], at: usize, value: u64) { b[at..at+8].copy_from_slice(&value.to_le_bytes()); }
    fn put_vec(b: &mut [u8], at: usize, v: Vec3) {
        for (i,f) in [v.x,v.y,v.z].into_iter().enumerate() { b[at+i*4..at+i*4+4].copy_from_slice(&f.to_le_bytes()); }
    }
    fn fragment_fixture() -> Vec<u8> {
        let mut b = vec![0u8; 0x800];
        put_u64(&mut b,0xf0,SYSTEM_BASE+0x200);
        put_u64(&mut b,0x210,SYSTEM_BASE+0x240);
        put_vec(&mut b,0x270,Vec3::new(100.,200.,300.)); // LOD.PositionOffset: not a root-bound transform
        put_u64(&mut b,0x328,SYSTEM_BASE+0x400);
        b[0x410] = 10; // composite
        put_u64(&mut b,0x470,SYSTEM_BASE+0x500);
        put_u64(&mut b,0x478,SYSTEM_BASE+0x520);
        b[0x4a0] = 1; // one child
        put_u64(&mut b,0x500,SYSTEM_BASE+0x600);
        for (i,v) in [Vec3::X,Vec3::Y,Vec3::Z,Vec3::new(5.,6.,7.)].into_iter().enumerate() {
            put_vec(&mut b,0x520+i*16,v);
        }
        b[0x610] = 3; // authored box
        put_vec(&mut b,0x620,Vec3::new(1.,2.,3.));
        put_vec(&mut b,0x630,Vec3::new(-1.,-2.,-3.));
        b
    }
    #[test]
    fn linked_door_stays_template_while_same_model_neighbor_is_baked() {
        use rage_formats::resource::build_rsc7;
        let dir=std::env::temp_dir().join(format!("skatev-door-fixture-{}",std::process::id()));
        let base=dir.join("base");std::fs::create_dir_all(&base).unwrap();
        let model=rage_joaat("fixture_door");
        let put32=|b:&mut [u8],o:usize,v:u32| b[o..o+4].copy_from_slice(&v.to_le_bytes());
        let mut ytyp=interiors::doors::tests::fixture();
        put32(&mut ytyp,0x458,model);put32(&mut ytyp,0x46c,Archetype::ASSET_TYPE_DRAWABLE);put32(&mut ytyp,0x470,model);
        ytyp[0x480]=0; // no archetype extension: only one placed instance is a door
        std::fs::write(base.join("fixture.ytyp"),build_rsc7(2,&ytyp,&[])).unwrap();
        let mut ymap=interiors::doors::tests::fixture();
        put32(&mut ymap,0x70,3_545_841_574);put32(&mut ymap,0x74,368); // CMapData root
        put64_test(&mut ymap,0x160,2);ymap[0x168]=2; // two entity pointers
        put32(&mut ymap,0x84,16);put64_test(&mut ymap,0x180,3);put64_test(&mut ymap,0x188,(128<<12)|3);
        put32(&mut ymap,0x208,model);put32(&mut ymap,0x288,model);
        std::fs::write(base.join("fixture.ymap"),build_rsc7(2,&ymap,&[])).unwrap();
        let mut ydr=fragment_fixture();put64_test(&mut ydr,0xc8,SYSTEM_BASE+0x600);
        std::fs::write(base.join("fixture_door.ydr"),build_rsc7(165,&ydr,&[])).unwrap();
        let (triangles,_,stats)=triangles(&dir,Options {primitives:true,all_flags:false},&mut Counts::default());
        assert_eq!(stats.dynamic_doors,1);
        assert_eq!(stats.instances.len(),1,"non-door instance of same model must remain baked");
        assert_eq!(stats.instances[0].position.x,5.0);
        assert_eq!(triangles.len(),12,"only neighbor box emitted, not closed door geometry");
        assert_eq!(stats.templates[&model].len(),12,"moving door still has exact local collision");
        // Test-created directory only, under the process temp root.
        std::fs::remove_dir_all(&dir).unwrap();
    }
    fn put64_test(b:&mut [u8],o:usize,v:u64) { b[o..o+8].copy_from_slice(&v.to_le_bytes()); }
    #[test]
    fn fragment_composite_has_one_model_transform_not_two_lod_offsets() {
        let b = fragment_fixture();
        let r = ResReader { system: &b, graphics: &[] };
        let triangles = fragment_bound_reader(&r,Options { primitives:true,all_flags:false },&mut Counts::default()).unwrap();
        assert_eq!(triangles.len(),12);
        let lo = triangles.iter().flat_map(|t| t.vertices).fold(Vec3::new(999.,999.,999.),Vec3::min);
        let hi = triangles.iter().flat_map(|t| t.vertices).fold(Vec3::new(-999.,-999.,-999.),Vec3::max);
        assert_eq!(lo,Vec3::new(4.,4.,4.));
        assert_eq!(hi,Vec3::new(6.,8.,10.));
    }
    #[test]
    fn fragment_drawable_fallback_refuses_unknown_frame() {
        let mut b = fragment_fixture();
        put_u64(&mut b,0xf0,0);
        put_u64(&mut b,0x30,SYSTEM_BASE+0x200);
        put_u64(&mut b,0x2f0,SYSTEM_BASE+0x600);
        for (i,v) in [Vec3::X,Vec3::Y,Vec3::Z,Vec3::ZERO].into_iter().enumerate() { put_vec(&mut b,0x2b0+i*16,v); }
        let options = Options { primitives:true,all_flags:false };
        assert_eq!(fragment_bound_reader(&ResReader { system:&b,graphics:&[] },options,&mut Counts::default()).unwrap().len(),12);
        put_vec(&mut b,0x2e0,Vec3::new(1.,2.,3.));
        assert!(fragment_bound_reader(&ResReader { system:&b,graphics:&[] },options,&mut Counts::default()).is_none());
    }
    #[test]
    fn fragment_truncated_pointers_are_rejected() {
        let mut b = fragment_fixture();
        put_u64(&mut b,0xf0,SYSTEM_BASE+0x7f8);
        assert!(fragment_bound_reader(&ResReader { system:&b,graphics:&[] },Options { primitives:true,all_flags:false },&mut Counts::default()).is_none());
    }
    #[test]
    fn baked_instance_preserves_composed_rotation_scale_and_translation() {
        // A scaled prop in a rotated/transformed interior. These are the same
        // composed world coordinates that the emitted triangle vertices use.
        let world = |v: Vec3| Vec3::new(100.0 - 2.0*v.y, -200.0 + 2.0*v.x, 30.0 + 3.0*v.z);
        let instance = BakedInstance::from_placement(0x1234_abcd, &world);
        let v = Vec3::new(0.25, -1.5, 2.0);
        let rebuilt = instance.position + instance.axes[0]*v.x + instance.axes[1]*v.y + instance.axes[2]*v.z;
        assert_eq!(rebuilt, world(v));
        let line = instance.line();
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 13);
        assert_eq!(fields[0], "1234abcd");
        assert_eq!(fields[1..].iter().map(|v| v.parse::<f32>().unwrap()).collect::<Vec<_>>(),
            vec![100., -200., 30., 0., 2., 0., -2., 0., 0., 0., 0., 3.]);
    }
}

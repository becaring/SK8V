//! Static GTA collision around the skater, as Skate collision + grind rails.
//! The static map is GTA's own loaded collision, read from its physics level
//! (`live.rs`), never from per-frame GTA raycasts; moving GTA entities are
//! `dynamic.rs`. Beside the cache path only the sidecars are read (`.svsd`
//! surfaces, `crackmaps.svgj` ground joints), by triangle vertex bits and
//! position.
use crate::{coords, curbs, rails};
use bevy_math::Vec3;
use std::path::Path;

/// Half-size of the XY square of collision built around the skater.
pub const RADIUS: f32 = 128.0;
/// Horizontal drift from the built centre that triggers a rebuild.
pub const RECENTRE: f32 = 64.0;

pub struct Area {
    pub center: Vec3,
    /// Skate space (Y up), counterclockwise.
    pub triangles: Vec<[[f32; 3]; 3]>,
    /// Packed Skate audio/physics/seam surface per triangle, plus
    /// `NO_GRIND_TAG` for curb bevels (grind probes ignore them).
    pub tags: Vec<u32>,
    /// Skate space polylines.
    pub rails: Vec<Vec<[f32; 3]>>,
    pub census: rails::RailCensus,
    pub bevels: curbs::BevelCensus,
    /// Where the triangles came from (live physics level census), for the log.
    pub note: String,
}

pub struct StaticWorld {
    /// Recovered sound surfaces beside the cache (`.svsd`), if baked.
    recovered: crate::surfaces::Recovered,
    /// Visual ground with texture joint maps beside the cache (`crackmaps.svgj`).
    ground: crate::ground::GroundSource,
    /// GTA's own loaded collision (`live.rs`), once the host has found it;
    /// until then no area can be built.
    live: Option<crate::live::Level>,
}

impl StaticWorld {
    /// `path` names the cache the sidecars sit beside; the cache itself is
    /// not read.
    pub fn open(path: &Path) -> Self {
        Self { recovered: crate::surfaces::Recovered::open(path), ground: crate::ground::GroundSource::open(path), live: None }
    }

    /// Where GTA's loaded collision is (None: it is not available).
    pub fn set_live(&mut self, level: Option<crate::live::Level>) {
        self.live = level;
    }

    /// The recovered-surface sidecar's load state, for the log.
    pub fn recovered_status(&self) -> &str {
        self.recovered.status()
    }

    /// The ground-joint sidecar's load state, for the log.
    pub fn ground_status(&self) -> &str {
        self.ground.status()
    }

    /// Collision and rails around GTA point `center`, from GTA's loaded
    /// collision (which already holds whatever map states are active).
    pub fn around(&mut self, center: Vec3) -> Result<Area, String> {
        let level = self.live.ok_or("GTA's loaded collision (its physics level) has not been found yet")?;
        let (tris, census) = level.walk([center.x, center.y], RADIUS)?;
        let mut note = census.to_string();
        let p = [center.x, center.y];
        if !tris.iter().any(|t| svwc::clean::floor_at(t, p).is_some_and(|z| (z - center.z).abs() < 4.0)) {
            note.push_str(&format!("; NO FLOOR under ({:.1}, {:.1}, {:.1}): {}", p[0], p[1], center.z, level.under(p).unwrap_or_else(|e| e)));
        }
        let recovered = self.recovered.around([center.x, center.y], RADIUS)?;
        if self.recovered.is_loaded() {
            let matched = tris.iter().filter(|t| recovered.contains_key(&svwc::surfaces::key(&t.v))).count();
            note.push_str(&format!("; surface records matched {matched} of {} triangles", tris.len()));
        }
        // Recovered joint lines of this area for the audio tick's wheel lookup.
        if self.recovered.is_loaded() {
            crate::surfaces::publish(crate::surfaces::GridIndex::build(tris.iter().filter_map(|t| {
                recovered.get(&svwc::surfaces::key(&t.v)).map(|r| (t.v, r.families))
            })));
        }
        crate::ground::publish(self.ground.around([center.x, center.y], RADIUS)?);
        let mut area = build_area_tagged(
            center,
            tris.iter().map(|t| (t.v, crate::surfaces::tag(t.material, recovered.get(&svwc::surfaces::key(&t.v))))),
        );
        area.note = note;
        Ok(area)
    }
}

/// Converts GTA-space triangles to a Skate collision area with rails.
pub fn build_area(center: Vec3, tris: impl Iterator<Item = [[f32; 3]; 3]>) -> Area {
    build_area_materials(center, tris.map(|t| (t, 0)))
}

pub fn build_area_materials(center: Vec3, tris: impl Iterator<Item = ([[f32; 3]; 3], u8)>) -> Area {
    build_area_tagged(center, tris.map(|(t, material)| (t, crate::materials::surface(material).packed())))
}

/// As `build_area_materials`, with each triangle's audio tag already chosen.
pub fn build_area_tagged(center: Vec3, tris: impl Iterator<Item = ([[f32; 3]; 3], u32)>) -> Area {
    let (gta, surfaces): (Vec<[Vec3; 3]>, Vec<u32>) = tris
        .map(|(t, tag)| (t.map(Vec3::from_array), tag))
        .filter(|(p, _)| p.iter().all(|v| v.is_finite())
            && (p[1] - p[0]).cross(p[2] - p[0]).length_squared() > 1e-10)
        .unzip();
    let (found, census) = rails::find_metres(&gta);
    let grid = crate::grid::XyGrid::triangles(curbs::CELL, &gta);
    let curbs::Bevels { triangles: bevel_tris, sources, buried, census: bevels } = curbs::bevels_with_sources(&gta, &grid);
    // Rails were found on the full GTA set; buried risers then leave the
    // collision (their bevels cover them).
    let mut keep = vec![true; gta.len()];
    for &i in buried
        .iter()
        .chain(&crate::plies::interior(&gta, &grid))
        .chain(&crate::plies::abutting_caps(&gta))
        .chain(&crate::plies::buried_faces(&gta))
    {
        keep[i] = false;
    }
    let mut collision: Vec<([Vec3; 3], u32)> = gta
        .iter()
        .zip(&surfaces)
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|((t, s), _)| (*t, *s))
        .chain(bevel_tris.iter().zip(&sources).map(|(t, &i)| (*t, surfaces[i] | skate_host::bridge::NO_GRIND_TAG)))
        .collect();
    spatial_order(&mut collision);
    Area {
        tags: collision.iter().map(|(_, s)| *s).collect(),
        center,
        triangles: collision.iter().map(|(p, _)| p.map(|v| coords::to_skate(v).to_array())).collect(),
        rails: found
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|p| coords::to_skate(p).to_array())
                    .collect()
            })
            .collect(),
        census,
        bevels,
        note: String::new(),
    }
}

/// Sorts triangles along a Z-order (Morton) curve of their centroids.
///
/// Skate groups imported triangles into 64-triangle query meshes in the order
/// given and builds its bounds hierarchy over those. GTA's soup arrives in
/// cache-tile order, so a chunk could span a whole tile and every probe
/// visited thousands of triangles (owner: 9..21 ms Skate ticks walking in
/// Rockford). Spatially coherent chunks keep the same triangles and tags and
/// let the existing hierarchy prune. GTA triangle order carries no meaning.
pub fn spatial_order<T>(tris: &mut Vec<([Vec3; 3], T)>) {
    if tris.is_empty() {
        return;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (t, _) in tris.iter() {
        let c = (t[0] + t[1] + t[2]) / 3.0;
        lo = lo.min(c);
        hi = hi.max(c);
    }
    let span = (hi - lo).max(Vec3::splat(1e-3));
    // 21 bits per axis (63-bit key).
    let spread = |v: u64| {
        let mut x = v & 0x1f_ffff;
        x = (x | x << 32) & 0x1f_0000_0000_ffff;
        x = (x | x << 16) & 0x1f_0000_ff00_00ff;
        x = (x | x << 8) & 0x100f_00f0_0f00_f00f;
        x = (x | x << 4) & 0x10c3_0c30_c30c_30c3;
        x = (x | x << 2) & 0x1249_2492_4924_9249;
        x
    };
    let key = |t: &[Vec3; 3]| {
        let c = ((t[0] + t[1] + t[2]) / 3.0 - lo) / span * 2_097_151.0;
        spread(c.x as u64) | spread(c.y as u64) << 1 | spread(c.z as u64) << 2
    };
    tris.sort_by_cached_key(|(t, _)| key(t));
}

/// What an area feeds Skate, checked against the invariants Skate relies on.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct AreaAudit {
    pub triangles: usize,
    /// Non-finite vertex (Skate rejects the whole build).
    pub non_finite: usize,
    /// Zero-area or zero-length-edge triangle in f32 Skate space (Skate's
    /// `WorldTriangle::from_vertices` / "Invalid SKATE collision triangle
    /// normal" reject the whole build).
    pub degenerate: usize,
    /// Packed wheel surface `(tag >> 7) & 31` of 16 or more: Skate's surface
    /// vote panics ("native surface histogram index exceeded").
    pub bad_surface: usize,
    /// Bits outside the packed surface and NO_GRIND_TAG.
    pub unknown_tag_bits: usize,
    pub tag_count_mismatch: bool,
    pub non_finite_rails: usize,
}

impl AreaAudit {
    pub fn ok(&self) -> bool {
        self.non_finite == 0
            && self.degenerate == 0
            && self.bad_surface == 0
            && self.unknown_tag_bits == 0
            && !self.tag_count_mismatch
            && self.non_finite_rails == 0
    }
}

impl std::fmt::Display for AreaAudit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "audit {}: {} triangles, {} non-finite, {} degenerate, {} bad surface, {} unknown tag bits, tags {}, {} non-finite rails",
            if self.ok() { "ok" } else { "FAILED" },
            self.triangles,
            self.non_finite,
            self.degenerate,
            self.bad_surface,
            self.unknown_tag_bits,
            if self.tag_count_mismatch { "MISMATCHED" } else { "matched" },
            self.non_finite_rails
        )
    }
}

/// Packed wheel surface Skate reads from a collision tag.
pub fn packed_surface(tag: u32) -> u32 {
    (tag >> 7) & 31
}

/// Checks a built area against what Skate requires of its input.
pub fn audit(area: &Area) -> AreaAudit {
    let mut a = AreaAudit {
        triangles: area.triangles.len(),
        tag_count_mismatch: area.tags.len() != area.triangles.len(),
        ..Default::default()
    };
    for t in &area.triangles {
        let p = t.map(Vec3::from_array);
        if !p.iter().all(|v| v.is_finite()) {
            a.non_finite += 1;
            continue;
        }
        let n = (p[1] - p[0]).cross(p[2] - p[0]);
        let edges = [p[1] - p[0], p[2] - p[1], p[0] - p[2]];
        if n.try_normalize().is_none() || edges.iter().any(|e| !(e.length() > 0.0)) {
            a.degenerate += 1;
        }
    }
    for &tag in &area.tags {
        if packed_surface(tag) >= 16 {
            a.bad_surface += 1;
        }
        if tag & !(skate_host::bridge::NO_GRIND_TAG | 0xf7ff) != 0 {
            a.unknown_tag_bits += 1;
        }
    }
    a.non_finite_rails = area
        .rails
        .iter()
        .filter(|r| r.iter().flatten().any(|v| !v.is_finite()))
        .count();
    a
}

pub fn needs_recentre(built: Vec3, skater: Vec3) -> bool {
    (skater - built).truncate().length() > RECENTRE
}

/// Builds are assumed to take this much longer than the recent average
/// before the next one is requested and placed.
const BUILD_LEAD: f32 = 1.3;
/// The next area's centre leads the skater by at most this much, so it
/// still covers the skater with room behind if they stop or turn.
const MAX_LEAD: f32 = RADIUS * 0.6;
/// A moving skater is rebuilt for while still this far inside the area.
const COVER: f32 = RADIUS * 0.85;

/// As `needs_recentre`, for a skater moving at `velocity` (GTA m/s) while
/// builds take `build_secs`: at speed the next area is requested while the
/// current one still covers the distance travelled during the build (a
/// Backwards Man launch at ~36 m/s ran off every area).
pub fn needs_recentre_moving(built: Vec3, skater: Vec3, velocity: Vec3, build_secs: f32) -> bool {
    let d = (skater - built).truncate().length();
    d > RECENTRE || d + velocity.truncate().length() * build_secs * BUILD_LEAD > COVER
}

/// Where to centre the next area: ahead of the skater by the distance they
/// will travel while it builds, at most `MAX_LEAD`.
pub fn lead_centre(skater: Vec3, velocity: Vec3, build_secs: f32) -> Vec3 {
    let ahead = Vec3::new(velocity.x, velocity.y, 0.) * (build_secs * BUILD_LEAD);
    skater + ahead.clamp_length_max(MAX_LEAD)
}

/// Whether a finished build centred at `built` should replace the installed
/// area at `installed` for a skater at `skater`: when it still covers them
/// well, or when it is nearer to them than what they are on now.
pub fn worth_installing(built: Vec3, installed: Vec3, skater: Vec3) -> bool {
    let to_new = (skater - built).truncate().length();
    to_new <= RADIUS * 0.75 || to_new < (skater - installed).truncate().length()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_skaters_get_their_next_area_ahead_and_in_time() {
        let v = Vec3::new(36.0, 0.0, 0.0);
        // Standing still: the fixed drift rule.
        assert!(!needs_recentre_moving(Vec3::ZERO, Vec3::new(30.0, 0.0, 0.0), Vec3::ZERO, 3.0));
        assert!(needs_recentre_moving(Vec3::ZERO, Vec3::new(65.0, 0.0, 0.0), Vec3::ZERO, 3.0));
        // 36 m/s with 2.5 s builds: requested long before the 64 m drift.
        assert!(needs_recentre_moving(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), v, 2.5));
        // The centre leads, capped so the skater stays well inside.
        let c = lead_centre(Vec3::ZERO, v, 2.5);
        assert!((c.x - MAX_LEAD).abs() < 1e-3 && c.y == 0.0);
        assert_eq!(lead_centre(Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO, 2.5), Vec3::new(1.0, 2.0, 3.0));
        // Steady state at 36 m/s: each build lands before the skater leaves
        // the installed area.
        let (mut installed, mut at, build) = (Vec3::ZERO, Vec3::ZERO, 2.5);
        for _ in 0..20 {
            while !needs_recentre_moving(installed, at, v, build) {
                at += v / 60.0;
            }
            let centre = lead_centre(at, v, build);
            at += v * build;
            assert!((at - installed).truncate().length() < RADIUS, "ran off the area at {at}");
            assert!(worth_installing(centre, installed, at));
            installed = centre;
        }
    }

    #[test]
    fn a_late_build_nearer_than_the_installed_area_is_still_installed() {
        let installed = Vec3::ZERO;
        let skater = Vec3::new(250.0, 0.0, 0.0);
        assert!(worth_installing(Vec3::new(120.0, 0.0, 0.0), installed, skater));
        assert!(!worth_installing(Vec3::new(-20.0, 0.0, 0.0), installed, skater));
        assert!(worth_installing(Vec3::new(10.0, 0.0, 0.0), installed, Vec3::new(20.0, 0.0, 0.0)));
    }

    #[test]
    fn spatial_order_keeps_every_triangle_with_its_tag_and_groups_neighbours() {
        let tri = |x: f32, y: f32| [Vec3::new(x, y, 0.0), Vec3::new(x + 0.5, y, 0.0), Vec3::new(x, y + 0.5, 0.0)];
        // Alternate far-apart triangles: source order interleaves two clusters.
        let mut t: Vec<([Vec3; 3], u32)> = (0..128)
            .map(|i| if i % 2 == 0 { (tri(i as f32 * 0.01, 0.0), i) } else { (tri(500.0 + i as f32 * 0.01, 500.0), i) })
            .collect();
        let before: std::collections::BTreeSet<u32> = t.iter().map(|(_, s)| *s).collect();
        super::spatial_order(&mut t);
        let after: std::collections::BTreeSet<u32> = t.iter().map(|(_, s)| *s).collect();
        assert_eq!(before, after);
        for (p, s) in &t {
            let near = if s % 2 == 0 { p[0].x < 10.0 } else { p[0].x > 400.0 };
            assert!(near, "tag {s} still belongs to its triangle");
        }
        // Each 64-triangle chunk now holds one cluster.
        for chunk in t.chunks(64) {
            let first = chunk[0].0[0].x > 400.0;
            assert!(chunk.iter().all(|(p, _)| (p[0].x > 400.0) == first), "chunk mixes clusters");
        }
    }

    #[test]
    fn filtering_preserves_contact_material_alignment() {
        let floor = [[0.,0.,0.],[1.,0.,0.],[0.,1.,0.]];
        let area = build_area_materials(Vec3::ZERO, [(floor,55), ([[0.;3];3],1), (floor,174)].into_iter());
        assert_eq!(area.tags, vec![crate::materials::surface(55).packed(), crate::materials::surface(174).packed()]);
        assert!(audit(&area).ok());
    }

    #[test]
    fn no_grind_tag_keeps_surface_zero_and_audits_cleanly() {
        let mut area = build_area(Vec3::ZERO,
            [[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]].into_iter());
        area.tags[0] = skate_host::bridge::NO_GRIND_TAG;
        assert_eq!(packed_surface(area.tags[0]), 0);
        assert!(audit(&area).ok());
        area.tags[0] |= 16 << 7;
        let result = audit(&area);
        assert_eq!(result.bad_surface, 1);
        assert_eq!(result.unknown_tag_bits, 1);
        assert!(!result.ok());
    }

    #[test]
    fn area_is_converted_to_skate_space_and_drops_degenerates() {
        let floor = [[0.0, 0.0, 5.0], [1.0, 0.0, 5.0], [0.0, 1.0, 5.0]];
        let degenerate = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let area = build_area(Vec3::ZERO, [floor, degenerate].into_iter());
        assert_eq!(area.triangles.len(), 1);
        let t = area.triangles[0].map(Vec3::from_array);
        assert!(
            t.iter().all(|v| (v.y - 5.0).abs() < 1e-6),
            "GTA z becomes skate y"
        );
        assert!(
            (t[1] - t[0]).cross(t[2] - t[0]).y > 0.0,
            "upward floor stays upward"
        );
    }

    #[test]
    fn recentre_ignores_vertical_motion() {
        assert!(!needs_recentre(Vec3::ZERO, Vec3::new(0.0, 0.0, 500.0)));
        assert!(needs_recentre(
            Vec3::ZERO,
            Vec3::new(RECENTRE + 1.0, 0.0, 0.0)
        ));
    }
}

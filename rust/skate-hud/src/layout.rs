//! Where the 1280x720 APT movie goes on a GTA backbuffer of any shape.
//!
//! Rule (docs/DECISIONS.md "HUD layout"):
//! 1. GTA's safe zone (`GET_SAFE_ZONE_SIZE`, the profile's "Safezone size",
//!    0.9..1.0) insets the screen evenly on every side, exactly as GTA's own
//!    HUD is inset. In game the host reports GTA's actual HUD area instead
//!    (`compute_in`: its script-graphics alignment, which also holds the HUD
//!    and minimap in from the edges on ultrawide panels).
//! 2. The movie is scaled uniformly so its 720 units fill the safe height
//!    (the retail HUD is authored at 16:9 with its own title-safe margins, so
//!    at 16:9 with safe zone 1.0 it sits exactly where it did on a 720p TV,
//!    and it keeps that size relative to the screen height on any panel, like
//!    GTA's own HUD).
//! 3. The HUD region is the safe area, centred, at most `max_aspect` wide
//!    (default 2.4:1, every "21:9" panel; 32:9 keeps the HUD in front of the player rather than
//!    in peripheral vision). The movie is centred in it.
//! 4. The native trickdisplay is anchored bottom-left inside one screen container
//!    (`mScreen`). The movie's own `Screen_EdgeOffset` mechanism (its
//!    constructor adds the global to `mScreen._x`) moves that container by
//!    `(1280 - visible_width) / 2` movie units, so it keeps its 16:9 distance
//!    from the region's left edge: negative (outwards) on 21:9/32:9, positive
//!    (inwards) on 16:10/4:3, where the movie's sides would otherwise fall
//!    outside the screen.
//! 5. SkateV's compact presentation relocates these draws to
//!    the bottom right, with text to the left of the meter. It reflects the
//!    edge anchoring, not the glyphs/artwork; see `presentation.rs`.
pub const MOVIE_WIDTH: f32 = 1280.0;
pub const MOVIE_HEIGHT: f32 = 720.0;
pub const MOVIE_ASPECT: f32 = MOVIE_WIDTH / MOVIE_HEIGHT;
/// Default widest HUD region: 2.4:1, the widest "21:9" panel class
/// (2560x1080 is 2.37, 3440x1440 2.39, 3840x1600 2.4).
pub const DEFAULT_MAX_ASPECT: f32 = 2.4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    /// Output pixels per movie unit.
    pub scale: f32,
    /// Output pixel position of movie point (0, 0).
    pub origin: [f32; 2],
    /// `Screen_EdgeOffset` in movie units (negative moves the screen
    /// container left, towards the region's left edge).
    pub edge_offset: f32,
    /// GTA safe area, output pixels: x0, y0, x1, y1.
    pub safe: [f32; 4],
    /// HUD region (inside `safe`), output pixels: x0, y0, x1, y1.
    pub region: [f32; 4],
}

impl Layout {
    /// `safe_zone` is GTA's `GET_SAFE_ZONE_SIZE` (1.0 = no inset);
    /// `max_aspect` <= 0 means no cap.
    pub fn compute(width: u32, height: u32, safe_zone: f32, max_aspect: f32) -> Self {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let sz = if safe_zone.is_finite() { safe_zone.clamp(0.5, 1.0) } else { 1.0 };
        let (sw, sh) = (w * sz, h * sz);
        let safe = [(w - sw) * 0.5, (h - sh) * 0.5, (w + sw) * 0.5, (h + sh) * 0.5];
        Self::within(width, height, safe, max_aspect)
    }

    /// As `compute`, inside GTA's own HUD area as the game reports it
    /// (normalized x0, y0, x1, y1 from its script-graphics alignment, which
    /// applies the safe zone and GTA's ultrawide placement). None when the
    /// area is degenerate.
    pub fn compute_in(width: u32, height: u32, area: [f32; 4], max_aspect: f32) -> Option<Self> {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let ok = area.iter().all(|v| v.is_finite() && (-0.01..=1.01).contains(v))
            && area[2] - area[0] > 0.25 && area[3] - area[1] > 0.25;
        ok.then(|| Self::within(width, height, [area[0] * w, area[1] * h, area[2] * w, area[3] * h], max_aspect))
    }

    fn within(width: u32, height: u32, safe: [f32; 4], max_aspect: f32) -> Self {
        let (sw, sh) = (safe[2] - safe[0], safe[3] - safe[1]);
        let (cx, cy) = ((safe[0] + safe[2]) * 0.5, (safe[1] + safe[3]) * 0.5);
        let cap = if max_aspect.is_finite() && max_aspect > 0.0 {
            max_aspect.max(MOVIE_ASPECT)
        } else {
            f32::INFINITY
        };
        let scale = sh / MOVIE_HEIGHT;
        let region_w = sw.min(sh * cap);
        let visible = region_w / scale;
        Self {
            width,
            height,
            scale,
            origin: [cx - MOVIE_WIDTH * 0.5 * scale, cy - MOVIE_HEIGHT * 0.5 * scale],
            edge_offset: (MOVIE_WIDTH - visible) * 0.5,
            safe,
            region: [cx - region_w * 0.5, safe[1], cx + region_w * 0.5, safe[3]],
        }
    }

    /// Movie point -> output pixels (after the edge offset was applied by
    /// the movie itself).
    pub fn to_pixels(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.origin[0] + p[0] * self.scale,
            self.origin[1] + p[1] * self.scale,
        ]
    }

    pub fn contains(&self, rect: [f32; 4], p: [f32; 2]) -> bool {
        p[0] >= rect[0] - 0.5 && p[0] <= rect[2] + 0.5 && p[1] >= rect[1] - 0.5 && p[1] <= rect[3] + 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn sixteen_by_nine_is_the_retail_frame() {
        let l = Layout::compute(1920, 1080, 1.0, DEFAULT_MAX_ASPECT);
        assert!(close(l.scale, 1.5));
        assert_eq!(l.origin, [0.0, 0.0]);
        assert!(close(l.edge_offset, 0.0));
        assert_eq!(l.region, [0.0, 0.0, 1920.0, 1080.0]);
        assert_eq!(l.to_pixels([1280.0, 720.0]), [1920.0, 1080.0]);
    }

    #[test]
    fn gta_area_replaces_the_safe_zone_model() {
        // 21:9 with GTA's HUD held to a centred 16:9 area at safe zone 0.9
        let (w, h) = (2560u32, 1080u32);
        let inset = (1.0 - (16.0 / 9.0) / (w as f32 / h as f32)) * 0.5;
        let area = [inset + 0.05 * (1.0 - 2.0 * inset), 0.05, 1.0 - inset - 0.05 * (1.0 - 2.0 * inset), 0.95];
        let l = Layout::compute_in(w, h, area, DEFAULT_MAX_ASPECT).unwrap();
        assert!(close(l.safe[0], area[0] * w as f32) && close(l.safe[3], 0.95 * h as f32));
        assert!(close(l.region[0], l.safe[0]) && close(l.region[2], l.safe[2]));
        assert!(close(l.scale, 0.9 * h as f32 / MOVIE_HEIGHT));
        // the area as compute models it gives compute's layout
        let m = Layout::compute(1920, 1080, 0.9, DEFAULT_MAX_ASPECT);
        assert_eq!(Layout::compute_in(1920, 1080, [0.05, 0.05, 0.95, 0.95], DEFAULT_MAX_ASPECT).map(|l| l.region.map(|v| v.round())),
                   Some(m.region.map(|v| v.round())));
        assert!(Layout::compute_in(w, h, [0.0; 4], DEFAULT_MAX_ASPECT).is_none());
    }

    #[test]
    fn safe_zone_insets_evenly() {
        let l = Layout::compute(1920, 1080, 0.9, DEFAULT_MAX_ASPECT);
        assert!(close(l.safe[0], 96.0) && close(l.safe[1], 54.0));
        assert!(close(l.safe[2], 1824.0) && close(l.safe[3], 1026.0));
        assert!(close(l.scale, 1.35));
        // The whole movie frame lands inside the safe area at 16:9.
        let a = l.to_pixels([0.0, 0.0]);
        let b = l.to_pixels([1280.0, 720.0]);
        assert!(l.contains(l.safe, a) && l.contains(l.safe, b));
    }

    #[test]
    fn ultrawide_21_9_widens_the_region_and_offsets_the_edge() {
        let l = Layout::compute(2560, 1080, 1.0, DEFAULT_MAX_ASPECT);
        assert!(close(l.scale, 1.5));
        // Movie centred: x=640 at the screen centre.
        assert!(close(l.to_pixels([640.0, 360.0])[0], 1280.0));
        // 2560 px = 1706.67 movie units visible; container moves 213.33 left.
        assert!(close(l.edge_offset, (1280.0 - 2560.0 / 1.5) / 2.0));
        // Left-anchored content at movie x keeps its 16:9 distance from the
        // region's left edge.
        let p = l.to_pixels([100.0 + l.edge_offset, 0.0]);
        assert!(close(p[0] - l.region[0], 150.0));
        assert_eq!(l.region, [0.0, 0.0, 2560.0, 1080.0]);
    }

    #[test]
    fn ultrawide_3440x1440_is_also_21_9() {
        let l = Layout::compute(3440, 1440, 1.0, DEFAULT_MAX_ASPECT);
        assert!(close(l.scale, 2.0));
        assert!(close(l.region[0], 0.0) && close(l.region[2], 3440.0));
        assert!(close(l.edge_offset, (1280.0 - 1720.0) / 2.0));
    }

    #[test]
    fn super_ultrawide_is_capped_and_centred() {
        let l = Layout::compute(5120, 1440, 1.0, DEFAULT_MAX_ASPECT);
        let region_w = 1440.0 * DEFAULT_MAX_ASPECT;
        assert!(close(l.region[2] - l.region[0], region_w));
        assert!(close((l.region[0] + l.region[2]) * 0.5, 2560.0));
        assert!(close(l.edge_offset, (1280.0 - region_w / 2.0) / 2.0));
        // Uncapped: the full width.
        let u = Layout::compute(5120, 1440, 1.0, 0.0);
        assert_eq!(u.region[2] - u.region[0], 5120.0);
    }

    #[test]
    fn narrow_screens_keep_height_and_offset_inwards() {
        for (w, h) in [(1920u32, 1200u32), (1600, 1200), (1280, 1024)] {
            let l = Layout::compute(w, h, 1.0, DEFAULT_MAX_ASPECT);
            assert!(close(l.scale, h as f32 / 720.0));
            assert_eq!(l.region, [0.0, 0.0, w as f32, h as f32]);
            // Visible movie width shrinks below 1280: positive offset.
            let visible = w as f32 / l.scale;
            assert!(l.edge_offset > 0.0 && close(l.edge_offset, (1280.0 - visible) / 2.0));
            // A left-anchored point keeps its 16:9 distance from the left edge.
            let p = l.to_pixels([100.0 + l.edge_offset, 0.0]);
            assert!(close(p[0], 100.0 * l.scale));
            // Vertically the movie fills the height.
            assert!(close(l.to_pixels([0.0, 720.0])[1], h as f32));
        }
    }

    #[test]
    fn degenerate_inputs_stay_finite() {
        for l in [
            Layout::compute(0, 0, f32::NAN, f32::NAN),
            Layout::compute(1, 4000, 0.0, -1.0),
            Layout::compute(8000, 1, 2.0, 100.0),
        ] {
            assert!(l.scale.is_finite() && l.scale > 0.0);
            assert!(l.origin.iter().chain(&l.region).all(|v| v.is_finite()));
            assert!(l.edge_offset.is_finite());
        }
    }
}

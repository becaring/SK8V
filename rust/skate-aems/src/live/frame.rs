//! The "game variables" layer: Skate's skater state as the sound handlers
//! read it. `Frame` is the semantic state SkateV publishes from its Skate
//! session each tick; `SkaterImage` lays it out at the byte offsets of
//! Skate 3's audio skater-state struct (`*(component + 36)`), so the ported
//! handlers read it unchanged.

/// Semantic skater state (Skate world units: metres, m/s, radians).
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub fields: Vec<(usize, Field)>,
    /// Recovered seam lines under each wheel (SkateV material recovery;
    /// `None` keeps the seam type's authored grid). Not part of the retail
    /// skater struct, so it travels beside the byte image.
    pub seam_grids: [Option<SeamGrid>; 4],
    /// Texture-space joints under each wheel (SkateV material recovery):
    /// `Some(true)` when the wheel's path entered a
    /// joint of the visual ground's texture this tick, `Some(false)` on
    /// joint-mapped ground otherwise; `None` leaves the grids above.
    pub seam_hits: [Option<bool>; 4],
    /// Relief crossings this tick, played directly (`seam_voices`); the
    /// wheels' `seam_hits` stay `Some(false)` so Class_Seams never doubles them.
    pub seam_events: Vec<SeamEvent>,
    /// Grain of the ground under the wheels (roughness.py rank, 0..=1, 0.5
    /// the median texture); `None` off measured ground. Shapes the rolling
    /// grains (`Grains::set_ground`).
    pub roughness: Option<f32>,
    /// Share of the wheels (0..=1) on loose ground or vegetation (Skate rows
    /// 7, 9); trims the rolling grains (`Grains::set_ground`).
    pub loose_ground: f32,
}

/// One wheel entering a measured groove, raised line or pit
/// (SkateV ground relief).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SeamEvent {
    pub wheel: u8,
    /// Seconds after the frame's start at which the wheel reached its far side.
    pub at: f32,
    /// Measured depth, 0..=1 (relief level / 63).
    pub level: f32,
    /// Path length the wheel rolled inside the relief, metres.
    pub width: f32,
    /// 1 groove, 2 raised line, 3 pit or chip.
    pub kind: u8,
    /// Wheel speed, m/s.
    pub speed: f32,
    /// The wheel's surface (skater `+620 + 4 * wheel`).
    pub surface: u32,
}

/// World lines `a * x + b * z = phase + k * spacing` in Skate space (metres;
/// `(a, b)` a unit vector in the ground plane). Seam clicks count crossings of
/// these lines instead of the authored grid.
/// Family 0 replaces the authored x axis, family 1 the z axis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SeamFamily {
    pub a: f32,
    pub b: f32,
    pub spacing: f32,
    pub phase: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SeamGrid {
    pub families: [Option<SeamFamily>; 2],
}

/// One value at a skater-state offset.
#[derive(Clone, Copy, Debug)]
pub enum Field {
    U8(u8),
    U32(u32),
    F32(f32),
}

/// 1 KB big-endian image of the skater-state struct.
#[derive(Clone)]
pub struct SkaterImage {
    pub bytes: Vec<u8>,
    pub seam_grids: [Option<SeamGrid>; 4],
    pub seam_hits: [Option<bool>; 4],
}

impl Default for SkaterImage {
    fn default() -> Self {
        SkaterImage { bytes: vec![0; 1024], seam_grids: [None; 4], seam_hits: [None; 4] }
    }
}

impl SkaterImage {
    pub fn r8(&self, off: usize) -> u8 {
        self.bytes.get(off).copied().unwrap_or(0)
    }
    pub fn r32(&self, off: usize) -> u32 {
        self.bytes.get(off..off + 4).map_or(0, |b| u32::from_be_bytes(b.try_into().unwrap()))
    }
    pub fn rf(&self, off: usize) -> f32 {
        f32::from_bits(self.r32(off))
    }
    pub fn w8(&mut self, off: usize, v: u8) {
        if off < self.bytes.len() {
            self.bytes[off] = v;
        }
    }
    pub fn w32(&mut self, off: usize, v: u32) {
        if off + 4 <= self.bytes.len() {
            self.bytes[off..off + 4].copy_from_slice(&v.to_be_bytes());
        }
    }
    pub fn wf(&mut self, off: usize, v: f32) {
        self.w32(off, v.to_bits());
    }

    pub fn from_frame(frame: &Frame) -> SkaterImage {
        let mut s = SkaterImage { seam_grids: frame.seam_grids, seam_hits: frame.seam_hits, ..Default::default() };
        for &(off, f) in &frame.fields {
            match f {
                Field::U8(v) => s.w8(off, v),
                Field::U32(v) => s.w32(off, v),
                Field::F32(v) => s.wf(off, v),
            }
        }
        s
    }
}

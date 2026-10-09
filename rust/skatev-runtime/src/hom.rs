//! Hall of Meat: Skate 3's WipeoutScorer (`Sk8::Score::HoM`), ported from the
//! TU3 executable's semantics as working PC math. Every number it scores with
//! comes from the user's converted tuning (`Hash_9E4C471BC6EAB615` / `default`
//! in skater-collections.json); nothing here is a retail value.
//!
//! One `tick` per Skate tick, after the session advanced, reading the completed
//! physical output (overlay patch 0029 `Session::hom_input`) plus the host
//! entities the rider struck that tick. It produces what the retail scorer
//! publishes to the rest of the game:
//! - the broken-bone slow-mo countdown (Scoring2+200), fed back to the camera;
//! - the scoring results (Scoring2+12336) and, from them, the HUD data the
//!   `homscoring` movie's `HOM_*` natives read (the packer at 82774828).
//!
//! Not ported: the per-region body damage publication (Scoring2+14328, used by
//! the damage overlay, not by `homscoring`) and achievements.
use bevy_math::{Mat4, Vec3};
use skate_host::bridge::hom::HomInput;
use skate_hud::HomData;
use skate_hud::hom::HIT_SLOTS;

/// Simulation step the retail scorer integrates with (one Skate tick).
const TICK: f32 = 1.0 / 60.0;
/// Broken-bone slow-mo countdown in ticks (82DAF7C0).
const BROKEN_TICKS: u32 = 30;
pub const BONES: usize = 25;
/// Damage below this neither applies nor spreads (82DAEB80, 82DAE778).
const DAMAGE_EPSILON: f32 = 0.1;
/// Contact normals steeper than this count as resting on the world (82DAEB30).
const SUPPORT_NORMAL_Y: f32 = 0.33;
/// The contact force doubles before tuning scales it (82DAEB70).
const FORCE_SCALE: f32 = 2.0;
/// A rotation is banked once it exceeds this many degrees (82DAC990).
const BANK_DEGREES: f32 = 30.0;
/// Body tweak turn counting offsets the angle by a quarter turn (82DAD550).
const TURN_OFFSET_DEGREES: f32 = 90.0;
/// Entity hits kept per kind (82DAF828).
const ENTITY_SLOTS: usize = 8;
/// Squared length below which 8296EA60 treats a 2D vector as zero.
const VECTOR_EPSILON: f32 = 1e-4;
/// The scorer's air latch survives landing this long (82DAE5A8).
const AIR_LATCH_SECONDS: f32 = 0.1;

/// Filtered physical categories the scorer distinguishes.
const CATEGORY_AIR: u32 = 2;
const CATEGORY_WIPEOUT: u32 = 4;
const CATEGORY_OFFBOARD_AIR: u32 = 7;

/// Tuning field identities (AttribSys field hashes of the HoM class).
mod field {
    pub const CLASS: &str = "Hash_9E4C471BC6EAB615";
    pub const KEY: &str = "default";
    pub const BONES: &str = "Hash_1F9B5EA147CBEEE9";
    /// Layout order 2100..2980: the twelve point graphs.
    pub const GRAPHS: [&str; 12] = [
        "Hash_A9B81D5BF44762E4", // tweak 1 hold time
        "Hash_95AE883C533FC1EB", // response strength sum
        "Hash_2E61C1BB9D8A8A95", // spin before the wipeout (air)
        "Hash_05B6EAA1D6A19CCE", // entity hits during tweak 2
        "Hash_C2D28EFB70B37684", // hard landing -> leg damage
        "Hash_96CDCC590988982E", // airtime
        "Hash_32B1A3C2961F4E6E", // drop height
        "Hash_36274FBAE087E832", // tweak 0 turns
        "Hash_1D49983B40C59E5A", // tweak 3 turns
        "Hash_B2A997F8579B1998", // speed at the start
        "Hash_6CD9F0CD7675949C", // spin during the wipeout
        "Hash_2451424AD330B08F", // ragdoll time
    ];
    pub const POINTS_LIMIT: &str = "Hash_8FB03ECD5DA26421";
    pub const LEG_SECONDARY: &str = "Hash_72A3CCC3C75697CD";
    pub const IMPULSE_SCALE: &str = "Hash_B7514DE7EBDE9D01";
    pub const TWEAKS: &str = "Hash_5717CFCCCA59BD47";
    pub const TWEAK_DEADZONE: &str = "Hash_9C494E9614FA5ED6";
    /// (type table, default points) for pedestrians, vehicles, DMOs.
    pub const ENTITIES: [(&str, &str); 3] = [
        ("Hash_9E620977A93D37BA", "Hash_96276C7210F1FDD2"),
        ("Hash_FF1CED9B60B28CDA", "Hash_FC8A190E55A10559"),
        ("Hash_D0E1C97AF413A1D3", "Hash_F54CF3579C22165B"),
    ];
}

/// Graph indices (layout order).
mod graph {
    pub const TWEAK1_TIME: usize = 0;
    pub const RESPONSE: usize = 1;
    pub const SPIN_AIR: usize = 2;
    pub const TWEAK2_HITS: usize = 3;
    pub const LANDING: usize = 4;
    pub const AIRTIME: usize = 5;
    pub const DROP: usize = 6;
    pub const TWEAK0_TURNS: usize = 7;
    pub const TWEAK3_TURNS: usize = 8;
    pub const SPEED: usize = 9;
    pub const SPIN_WIPEOUT: usize = 10;
    pub const RAGDOLL_TIME: usize = 11;
}

/// Bones of the left and right leg the hard landing damages (82DADBA0..).
const LEFT_SHIN: usize = 15;
const LEFT_THIGH: usize = 14;
const RIGHT_SHIN: usize = 19;
const RIGHT_THIGH: usize = 18;

/// Entity kinds, in sub-scorer order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Pedestrian = 0,
    Vehicle = 1,
    Dmo = 2,
}

/// `Sk8::PointNegGraphData8`: eight (x, y) knots, clamped outside.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Graph {
    pub x: [f32; 8],
    pub y: [f32; 8],
}

impl Graph {
    /// 82481E10: below the first knot y0, from the last knot on its y, else
    /// linear between the bracketing knots (a flat step takes the right y).
    pub fn eval(&self, x: f32) -> f32 {
        if x < self.x[0] {
            return self.y[0];
        }
        if !(x < self.x[7]) {
            return self.y[7];
        }
        for i in 1..8 {
            if x < self.x[i] {
                let dx = self.x[i] - self.x[i - 1];
                if dx > 0.0 {
                    return (self.y[i] - self.y[i - 1]) / dx * (x - self.x[i - 1]) + self.y[i - 1];
                }
                return self.y[i];
            }
        }
        self.y[0]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bone {
    /// Up to four connected bones (-1: none) and the share of damage they take.
    pub neighbours: [(i32, f32); 4],
    /// Per damage level: points scored and damage threshold.
    pub levels: [(u32, f32); 6],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tweak {
    pub id: u32,
    /// Stick direction.
    pub direction: [f32; 2],
    /// Ticks held before it scores.
    pub hold_ticks: u32,
    pub points: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityPoints {
    pub table: Vec<(i32, u32)>,
    pub default: u32,
}

impl EntityPoints {
    /// 82DAFAB0: the entry of that entity type, else the default.
    fn points(&self, kind: i32) -> u32 {
        self.table.iter().find(|(k, _)| *k == kind).map_or(self.default, |(_, p)| *p)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tuning {
    pub bones: [Bone; BONES],
    pub graphs: [Graph; 12],
    pub points_limit: u32,
    pub leg_secondary: f32,
    pub impulse_scale: f32,
    pub tweaks: Vec<Tweak>,
    pub tweak_deadzone: f32,
    pub entities: [EntityPoints; 3],
}

pub(crate) fn hex_bytes(text: &str) -> Result<Vec<u8>, String> {
    let hex: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if hex.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    hex.chunks(2)
        .map(|c| {
            std::str::from_utf8(c)
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .ok_or_else(|| "invalid hex".to_string())
        })
        .collect()
}

pub(crate) fn be_u32(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|w| u32::from_be_bytes([w[0], w[1], w[2], w[3]]))
        .ok_or_else(|| format!("tuning data too short at {at}"))
}

pub(crate) fn be_f32(b: &[u8], at: usize) -> Result<f32, String> {
    let v = f32::from_bits(be_u32(b, at)?);
    if v.is_finite() { Ok(v) } else { Err(format!("non-finite tuning value at {at}")) }
}

impl Tuning {
    /// From a parsed skater-collections.json.
    pub fn from_collections(json: &serde_json::Value) -> Result<Self, String> {
        let row = json["collections"]
            .as_array()
            .ok_or("skater collections without a collection list")?
            .iter()
            .find(|c| c["class"] == field::CLASS && c["key"] == field::KEY)
            .ok_or("Hall of Meat tuning missing from skater collections")?;
        let fields = &row["fields"];
        let data = |name: &str| -> Result<Vec<u8>, String> {
            hex_bytes(fields[name]["data"].as_str().ok_or_else(|| format!("HoM field {name} missing"))?)
        };
        let items = |name: &str| -> Result<Vec<Vec<u8>>, String> {
            fields[name]["array"]["items"]
                .as_array()
                .ok_or_else(|| format!("HoM array {name} missing"))?
                .iter()
                .map(|v| hex_bytes(v.as_str().ok_or("HoM array item is not hex")?))
                .collect()
        };
        let set = data(field::BONES)?;
        let mut bones = [Bone::default(); BONES];
        for (b, bone) in bones.iter_mut().enumerate() {
            let base = b * 84;
            for n in 0..4 {
                bone.neighbours[n] = (be_u32(&set, base + 4 + 8 * n)? as i32, be_f32(&set, base + 8 + 8 * n)?);
            }
            for l in 0..6 {
                bone.levels[l] = (be_u32(&set, base + 36 + 8 * l)?, be_f32(&set, base + 40 + 8 * l)?);
            }
        }
        let mut graphs = [Graph::default(); 12];
        for (g, name) in graphs.iter_mut().zip(field::GRAPHS) {
            let d = data(name)?;
            for i in 0..8 {
                g.x[i] = be_f32(&d, 16 + 4 * i)?;
                g.y[i] = be_f32(&d, 48 + 4 * i)?;
            }
        }
        let tweaks = items(field::TWEAKS)?
            .iter()
            .map(|d| {
                Ok(Tweak {
                    id: be_u32(d, 0)?,
                    direction: [be_f32(d, 8)?, be_f32(d, 12)?],
                    hold_ticks: be_u32(d, 16)?,
                    points: be_u32(d, 20)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut entities: [EntityPoints; 3] = Default::default();
        for (e, (table, default)) in entities.iter_mut().zip(field::ENTITIES) {
            e.table = items(table)?
                .iter()
                .map(|d| Ok((be_u32(d, 0)? as i32, be_u32(d, 4)?)))
                .collect::<Result<Vec<_>, String>>()?;
            e.default = be_u32(&data(default)?, 0)?;
        }
        Ok(Self {
            bones,
            graphs,
            points_limit: be_u32(&data(field::POINTS_LIMIT)?, 0)?,
            leg_secondary: be_f32(&data(field::LEG_SECONDARY)?, 0)?,
            impulse_scale: be_f32(&data(field::IMPULSE_SCALE)?, 0)?,
            tweaks,
            tweak_deadzone: be_f32(&data(field::TWEAK_DEADZONE)?, 0)?,
            entities,
        })
    }

    /// Bone level for a damage amount: the highest level whose positive
    /// threshold the damage exceeds, else -1.
    fn level(&self, bone: usize, damage: f32) -> i32 {
        for l in (0..6).rev() {
            let threshold = self.bones[bone].levels[l].1;
            if threshold > 0.0 && damage > threshold {
                return l as i32;
            }
        }
        -1
    }

    fn graph(&self, index: usize, x: f32) -> f32 {
        self.graphs[index].eval(x)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Slot {
    level: i32,
    prev_level: i32,
    /// Peak of `accumulated` this wipeout.
    damage: f32,
    /// Damage taken, decaying each tick.
    accumulated: f32,
    tag: u32,
    /// No world contact with an upward normal this tick.
    unsupported: bool,
}

impl Default for Slot {
    fn default() -> Self {
        Self { level: -1, prev_level: -1, damage: 0.0, accumulated: 0.0, tag: 0, unsupported: false }
    }
}

/// Rotation about one pelvis axis (80-byte tracker at +112/+192/+272).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Tracker {
    /// The frame rows {z, x, y} it last saw.
    frame: [[f32; 3]; 3],
    /// Accumulated angle (radians) since the rotation last reversed.
    angle: f32,
    mode: u8,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = length(a);
    (l > 0.0 && l.is_finite()).then(|| a.map(|v| v / l))
}

impl Tracker {
    fn new(mode: u8, frame: [[f32; 3]; 3]) -> Self {
        Self { frame, angle: 0.0, mode }
    }

    /// 82DAC9D0: signed rotation of the reference row about the axis row,
    /// zero when the reference turned more than a quarter turn.
    fn step(&self, new: &[[f32; 3]; 3]) -> f32 {
        let (axis, reference) = match self.mode {
            0 => (1, 2),
            1 => (2, 1),
            2 => (0, 1),
            _ => return 0.0,
        };
        let axis_v = self.frame[axis];
        let previous = self.frame[reference];
        let Some(side) = normalize(cross(axis_v, new[reference])) else { return 0.0 };
        let Some(projected) = normalize(cross(side, axis_v)) else { return 0.0 };
        let c = dot(projected, previous);
        if 0.0 > c {
            return 0.0;
        }
        dot(cross(previous, projected), axis_v).atan2(c)
    }

    /// 82DAC880: wrap into [-pi, pi], accumulate, keep the frame.
    fn advance(&mut self, new: &[[f32; 3]; 3]) -> f32 {
        let mut a = self.step(new);
        if a > std::f32::consts::PI {
            a -= std::f32::consts::TAU;
        } else if a < -std::f32::consts::PI {
            a += std::f32::consts::TAU;
        }
        self.angle += a;
        self.frame = *new;
        a
    }

    /// 82DAC930: degrees banked when the rotation reverses after more than
    /// the banking angle; the new rotation then starts from this tick's step.
    fn bank(&mut self, new: &[[f32; 3]; 3]) -> f32 {
        let base = self.angle.to_degrees().abs();
        let a = self.advance(new);
        if self.angle.to_degrees().abs() < base && base > BANK_DEGREES {
            self.angle = a;
            base
        } else {
            0.0
        }
    }
}

/// One entity sub-scorer (132 bytes at +16/+20/+24).
#[derive(Clone, Debug, Default, PartialEq)]
struct EntityScorer {
    /// (entity id, type, points), first hits only.
    entries: Vec<(u32, i32, u32)>,
    points: u32,
    /// New entities this update.
    hits: u32,
}

impl EntityScorer {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// 82DAF948: every entity struck this tick that is not yet listed.
    fn update(&mut self, table: &EntityPoints, struck: impl Iterator<Item = u32>) {
        self.hits = 0;
        for id in struck {
            if self.entries.iter().any(|e| e.0 == id) {
                continue;
            }
            // GTA entities carry no Skate entity type: the default applies.
            let kind = -1;
            let points = table.points(kind);
            if self.entries.len() < ENTITY_SLOTS {
                self.entries.push((id, kind, points));
                self.points = self.points.wrapping_add(points);
                self.hits += 1;
            }
        }
    }
}

/// A host entity the rider struck this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityHit {
    pub id: u32,
    pub kind: EntityKind,
}

/// Scoring results (Scoring2+12336, written by 82DADC48), the subset the HUD
/// and the log use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub total: u32,
    pub ragdoll_points: i32,
    pub bone_points: u32,
    pub bonus_points: u32,
    pub bones_at_level: [u32; 6],
    pub speed_points: u32,
    pub spin_points: u32,
    pub response_points: i32,
    pub tweak_points: u32,
    pub last_tweak_points: u32,
    pub tweaks_scored: u32,
    pub active_tweak: i32,
    pub tweak_scored: bool,
    pub max_air_ticks: u32,
    pub max_drop: f32,
    pub total_air_points: u32,
    pub total_drop_points: u32,
    pub ragdoll_seconds: f32,
    pub wipeout_seconds: f32,
    pub start_speed: f32,
    pub max_spin_degrees: f32,
    pub distance: f32,
    /// Per entity kind: entries, points, hits this update.
    pub entities: [(Vec<(u32, i32, u32)>, u32, u32); 3],
    pub levels: [i32; BONES],
}

/// One tick's publication.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TickOutput {
    /// Scoring2+200 for the camera: 1.0 while the slow-mo countdown runs.
    pub broken_bone_duration: f32,
    /// Bones that reached a broken level this tick (Scoring2+208).
    pub newly_broken: u32,
    /// A wipeout started this tick.
    pub started: bool,
    /// The wipeout ended this tick (Scoring2 b14646).
    pub ended: bool,
    /// The scorer reset this tick (became airborne, or a fresh wipeout).
    pub reset: bool,
    /// Results were published and the HUD data refreshed this tick.
    pub published: bool,
}

#[derive(Clone, Debug)]
pub struct Scorer {
    pub tuning: Tuning,
    slots: [Slot; BONES],
    trackers: [Tracker; 3],
    entities: [EntityScorer; 3],
    takeoff: [f32; 3],
    peak: [f32; 3],
    start_velocity: [f32; 3],
    wipeout_ticks: u32,
    ragdoll_ticks: u32,
    over_ticks: u32,
    limit_ticks: u32,
    tweak_hold: u32,
    broken_countdown: u32,
    foot_timer: i32,
    speed_points: u32,
    spin_points: u32,
    spin_points_max: u32,
    tweak_points: u32,
    last_tweak_points: u32,
    tweaks_scored: u32,
    response_count: u32,
    spin_sum: f32,
    spin_total: f32,
    spin_max: f32,
    foot_left: f32,
    foot_right: f32,
    response_sum: f32,
    response_max: f32,
    tweak_mark: f32,
    segment_distance: f32,
    segment_distance_max: f32,
    distance: f32,
    distance_since_air: f32,
    /// Per tweak id 0..3: longest hold (s), time held (s), spin while held (deg).
    tweak_longest: [f32; 4],
    tweak_time: [f32; 4],
    tweak_spin: [f32; 4],
    tweak2_hits: u32,
    tweak: i32,
    previous_tweak: i32,
    segment: Segment,
    segment_max: Segment,
    segment_total: Segment,
    in_wipeout: bool,
    air_latch: bool,
    over_latched: bool,
    limit_latched: bool,
    airborne: bool,
    tweak_active: bool,
    tweak_scored: bool,
    speed_recorded: bool,
    timer: f32,
    results: Results,
    /// What the `homscoring` natives read (the HUD data object `pack` fills).
    hud: HomData,
}

/// One unsupported (body in the air) segment: ticks, airtime points, drop
/// points and drop height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Segment {
    ticks: u32,
    air_points: u32,
    drop_points: u32,
    drop: f32,
}

fn truncate(v: f32) -> i64 {
    // fctidz: toward zero; NaN and overflow are not expected from the graphs.
    if v.is_finite() { v as i64 } else { 0 }
}

impl Scorer {
    pub fn new(tuning: Tuning) -> Self {
        let mut s = Self {
            tuning,
            slots: [Slot::default(); BONES],
            trackers: [Tracker::new(2, [[0.0; 3]; 3]), Tracker::new(0, [[0.0; 3]; 3]), Tracker::new(1, [[0.0; 3]; 3])],
            entities: Default::default(),
            takeoff: [0.0; 3],
            peak: [0.0; 3],
            start_velocity: [0.0; 3],
            wipeout_ticks: 0,
            ragdoll_ticks: 0,
            over_ticks: 0,
            limit_ticks: 0,
            tweak_hold: 0,
            broken_countdown: 0,
            foot_timer: -1,
            speed_points: 0,
            spin_points: 0,
            spin_points_max: 0,
            tweak_points: 0,
            last_tweak_points: 0,
            tweaks_scored: 0,
            response_count: 0,
            spin_sum: 0.0,
            spin_total: 0.0,
            spin_max: 0.0,
            foot_left: 0.0,
            foot_right: 0.0,
            response_sum: 0.0,
            response_max: 0.0,
            tweak_mark: 0.0,
            segment_distance: 0.0,
            segment_distance_max: 0.0,
            distance: 0.0,
            distance_since_air: 0.0,
            tweak_longest: [0.0; 4],
            tweak_time: [0.0; 4],
            tweak_spin: [0.0; 4],
            tweak2_hits: 0,
            tweak: -1,
            previous_tweak: -1,
            segment: Segment::default(),
            segment_max: Segment::default(),
            segment_total: Segment::default(),
            in_wipeout: false,
            air_latch: false,
            over_latched: false,
            limit_latched: false,
            airborne: false,
            tweak_active: false,
            tweak_scored: false,
            speed_recorded: false,
            timer: 0.0,
            results: Results::default(),
            hud: HomData::default(),
        };
        s.reset_scoring(&[[0.0; 3]; 3]);
        s
    }

    pub fn results(&self) -> &Results {
        &self.results
    }
    pub fn hud(&self) -> &HomData {
        &self.hud
    }
    pub fn in_wipeout(&self) -> bool {
        self.in_wipeout
    }
    /// Damage level per scorer bone (what the retail HoM presentation reads).
    pub fn bone_levels(&self) -> [i32; BONES] {
        std::array::from_fn(|b| self.slots[b].level)
    }

    /// A new session or respawn: forget everything, including the latches.
    pub fn restart(&mut self) {
        let tuning = self.tuning.clone();
        *self = Self::new(tuning);
    }

    /// The tracker frame rows {z, x, y} of the pelvis axes.
    fn frame(input: &HomInput) -> [[f32; 3]; 3] {
        [input.pelvis[2], input.pelvis[0], input.pelvis[1]]
    }

    /// 82DAD0A8.
    fn reset_scoring(&mut self, frame: &[[f32; 3]; 3]) {
        self.takeoff = [0.0; 3];
        self.peak = [0.0; 3];
        self.start_velocity = [0.0; 3];
        self.wipeout_ticks = 0;
        self.ragdoll_ticks = 0;
        self.over_ticks = 0;
        self.limit_ticks = 0;
        self.broken_countdown = 0;
        self.speed_points = 0;
        self.spin_points = 0;
        self.spin_points_max = 0;
        self.tweak_points = 0;
        self.last_tweak_points = 0;
        self.tweaks_scored = 0;
        self.response_count = 0;
        self.spin_sum = 0.0;
        self.spin_total = 0.0;
        self.spin_max = 0.0;
        self.response_sum = 0.0;
        self.response_max = 0.0;
        self.segment = Segment::default();
        self.segment_max = Segment::default();
        self.segment_total = Segment::default();
        self.segment_distance = 0.0;
        self.segment_distance_max = 0.0;
        self.distance = 0.0;
        self.distance_since_air = 0.0;
        self.over_latched = false;
        self.limit_latched = false;
        self.airborne = false;
        self.speed_recorded = false;
        self.tweak2_hits = 0;
        self.tweak_longest = [0.0; 4];
        self.tweak_time = [0.0; 4];
        self.tweak_spin = [0.0; 4];
        for e in &mut self.entities {
            e.reset();
        }
        for t in &mut self.trackers {
            t.frame = *frame;
            t.angle = 0.0;
        }
        self.slots = [Slot::default(); BONES];
        self.tweak = -1;
        self.previous_tweak = -1;
        self.tweak_mark = self.spin_sum;
        self.tweak_active = false;
        self.tweak_scored = false;
        self.tweak_hold = 0;
    }

    /// 82DAE6F0: damage a bone and spread it to its neighbours.
    fn damage(&mut self, bone: usize, amount: f32, exclude: usize, depth: u32) {
        let slot = &mut self.slots[bone];
        slot.accumulated += amount;
        slot.damage = slot.damage.max(slot.accumulated);
        if depth >= 64 {
            return; // Guards cyclic connection data; the decay ends real chains far sooner.
        }
        for (n, share) in self.tuning.bones[bone].neighbours {
            if n < 0 || n as usize >= BONES || n as usize == exclude {
                continue;
            }
            let spread = share * amount;
            if spread > DAMAGE_EPSILON {
                self.damage(n as usize, spread, bone, depth + 1);
            }
        }
    }

    /// 82DAD448 (all bones) / 82DAD368 (one level).
    fn bone_points(&self, only: Option<i32>) -> u32 {
        let mut sum = 0u32;
        for (b, slot) in self.slots.iter().enumerate() {
            if slot.level < 0 || only.is_some_and(|l| l != slot.level) {
                continue;
            }
            sum = sum.wrapping_add(self.tuning.bones[b].levels[slot.level as usize].0);
        }
        sum
    }

    /// 82DAD640.
    fn update_spin(&mut self) {
        self.spin_total = self.spin_sum
            + self.trackers[1].angle.to_degrees().abs()
            + self.trackers[0].angle.to_degrees().abs()
            + self.trackers[2].angle.to_degrees().abs();
        let g = if self.air_latch { graph::SPIN_AIR } else { graph::SPIN_WIPEOUT };
        self.spin_points = truncate(self.tuning.graph(g, self.spin_total)) as u32;
        self.spin_max = self.spin_max.max(self.spin_total);
        self.spin_points_max = self.spin_points_max.max(self.spin_points);
    }

    fn track_rotation(&mut self, input: &HomInput, frozen: bool) {
        let frame = Self::frame(input);
        if frozen {
            for t in &mut self.trackers {
                t.frame = frame;
            }
        } else {
            for i in 0..3 {
                self.spin_sum += self.trackers[i].bank(&frame);
            }
        }
    }

    fn takeoff(&mut self, input: &HomInput) {
        self.takeoff = input.com_position;
        self.peak = input.com_position;
        self.segment = Segment::default();
        self.tweak_mark = self.spin_sum;
        self.tweak = -1;
        self.tweak_active = false;
        self.tweak_scored = false;
        self.tweak_hold = 0;
    }

    /// 82DAECB8: one tick of an unsupported segment.
    fn segment_tick(&mut self, input: &HomInput, air_reckoning: bool) {
        if input.com_position[1] > self.peak[1] {
            self.peak = input.com_position;
        }
        if !air_reckoning {
            self.segment.ticks += 1;
        }
        let drop = self.peak[1] - input.com_position[1];
        self.segment.drop = drop;
        self.segment.air_points = truncate(self.tuning.graph(graph::AIRTIME, self.segment.ticks as f32 * TICK)) as u32;
        self.segment.drop_points = truncate(self.tuning.graph(graph::DROP, drop)) as u32;
        self.body_tweak(input, air_reckoning);
        self.previous_tweak = self.tweak;
    }

    /// 82DAEBC0: a segment ended.
    fn land(&mut self) {
        let (s, t, m) = (self.segment, &mut self.segment_total, &mut self.segment_max);
        t.ticks = t.ticks.wrapping_add(s.ticks);
        t.air_points = t.air_points.wrapping_add(s.air_points);
        t.drop_points = t.drop_points.wrapping_add(s.drop_points);
        t.drop += s.drop;
        m.ticks = m.ticks.max(s.ticks);
        m.air_points = m.air_points.max(s.air_points);
        m.drop_points = m.drop_points.max(s.drop_points);
        m.drop = m.drop.max(s.drop);
        self.segment = Segment::default();
        self.tweak_active = false;
        self.tweak_scored = false;
        self.tweak_hold = 0;
        self.previous_tweak = self.tweak;
        self.tweak = -1;
        self.tweak_mark = self.spin_sum;
    }

    /// 82DAD728: body tweaks from the wipeout gesture stick.
    fn body_tweak(&mut self, input: &HomInput, air_reckoning: bool) {
        let previous = self.tweak;
        self.tweak_active = false;
        self.tweak_scored = false;
        self.last_tweak_points = 0;
        self.tweak = -1;
        let count = self.tuning.tweaks.len();
        let stick = input.stick;
        let magnitude = stick[0] * stick[0] + stick[1] * stick[1];
        let deadzone = self.tuning.tweak_deadzone;
        let mut matched = false;
        if count > 0 && magnitude > deadzone * deadzone {
            let sector = std::f32::consts::PI / count as f32;
            for i in 0..count {
                let tweak = self.tuning.tweaks[i];
                let d = tweak.direction;
                let squared = d[0] * d[0] + d[1] * d[1];
                // 8296EA60: a near-zero vector has angle zero to anything.
                let angle = if squared > VECTOR_EPSILON && magnitude > VECTOR_EPSILON {
                    ((d[0] * stick[0] + d[1] * stick[1]) / (squared.sqrt() * magnitude.sqrt()))
                        .clamp(-1.0, 1.0)
                        .acos()
                } else {
                    0.0
                };
                if !(angle < sector) {
                    continue;
                }
                self.tweak = tweak.id as i32;
                if self.tweak != previous {
                    self.tweak_mark = self.spin_total;
                    self.tweak_hold = 0;
                    if air_reckoning {
                        self.tweak_scored = true;
                    }
                } else {
                    let turned = self.spin_total - self.tweak_mark;
                    self.tweak_mark = self.spin_total;
                    if air_reckoning {
                        self.tweak_active = true;
                    } else {
                        self.tweak_hold += 1;
                        let id = tweak.id as usize;
                        if id < 4 {
                            self.tweak_spin[id] += turned;
                            self.tweak_time[id] += TICK;
                            self.tweak_longest[id] = self.tweak_longest[id].max(self.tweak_hold as f32 * TICK);
                        }
                    }
                }
                if self.tweak_hold == tweak.hold_ticks {
                    self.tweak_points = self.tweak_points.wrapping_add(tweak.points);
                    self.tweak_scored = true;
                    self.tweaks_scored += 1;
                    self.last_tweak_points = tweak.points;
                }
                if self.tweak_hold >= tweak.hold_ticks {
                    self.tweak_active = true;
                }
                matched = true;
                break;
            }
        }
        if !matched {
            self.tweak_hold = 0;
        }
    }

    /// 82DAD528.
    fn tweak_bonus(&self) -> u32 {
        let turns = |deg: f32| ((deg + TURN_OFFSET_DEGREES) / 360.0).floor();
        let t = &self.tuning;
        let sum = t.graph(graph::TWEAK2_HITS, self.tweak2_hits as f32)
            + t.graph(graph::TWEAK1_TIME, self.tweak_time[1])
            + t.graph(graph::TWEAK3_TURNS, turns(self.tweak_spin[3]))
            + t.graph(graph::TWEAK0_TURNS, turns(self.tweak_spin[0]));
        (truncate(sum) as u32).wrapping_add(self.tweak_points)
    }

    fn ragdoll_points(&self) -> i32 {
        truncate(self.tuning.graph(graph::RAGDOLL_TIME, self.ragdoll_ticks as f32 * TICK)) as i32
    }
    fn response_points(&self) -> i32 {
        truncate(self.tuning.graph(graph::RESPONSE, self.response_sum)) as i32
    }

    /// 82DADA30.
    fn bonus(&self) -> u32 {
        let entity: u32 = self.entities.iter().fold(0u32, |a, e| a.wrapping_add(e.points));
        [
            self.tweak_bonus(),
            self.spin_points_max,
            self.speed_points,
            self.segment.drop_points,
            self.segment.air_points,
            self.segment_total.drop_points,
            self.segment_total.air_points,
            self.response_points() as u32,
            self.ragdoll_points() as u32,
            entity,
        ]
        .iter()
        .fold(0u32, |a, v| a.wrapping_add(*v))
    }

    /// 82DADB48: the hard landing that started the wipeout hurts the legs.
    fn wipeout_started(&mut self) {
        let t = &self.tuning;
        let left = t.graph(graph::LANDING, self.foot_left) * t.impulse_scale;
        let right = t.graph(graph::LANDING, self.foot_right) * t.impulse_scale;
        let secondary = t.leg_secondary;
        self.damage(LEFT_SHIN, left, LEFT_SHIN, 0);
        self.damage(LEFT_THIGH, secondary * left, LEFT_THIGH, 0);
        self.damage(RIGHT_SHIN, right, RIGHT_SHIN, 0);
        self.damage(RIGHT_THIGH, secondary * right, RIGHT_THIGH, 0);
        self.foot_left = 0.0;
        self.foot_right = 0.0;
    }

    /// 82DAE9A8: this tick's contacts damage the bones.
    fn contacts(&mut self, input: &HomInput) {
        self.airborne = true;
        for b in 0..BONES {
            let c = input.bones[b];
            self.slots[b].tag = c.tag;
            self.slots[b].unsupported = !(c.world && c.normal[1] > SUPPORT_NORMAL_Y);
            let amount = c.force * FORCE_SCALE * self.tuning.impulse_scale;
            if amount > DAMAGE_EPSILON {
                self.damage(b, amount, b, 0);
            }
            self.airborne &= self.slots[b].unsupported;
        }
    }

    /// 82DAF1D8: one tick inside the wipeout.
    fn wipeout_tick(&mut self, input: &HomInput, hits: &[EntityHit]) -> u32 {
        let air = input.air_reckoning;
        let was_airborne = self.airborne;
        if !air {
            self.distance += length(input.com_velocity) * TICK;
        }
        for s in &mut self.slots {
            s.prev_level = s.level;
        }
        self.contacts(input);
        for b in 0..BONES {
            let level = self.tuning.level(b, self.slots[b].damage);
            let s = &mut self.slots[b];
            s.level = level;
            s.accumulated *= 0.8;
        }
        if self.airborne && !was_airborne {
            self.takeoff(input);
        } else if was_airborne && !self.airborne {
            self.segment_tick(input, air);
            self.land();
        }
        let before = self.tweak2_hits;
        for (k, e) in self.entities.iter_mut().enumerate() {
            e.update(&self.tuning.entities[k], hits.iter().filter(|h| h.kind as usize == k).map(|h| h.id));
            if self.tweak == 2 || self.previous_tweak == 2 {
                self.tweak2_hits = self.tweak2_hits.wrapping_add(e.hits);
            }
        }
        if before > self.tweak2_hits && self.previous_tweak == 2 {
            self.previous_tweak = -1;
        }
        if self.airborne {
            self.segment_tick(input, air);
            self.segment_distance = 0.0;
        } else if !air {
            let step = length(input.com_velocity) * TICK;
            self.segment_distance += step;
            self.distance_since_air += step;
            self.segment_distance_max = self.segment_distance_max.max(self.segment_distance);
        }
        self.track_rotation(input, air || input.over);
        self.update_spin();
        if !air {
            self.wipeout_ticks += 1;
            if !input.over {
                self.ragdoll_ticks += 1;
            }
            self.over_latched |= input.over;
            if self.over_latched {
                self.over_ticks += 1;
            }
            if !self.limit_latched {
                self.limit_latched = self.bone_points(None) > self.tuning.points_limit;
            }
            if self.limit_latched {
                self.limit_ticks += 1;
            }
        }
        if input.response_strength > 0.0 {
            self.response_sum += input.response_strength;
            self.response_max = self.response_max.max(input.response_strength);
            self.response_count += 1;
        }
        let newly = self.slots.iter().filter(|s| s.level >= 4 && s.prev_level < 4).count() as u32;
        if newly > 0 {
            self.broken_countdown = BROKEN_TICKS;
        } else if self.broken_countdown > 0 {
            self.broken_countdown -= 1;
        }
        newly
    }

    /// 82DAEDC8: outside a wipeout. Only airborne play before a bail scores.
    fn free_tick(&mut self, input: &HomInput) {
        if self.foot_timer >= 0 {
            self.foot_timer += 1;
            if self.foot_timer == 3 {
                self.foot_left = 0.0;
                self.foot_right = 0.0;
                self.foot_timer = -1;
            }
        }
        match input.hard_landing_kind {
            0 => (self.foot_left, self.foot_right, self.foot_timer) = (input.hard_landing_value, input.hard_landing_value, 0),
            1 => (self.foot_left, self.foot_right, self.foot_timer) = (input.hard_landing_value, 0.0, 0),
            2 => (self.foot_left, self.foot_right, self.foot_timer) = (0.0, input.hard_landing_value, 0),
            _ => {}
        }
        if !self.air_latch {
            return;
        }
        let air = input.air_reckoning;
        if !air {
            self.distance += length(input.com_velocity) * TICK;
        }
        let was_airborne = self.airborne;
        self.airborne = matches!(input.category, CATEGORY_AIR | CATEGORY_OFFBOARD_AIR);
        if self.airborne && !was_airborne {
            self.takeoff(input);
        } else if was_airborne && !self.airborne {
            self.segment_tick(input, air);
            self.land();
        }
        if self.airborne {
            self.segment_tick(input, air);
            if !self.speed_recorded || length(input.com_velocity) > length(self.start_velocity) {
                self.start_velocity = input.com_velocity;
            }
            self.speed_points = truncate(self.tuning.graph(graph::SPEED, length(self.start_velocity))) as u32;
            self.speed_recorded = true;
        }
        self.track_rotation(input, air);
        self.update_spin();
        if !air {
            self.wipeout_ticks += 1;
        }
    }

    /// 82DADC48: results.
    fn publish(&mut self) {
        let bone_points = self.bone_points(None);
        let bonus = self.bonus();
        let mut bones_at_level = [0u32; 6];
        for (l, v) in bones_at_level.iter_mut().enumerate() {
            *v = self.bone_points(Some(l as i32));
        }
        self.results = Results {
            total: bone_points.wrapping_add(bonus),
            ragdoll_points: self.ragdoll_points(),
            bone_points,
            bonus_points: bonus,
            bones_at_level,
            speed_points: self.speed_points,
            spin_points: self.spin_points_max,
            response_points: self.response_points(),
            tweak_points: self.tweak_points,
            last_tweak_points: self.last_tweak_points,
            tweaks_scored: self.tweaks_scored,
            active_tweak: if self.tweak_active { self.tweak } else { -1 },
            tweak_scored: self.tweak_scored,
            max_air_ticks: self.segment_max.ticks,
            max_drop: self.segment_max.drop,
            total_air_points: self.segment_total.air_points,
            total_drop_points: self.segment_total.drop_points,
            ragdoll_seconds: self.ragdoll_ticks as f32 * TICK,
            wipeout_seconds: self.wipeout_ticks as f32 * TICK,
            start_speed: length(self.start_velocity),
            max_spin_degrees: self.spin_max,
            distance: self.distance,
            entities: std::array::from_fn(|k| {
                let e = &self.entities[k];
                (e.entries.clone(), e.points, e.hits)
            }),
            // DC48 publishes the level the slot had before this tick's update.
            levels: std::array::from_fn(|b| self.slots[b].prev_level),
        };
    }

    /// 82774828: the HUD data the movie's natives read.
    fn pack(&mut self, metric_flags: [bool; 5]) {
        let r = &self.results;
        let h = &mut self.hud;
        h.score = r.total as i32 as f32;
        h.speed_points = r.speed_points as f32;
        h.drop_points = r.total_drop_points as f32;
        h.air_points = r.total_air_points as f32;
        h.ragdoll_points = r.ragdoll_points as f32;
        h.speed = r.start_speed;
        h.max_drop = r.max_drop;
        h.max_air_seconds = r.max_air_ticks as f32 * TICK;
        h.ragdoll_seconds = r.ragdoll_seconds;
        h.max_spin_degrees = r.max_spin_degrees;
        h.spin_points = r.spin_points as f32;
        h.metric_flags = metric_flags;
        // HUD score slots per collision kind (82774AE0: index % 10).
        let new_hits = |k: usize, scores: &mut [f32; HIT_SLOTS]| -> u32 {
            let (entries, _, hits) = &r.entities[k];
            let first = entries.len().saturating_sub(*hits as usize);
            for (i, e) in entries[first..].iter().enumerate() {
                scores[i % HIT_SLOTS] = e.2 as f32;
            }
            (*hits as i32).max(0) as u32
        };
        h.car_hits = new_hits(EntityKind::Vehicle as usize, &mut h.car_scores);
        h.ped_hits = new_hits(EntityKind::Pedestrian as usize, &mut h.ped_scores);
        h.dmo_hits = new_hits(EntityKind::Dmo as usize, &mut h.dmo_scores);
        h.tweak_scored = r.tweak_scored;
        h.tweak_points = r.last_tweak_points as f32;
        h.skater_hits = 0;
        h.skater_scores[0] = 0.0;
    }

    /// 82DAE4F0 plus the publication that follows it. `metric_flags` are the
    /// HUD's metric panel flags (see `HomData::metric_flags`).
    pub fn tick(&mut self, input: &HomInput, hits: &[EntityHit], metric_flags: [bool; 5]) -> TickOutput {
        let mut out = TickOutput::default();
        let was_wipeout = self.in_wipeout;
        let was_latch = self.air_latch;
        let in_air = matches!(input.category, CATEGORY_AIR | CATEGORY_OFFBOARD_AIR);
        self.in_wipeout =
            input.category == CATEGORY_WIPEOUT && !input.teleport_flags[1] && !input.teleport_flags[0];
        self.timer = if in_air { 0.0 } else { self.timer + TICK };
        self.air_latch = in_air || (was_latch && self.timer < AIR_LATCH_SECONDS);
        let rise = !was_latch && self.air_latch;
        let fresh = !was_latch && !was_wipeout && self.in_wipeout;
        if rise || fresh {
            self.reset_scoring(&Self::frame(input));
            out.reset = true;
            if fresh {
                self.start_velocity = input.com_velocity;
                self.speed_points = truncate(self.tuning.graph(graph::SPEED, length(self.start_velocity))) as u32;
                self.speed_recorded = true;
            }
        }
        if self.in_wipeout != was_wipeout {
            if self.in_wipeout {
                self.wipeout_started();
                out.started = true;
            } else {
                out.ended = true;
                self.publish();
                out.published = true;
            }
        }
        if self.in_wipeout {
            out.newly_broken = self.wipeout_tick(input, hits);
            self.publish();
            out.published = true;
        } else {
            self.free_tick(input);
        }
        out.broken_bone_duration = if self.in_wipeout && self.broken_countdown > 0 { 1.0 } else { 0.0 };
        if out.published {
            self.pack(metric_flags);
        }
        out
    }
}

#[cfg(test)]
#[path = "hom_tests.rs"]
mod tests;

/// Worker side: the scorer after every Skate tick while Hall of Meat is on,
/// its slow-mo publication fed back to the session's camera.
pub struct Driver {
    /// The player's choice (INI start value, then the in-game toggle).
    enabled: bool,
    metrics: bool,
    scorer: Option<Scorer>,
    /// The last tick's publication.
    pub last: TickOutput,
    /// The last tick's filtered physical category (HomInput::category).
    pub category: u32,
    /// The broken-bone x-ray presentation (needs tools/prepare-hom-xray.py).
    xray: Option<crate::xray::Xray>,
}

impl Driver {
    /// `enabled`: the HallOfMeat option at start; `metrics`: show the metric
    /// panel. The tuning loads either way so the in-game toggle can switch
    /// it on.
    pub fn load(data_root: &std::path::Path, enabled: bool, metrics: bool, log: &crate::worker::Log) -> Self {
        let mut d =
            Self { enabled, metrics, scorer: None, last: TickOutput::default(), category: 0, xray: None };
        let path = data_root.join("private/stock/skater-collections.json");
        let collections = std::fs::read(&path)
            .map_err(|e| format!("{}: {e}", path.display()))
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).map_err(|e| e.to_string()));
        let tuning = collections.as_ref().map_err(|e| e.clone()).and_then(Tuning::from_collections);
        if let Ok(j) = &collections {
            match crate::xray::Xray::load(data_root, j) {
                Ok(x) => {
                    log(&format!("Hall of Meat x-ray: {} skeleton triangles", x.triangle_count()));
                    d.xray = Some(x);
                }
                Err(e) => log(&format!("Hall of Meat x-ray unavailable ({e})")),
            }
        }
        match tuning {
            Ok(t) => {
                log(&format!(
                    "Hall of Meat {}: {} body tweaks, metric panel {}",
                    if enabled { "on" } else { "off (toggle in game)" },
                    t.tweaks.len(),
                    if metrics { "on" } else { "off" }
                ));
                d.scorer = Some(Scorer::new(t));
            }
            Err(e) => log(&format!("Hall of Meat unavailable ({e}); playing without it")),
        }
        d
    }

    /// On and the tuning loaded.
    /// The last bail's Hall of Meat total (0 without a scorer).
    pub fn bail_total(&self) -> u32 {
        self.scorer.as_ref().map_or(0, |s| s.results().total)
    }
    pub fn enabled(&self) -> bool {
        self.enabled && self.scorer.is_some()
    }
    /// The tuning loaded (the toggle can switch it on).
    pub fn available(&self) -> bool {
        self.scorer.is_some()
    }
    /// The in-game toggle. Switching off abandons a bail in progress: the
    /// scorer restarts and the slow-mo stops.
    pub fn set_enabled(
        &mut self,
        session: Option<&mut skate_host::bridge::Session>,
        enabled: bool,
        log: &crate::worker::Log,
    ) {
        if enabled == self.enabled {
            return;
        }
        self.enabled = enabled;
        if let Some(s) = self.scorer.as_mut() {
            s.restart();
        }
        self.last = TickOutput::default();
        if let Some(session) = session {
            session.set_hall_of_meat(self.enabled());
            session.set_broken_bone_duration(0.0);
        }
        log(&format!("Hall of Meat {}", if self.enabled() { "on" } else { "off" }));
    }
    pub fn scorer(&self) -> Option<&Scorer> {
        self.scorer.as_ref()
    }
    /// This tick's broken-bone x-ray (GTA space) while a bail runs with Hall
    /// of Meat on; empty otherwise. `eye`: Skate-space camera position.
    pub fn xray(
        &mut self,
        names: &[String],
        bones: &[Mat4],
        root: Mat4,
        eye: Vec3,
        overlay: Option<&crate::xray::Overlay>,
    ) -> Vec<crate::xray::Vertex> {
        if !self.enabled {
            return Vec::new();
        }
        let (Some(scorer), Some(xray)) = (self.scorer.as_ref(), self.xray.as_mut()) else {
            return Vec::new();
        };
        if !scorer.in_wipeout() && !crate::xray::show_all() {
            return Vec::new();
        }
        xray.draw(&scorer.bone_levels(), names, bones, root, eye, overlay)
    }
    pub fn metric_flags(&self) -> [bool; 5] {
        [self.metrics; 5]
    }

    /// After `Session::advance`. `struck`: host entity tags the rider pushed
    /// this tick with their Skate collision group.
    pub fn tick(
        &mut self,
        session: &mut skate_host::bridge::Session,
        struck: &[(u32, u32)],
        log: &crate::worker::Log,
    ) -> Option<TickOutput> {
        if !self.enabled {
            self.last = TickOutput::default();
            return None;
        }
        let scorer = self.scorer.as_mut()?;
        let hits: Vec<EntityHit> = struck
            .iter()
            .filter_map(|&(id, group)| {
                let kind = match group {
                    skate_host::bridge::VEHICLE_GROUP => EntityKind::Vehicle,
                    skate_host::bridge::CHARACTER_GROUP => EntityKind::Pedestrian,
                    _ => return None,
                };
                Some(EntityHit { id, kind })
            })
            .collect();
        let input = session.hom_input();
        self.category = input.category;
        let flags = [self.metrics; 5];
        let out = scorer.tick(&input, &hits, flags);
        session.set_broken_bone_duration(out.broken_bone_duration);
        if out.started {
            log("Hall of Meat: bail");
        }
        if out.newly_broken > 0 {
            let r = scorer.results();
            log(&format!(
                "Hall of Meat: {} bone(s) broke, {} bone points so far",
                out.newly_broken, r.bone_points
            ));
        }
        if out.ended {
            let r = scorer.results();
            log(&format!(
                "Hall of Meat: bail over: {} points ({} bones + {} bonus: speed {}, spin {}, air {}, drop {}, ragdoll {}, tweaks {}, hits {}/{}/{})",
                r.total,
                r.bone_points,
                r.bonus_points,
                r.speed_points,
                r.spin_points,
                r.total_air_points,
                r.total_drop_points,
                r.ragdoll_points,
                r.tweak_points,
                r.entities[0].1,
                r.entities[1].1,
                r.entities[2].1
            ));
        }
        self.last = out;
        Some(out)
    }
}

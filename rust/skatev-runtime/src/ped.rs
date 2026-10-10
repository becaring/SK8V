//! A GTA character driven by Skate's pose (presentation only).
//!
//! The ped model comes from the user's own files via `skatev-ped-export`
//! (local ped cache: `skeleton.json` + `<comp>_<NNN>_<u|r>.svpc`).
//!
//! Retarget (the pinned mashup's rig.rs: `posed * bind^-1 * fit`): each
//! mapped GTA bone wears its Skate joint's full rotation, carried through a
//! bind fit that swings the GTA bind segment onto Skate's. Wrists, head,
//! foot roll and spine twist therefore follow Skate (the earlier aim-only
//! retarget used joint positions and dropped them: 20-60 degree errors).
//!
//! Where the rig differs from the precedent: Skate and GTA place their
//! spine, clavicle and pelvis joints at different anatomical points (Skate
//! hips->chest 0.43 m, GTA 0.30 m; GTA's pelvis joint sits 7 cm above its
//! hip joints, Skate's between them; `rig_compare`). Pinning every GTA joint
//! on Skate's, as the mashup does for its soldier, tore the crotch and
//! shoulders. GTA therefore keeps its own bone lengths (every bone rigid):
//! the pelvis is placed so the hip joints straddle Skate's, and two-bone
//! limb solves put the GTA wrists and ankles on Skate's (the contacts:
//! deck, grabs, plants), bending in the plane Skate's limb bends in.
//! GTA's expression-driven roll bones take the swing/twist split their bind
//! position along the limb implies. The worn shoes then meet the original
//! Skate shoes' support planes, as before.
use crate::skin::{NO_TEXTURE, ShadedTri, light_byte, light_term, smooth_light, to_rgba};
use bevy_math::{Mat3, Mat4, Quat, Vec3};
use std::path::{Path, PathBuf};

/// GTA ped component slots, in `GET_PED_DRAWABLE_VARIATION` order.
pub const COMPONENTS: [&str; 12] = [
    "head", "berd", "hair", "uppr", "lowr", "hand", "feet", "teef", "accs", "task", "decl", "jbib",
];

/// GTA bone, the Skate joint it wears, and the bind segment (GTA child,
/// Skate child) the fit swings together. `None`: the end of a chain, fitted
/// like the bone named in `TERMINAL_FIT`.
type Segment = Option<(&'static str, &'static str)>;
const MAP: [(&str, &str, Segment); 23] = [
    ("SKEL_Pelvis", "HIPS", None),
    ("SKEL_Spine0", "SPINE", Some(("SKEL_Spine1", "SPINE1"))),
    ("SKEL_Spine1", "SPINE1", Some(("SKEL_Spine2", "SPINE2"))),
    ("SKEL_Spine2", "SPINE2", Some(("SKEL_Spine3", "SPINE3"))),
    ("SKEL_Spine3", "SPINE3", Some(("SKEL_Neck_1", "NECK"))),
    ("SKEL_Neck_1", "NECK", Some(("SKEL_Head", "HEAD"))),
    ("SKEL_Head", "HEAD", None),
    ("SKEL_L_Clavicle", "LEFTSHOULDER", Some(("SKEL_L_UpperArm", "LEFTARM"))),
    ("SKEL_L_UpperArm", "LEFTARM", Some(("SKEL_L_Forearm", "LEFTFOREARM"))),
    ("SKEL_L_Forearm", "LEFTFOREARM", Some(("SKEL_L_Hand", "LEFTHAND"))),
    ("SKEL_L_Hand", "LEFTHAND", None),
    ("SKEL_R_Clavicle", "RIGHTSHOULDER", Some(("SKEL_R_UpperArm", "RIGHTARM"))),
    ("SKEL_R_UpperArm", "RIGHTARM", Some(("SKEL_R_Forearm", "RIGHTFOREARM"))),
    ("SKEL_R_Forearm", "RIGHTFOREARM", Some(("SKEL_R_Hand", "RIGHTHAND"))),
    ("SKEL_R_Hand", "RIGHTHAND", None),
    ("SKEL_L_Thigh", "LEFTUPLEG", Some(("SKEL_L_Calf", "LEFTLEG"))),
    ("SKEL_L_Calf", "LEFTLEG", Some(("SKEL_L_Foot", "LEFTFOOT"))),
    ("SKEL_L_Foot", "LEFTFOOT", Some(("SKEL_L_Toe0", "LEFTTOEBASE"))),
    ("SKEL_L_Toe0", "LEFTTOEBASE", None),
    ("SKEL_R_Thigh", "RIGHTUPLEG", Some(("SKEL_R_Calf", "RIGHTLEG"))),
    ("SKEL_R_Calf", "RIGHTLEG", Some(("SKEL_R_Foot", "RIGHTFOOT"))),
    ("SKEL_R_Foot", "RIGHTFOOT", Some(("SKEL_R_Toe0", "RIGHTTOEBASE"))),
    ("SKEL_R_Toe0", "RIGHTTOEBASE", None),
];
/// Chain ends take the fit of the bone before them (the precedent's
/// terminal joints), which keeps wrist and neck seams attached.
const TERMINAL_FIT: [(&str, &str); 3] = [
    ("SKEL_Head", "SKEL_Neck_1"),
    ("SKEL_L_Hand", "SKEL_L_Forearm"),
    ("SKEL_R_Hand", "SKEL_R_Forearm"),
];
/// Both binds stand with flat soles; the ankle->toe bones slope differently
/// (GTA 18, Skate 29 degrees), so swinging them together tipped the GTA sole
/// 11 degrees into the deck. Feet are fitted about the vertical only.
const GROUND_FIT: [&str; 2] = ["SKEL_L_Foot", "SKEL_R_Foot"];
/// Toes end the leg chains and follow their foot's fit.
const TOE_FIT: [(&str, &str); 2] = [("SKEL_L_Toe0", "SKEL_L_Foot"), ("SKEL_R_Toe0", "SKEL_R_Foot")];
/// Two-bone limbs whose end joint is placed on Skate's: (upper, lower, end).
const LIMBS: [(&str, &str, &str); 4] = [
    ("SKEL_L_Thigh", "SKEL_L_Calf", "SKEL_L_Foot"),
    ("SKEL_R_Thigh", "SKEL_R_Calf", "SKEL_R_Foot"),
    ("SKEL_L_UpperArm", "SKEL_L_Forearm", "SKEL_L_Hand"),
    ("SKEL_R_UpperArm", "SKEL_R_Forearm", "SKEL_R_Hand"),
];
/// Roll bones GTA drives by expression: (roll, base, twisted, carries the
/// twisted bone's swing). A roll at the root of the twisted bone (thigh, upper
/// arm, neck) takes its swing without its twist; a roll part-way along the
/// base bone (forearm, neck) takes that fraction of the end's twist.
/// Rigid FK left them behind (thigh roll 8 -> 26 cm, crotch torn 3-4 cm).
const ROLLS: [(&str, &str, &str, bool); 10] = [
    ("RB_L_ThighRoll", "SKEL_Pelvis", "SKEL_L_Thigh", true),
    ("RB_R_ThighRoll", "SKEL_Pelvis", "SKEL_R_Thigh", true),
    ("RB_L_ArmRoll", "SKEL_L_Clavicle", "SKEL_L_UpperArm", true),
    ("RB_R_ArmRoll", "SKEL_R_Clavicle", "SKEL_R_UpperArm", true),
    ("RB_L_ArmRoll_Vest", "SKEL_L_Clavicle", "SKEL_L_UpperArm", true),
    ("RB_R_ArmRoll_Vest", "SKEL_R_Clavicle", "SKEL_R_UpperArm", true),
    ("RB_L_ForeArmRoll", "SKEL_L_Forearm", "SKEL_L_Hand", false),
    ("RB_R_ForeArmRoll", "SKEL_R_Forearm", "SKEL_R_Hand", false),
    ("RB_Neck", "SKEL_Spine3", "SKEL_Neck_1", true),
    ("RB_Neck_1", "SKEL_Neck_1", "SKEL_Head", false),
];

/// Rotation (possibly scaled) + translation as a matrix.
fn rigid(r: Mat3, t: Vec3) -> Mat4 {
    Mat4::from_cols(
        r.x_axis.extend(0.0),
        r.y_axis.extend(0.0),
        r.z_axis.extend(0.0),
        t.extend(1.0),
    )
}

/// Orthonormal frame with `u` first and `v` (orthogonalised) second.
fn frame(u: Vec3, v: Vec3) -> Option<Mat3> {
    let u = u.try_normalize()?;
    let w = u.cross(v).try_normalize()?;
    Some(Mat3::from_cols(u, w.cross(u), w))
}

/// The part of rotation `q` about unit `axis` (swing-twist split, `q =
/// swing * twist`), on the short arc.
fn twist(q: Quat, axis: Vec3) -> Quat {
    let p = axis * Vec3::new(q.x, q.y, q.z).dot(axis);
    let t = Quat::from_xyzw(p.x, p.y, p.z, q.w);
    let t = if t.length_squared() < 1e-12 { Quat::IDENTITY } else { t.normalize() };
    if t.w < 0.0 { -t } else { t }
}

/// Skate's bind palm from its hand mesh (no finger joints): the hand's
/// vertices beyond the wrist, their least-spread axis (palm normal, no sign)
/// and the direction to the farthest fifth (the fingers). GTA axes.
fn palm(verts: &[Vec3], wrist: Vec3, forearm: Vec3) -> Option<(Vec3, Vec3)> {
    let along = (wrist - forearm).try_normalize()?;
    // Reach of Skate's hand mesh beyond the wrist: 0.178 m (`rig_compare`).
    let mut hand: Vec<Vec3> = verts.iter().copied()
        .filter(|v| (*v - wrist).dot(along) > 0.0 && v.distance(wrist) < 0.22)
        .collect();
    if hand.len() < 50 { return None; }
    let centre = hand.iter().copied().sum::<Vec3>() / hand.len() as f32;
    let mut spread = Mat3::ZERO;
    for v in &hand {
        let d = *v - centre;
        spread += Mat3::from_cols(d * d.x, d * d.y, d * d.z);
    }
    let dominant = |m: Mat3, seed: Vec3| (0..100).try_fold(seed, |v, _| (m * v).try_normalize());
    let first = dominant(spread, along)?;
    let second = dominant(spread - Mat3::from_cols(first * first.x, first * first.y, first * first.z) * first.dot(spread * first), first.any_orthonormal_vector())?;
    let normal = first.cross(second).try_normalize()?;
    hand.sort_by(|a, b| (*b - wrist).dot(along).total_cmp(&(*a - wrist).dot(along)));
    let tips = &hand[..hand.len() / 5];
    let fingers = tips.iter().copied().sum::<Vec3>() / tips.len() as f32 - wrist;
    Some((normal, fingers))
}

/// The deck's top surface as a height grid, built once from Skate's board
/// in bind (lying flat: up +Z, nose -Y, GTA axes). The deck is rigid with
/// the board root, so a posed point is looked up through that joint.
struct Deck {
    origin: Vec3,
    top: Vec<f32>,
    /// The deck's underside under each top cell (NaN: none measured).
    bottom: Vec<f32>,
}

impl Deck {
    /// Cell size and reach of the grid (m): the deck is ~0.82 x 0.21 m.
    const CELL: f32 = 0.01;
    const LONG: usize = 96;
    const LAT: usize = 32;

    fn new(board: &[[Vec3; 3]]) -> Option<Self> {
        if board.is_empty() { return None; }
        let origin = board.iter().flatten().copied().sum::<Vec3>() / (board.len() * 3) as f32;
        let cells = Self::LONG * Self::LAT;
        let mut deck = Self { origin, top: vec![f32::NAN; cells], bottom: vec![f32::NAN; cells] };
        // Top faces first; then the underside is the highest down-facing
        // face below the top (trucks and wheels hang lower).
        for down in [false, true] {
        for t in board {
            let nz = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero().z;
            if if down { nz > -0.5 } else { nz < 0.5 } { continue; }
            let uv = t.map(|p| deck.cell(p));
            let lo = |f: fn(&(f32, f32)) -> f32| uv.iter().map(f).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
            let hi = |f: fn(&(f32, f32)) -> f32, n: usize| uv.iter().map(f).fold(f32::MIN, f32::max).ceil().min(n as f32 - 1.0) as usize;
            let (u0, u1, v0, v1) = (lo(|c| c.0), hi(|c| c.0, Self::LONG), lo(|c| c.1), hi(|c| c.1, Self::LAT));
            let area = (uv[1].0 - uv[0].0) * (uv[2].1 - uv[0].1) - (uv[2].0 - uv[0].0) * (uv[1].1 - uv[0].1);
            if area.abs() < 1e-9 { continue; }
            for v in v0..=v1 {
                for u in u0..=u1 {
                    let (x, y) = (u as f32 + 0.5, v as f32 + 0.5);
                    let a = ((uv[1].0 - x) * (uv[2].1 - y) - (uv[2].0 - x) * (uv[1].1 - y)) / area;
                    let b = ((uv[2].0 - x) * (uv[0].1 - y) - (uv[0].0 - x) * (uv[2].1 - y)) / area;
                    if a < 0.0 || b < 0.0 || a + b > 1.0 { continue; }
                    let h = (t[0] * a + t[1] * b + t[2] * (1.0 - a - b)).z - origin.z;
                    let at = v * Self::LONG + u;
                    if down {
                        let (top, cell) = (deck.top[at], &mut deck.bottom[at]);
                        if h < top && (cell.is_nan() || h > *cell) { *cell = h; }
                    } else {
                        let cell = &mut deck.top[at];
                        if cell.is_nan() || h > *cell { *cell = h; }
                    }
                }
            }
        }
        }
        Some(deck)
    }

    /// Grid coordinates (cells) of a bind-frame point.
    fn cell(&self, p: Vec3) -> (f32, f32) {
        let d = p - self.origin;
        (d.y / Self::CELL + Self::LONG as f32 / 2.0, d.x / Self::CELL + Self::LAT as f32 / 2.0)
    }

    /// How far bind-frame `p` is below the deck's top face, if it is within
    /// the deck (at most 4 cm under it: lower is beneath the board, not in it).
    fn depth(&self, p: Vec3) -> Option<f32> {
        let (u, v) = self.cell(p);
        if u < 0.0 || v < 0.0 || u >= Self::LONG as f32 || v >= Self::LAT as f32 { return None; }
        let depth = self.top[v as usize * Self::LONG + u as usize] - (p.z - self.origin.z);
        (depth > 0.0 && depth < 0.04).then_some(depth)
    }

    /// How far bind-frame `p` is in from the deck's outline (m, up to 2 cm).
    fn edge(&self, p: Vec3) -> f32 {
        let (u, v) = self.cell(p);
        let mut near = 2.0f32;
        for dv in -2i32..=2 {
            for du in -2i32..=2 {
                let (x, y) = (u as i32 + du, v as i32 + dv);
                let out = x < 0 || y < 0 || x >= Self::LONG as i32 || y >= Self::LAT as i32
                    || self.top[y as usize * Self::LONG + x as usize].is_nan();
                if out { near = near.min(((du * du + dv * dv) as f32).sqrt() - 0.5); }
            }
        }
        near.max(0.0) * Self::CELL
    }

    /// If bind-frame `p` is inside the deck slab: how far it is under the
    /// top and above the underside (an edge cell without an underside is
    /// taken 1.5 cm thick, about the measured ply).
    fn inside(&self, p: Vec3) -> Option<(f32, f32)> {
        let (u, v) = self.cell(p);
        if u < 0.0 || v < 0.0 || u >= Self::LONG as f32 || v >= Self::LAT as f32 { return None; }
        let at = v as usize * Self::LONG + u as usize;
        let (top, bottom) = (self.top[at], self.bottom[at]);
        let bottom = if bottom.is_nan() { top - 0.015 } else { bottom };
        let h = p.z - self.origin.z;
        (h < top && h > bottom).then_some((top - h, h - bottom))
    }
}

/// A GTA hand for deck contact: its fingers curl (about the bend they have
/// in bind) only as far as they must to stay out of the deck.
struct Hand {
    bone: usize,
    /// Per finger (thumb first): its three bones, the bind curl axis (GTA
    /// model space) and the vertices it carries (with their bone level).
    fingers: Vec<([usize; 3], Vec3, Vec<(Vec3, usize)>)>,
    /// Vertices carried by the hand bone itself (bind positions).
    palm: Vec<Vec3>,
}

impl Hand {
    fn new(model: &PedModel, side: &str) -> Option<Self> {
        let find = |n: String| model.bone_names.iter().position(|b| *b == n);
        let bone = find(format!("SKEL_{side}_Hand"))?;
        let dominant = |v: &Vert| v.b[(0..4).max_by(|&a, &b| v.w[a].total_cmp(&v.w[b])).unwrap()] as usize;
        let mut fingers = Vec::new();
        for k in 0..5 {
            let Some(chain) = (0..3).map(|i| find(format!("SKEL_{side}_Finger{k}{i}"))).collect::<Option<Vec<_>>>() else { continue };
            let chain = [chain[0], chain[1], chain[2]];
            let at = |i: usize| model.bind[chain[i]].w_axis.truncate();
            let Some(axis) = (at(1) - at(0)).cross(at(2) - at(1)).try_normalize() else { continue };
            let verts = model.verts.iter().filter_map(|v| {
                let level = chain.iter().position(|&b| b == dominant(v))?;
                Some((v.p, level))
            }).collect();
            fingers.push((chain, axis, verts));
        }
        // The palm: the hand bone and its helpers (MH_ knuckles), not fingers.
        let finger = |b: usize| fingers.iter().any(|f: &([usize; 3], Vec3, Vec<(Vec3, usize)>)| f.0.contains(&b));
        let under_hand = |mut b: usize| loop {
            if b == bone { return true; }
            match model.parents.get(b) { Some(Some(p)) => b = *p, _ => return false }
        };
        let palm = model.verts.iter().filter(|v| { let d = dominant(v); under_hand(d) && !finger(d) }).map(|v| v.p).collect();
        Some(Self { bone, fingers, palm })
    }
}

/// A solved two-bone limb: new rotations of the upper and lower bones and
/// where the end lands.
struct Limb {
    upper: Mat3,
    lower: Mat3,
    end: Vec3,
}

/// Two-bone solve keeping both bone lengths. `bend` is the hinge axis of
/// the limb's current pose (from the chain itself, or from the bone's
/// transferred rotation where the chain is straight); rotations change by
/// the least turn that keeps that hinge, so twist stays Skate's. The end
/// follows the target up to `stretch` beyond (or inside) reach (the
/// leg-length difference), then falls short rather than tearing the mesh.
fn solve_limb(
    hip: Vec3, knee: Vec3, ankle: Vec3, target: Vec3, bend: Vec3,
    upper: Mat3, lower: Mat3, stretch: f32,
) -> Option<Limb> {
    let (a, b) = (hip.distance(knee), knee.distance(ankle));
    let reach = target - hip;
    let distance = reach.length();
    if !(distance.is_finite() && a > 1e-6 && b > 1e-6 && distance > 1e-6) { return None; }
    let u = reach / distance;
    let d = distance.clamp((a - b).abs() + 1e-5, a + b - 1e-5);
    let along = (a * a - b * b + d * d) / (2.0 * d);
    let height = (a * a - along * along).max(0.0).sqrt();
    // Knee side: perpendicular to the reach in the hinge plane.
    let side = u.cross(bend).try_normalize()?;
    let new_knee = hip + u * along + side * height;
    // Continuous in the target: never a jump where the allowance runs out.
    let end = hip + u * (d + (distance - d).clamp(-stretch, stretch));
    let turn = |from: Vec3, to: Vec3| -> Option<Mat3> {
        Some(frame(to, bend)? * frame(from, bend)?.transpose())
    };
    Some(Limb {
        upper: turn(knee - hip, new_knee - hip)? * upper,
        lower: turn(ankle - knee, end - new_knee)? * lower,
        end,
    })
}

/// `rage_joaat`, the GTA model-name hash.
pub fn joaat(s: &str) -> u32 {
    let mut h: u32 = 0;
    for b in s.bytes().map(|b| b.to_ascii_lowercase()) {
        h = h.wrapping_add(b as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

#[derive(Clone, Debug, Default)]
pub struct Variation {
    pub drawable: [u16; 12],
    pub texture: [u8; 12],
}

#[derive(Clone, Copy)]
struct Vert {
    p: Vec3,
    b: [u16; 4],
    w: [f32; 4],
    c: [f32; 3],
    a: f32,
    uv: [f32; 2],
}

/// Below this mean diffuse alpha a triangle is a cutout (hair cards, lace,
/// decals): drawn only with its real texture (DRAW_POLY has no alpha).
const CUTOUT_ALPHA: f32 = 0.5;

/// The two rigs fitted together (`PedModel::calibrate`).
struct Fit {
    /// Per GTA bone: Skate bind -> GTA bind rotation (fit * GTA bind), so the
    /// posed rotation is Skate's skin rotation times this.
    offset: Vec<Option<Mat3>>,
    /// Skate bind joint positions (GTA axes), by source index.
    source_bind: Vec<Option<Vec3>>,
    /// Midpoint of the GTA hip joints in the pelvis bind frame.
    hips: Vec3,
    /// Per limb: hinge axis in the upper bone's bind frame (GTA's bind bend).
    hinge: Vec<Vec3>,
    /// Per foot: ankle offset (Skate foot bind frame, GTA axes) that sets the
    /// worn GTA sole on the original Skate sole. Both binds stand flat and
    /// the foot now wears Skate's full rotation, so one offset carried by the
    /// foot holds at every angle (the per-tick lowest-vertex fit jumped
    /// between heel and toe: knee pops).
    sole: [Vec3; 2],
    /// Skate's deck, for keeping the shoes out of it.
    deck: Option<Deck>,
}

/// A GTA roll bone and how its expression is reproduced.
struct Roll {
    bone: usize,
    base: usize,
    twisted: usize,
    swing: bool,
    /// Twist share: the roll's position along its segment in bind.
    share: f32,
    /// Bind direction of that segment (GTA model space).
    axis: Vec3,
}

/// Hand contact carried between ticks: solved fresh each tick, the palm push
/// and finger curls snapped between ticks (the hand
/// jumping all over the board; fingertips moved up to 13 cm a tick).
#[derive(Default)]
struct Contact {
    /// Per hand: palm push (board bind frame), smoothed.
    palm: [Vec3; 2],
    /// Per hand: the side it leaves the deck by (+1 up, -1 down, 0 none).
    side: [f32; 2],
    /// Per hand and finger: curl (radians), rate limited.
    curl: [[f32; 5]; 2],
}

pub struct PedModel {
    pub name: String,
    bone_names: Vec<String>,
    parents: Vec<Option<usize>>,
    local: Vec<Mat4>,
    bind: Vec<Mat4>,
    inv_bind: Vec<Mat4>,
    order: Vec<usize>,
    /// Skate joint names the retarget reads, by index used in `mapped`.
    pub sources: Vec<String>,
    /// Per GTA bone: the source index it wears.
    mapped: Vec<Option<usize>>,
    /// Per GTA bone: the bone whose bind fit it takes (itself, or its chain's).
    fit_from: Vec<usize>,
    pelvis: usize,
    hip_sources: [usize; 2],
    /// Skate's truck joints (the deck line) and body joints (ground level
    /// proxy) for hand contact weights.
    truck_sources: [usize; 2],
    /// Skate's board root: its rotation carries the deck's bind up (+Z).
    board_source: usize,
    body_sources: Vec<usize>,
    thighs: [usize; 2],
    limbs: Vec<[usize; 3]>,
    rolls: Vec<Roll>,
    fit: Option<Fit>,
    /// Unsimplified worn shoe geometry for contact fitting, not rendering.
    shoe_vertices: [Vec<Vert>; 2],
    /// Per hand: its bone, and its vertices (hand and fingers) with the
    /// dominant bone of each, for keeping the hand out of the deck.
    hands: [Option<Hand>; 2],
    contact: std::cell::RefCell<Contact>,
    /// Per bone: an expression-driven helper is posed rigidly with a leader
    /// bone instead of its hierarchy parent: (leader, bind-relative transform).
    followers: Vec<Option<(usize, Mat4)>>,
    verts: Vec<Vert>,
    tris: Vec<[u32; 3]>,
    /// Per triangle: texture slot (`NO_TEXTURE` if none) and cutout flag.
    tri_texture: Vec<u16>,
    tri_cutout: Vec<bool>,
    /// Texture slots: (streamed texture dictionary, texture name).
    textures: Vec<(String, String)>,
}

/// Whether two `skeleton.json` folders hold the same bones (name, tag and
/// parent, in order): a pose solved on one can be written to the other.
pub fn same_skeleton(a: &Path, b: &Path) -> bool {
    let bones = |dir: &Path| -> Option<Vec<(String, u64, i64)>> {
        let sk: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("skeleton.json")).ok()?).ok()?;
        Some(sk["bones"].as_array()?.iter()
            .map(|b| (b["name"].as_str().unwrap_or("").to_string(), b["tag"].as_u64().unwrap_or(0), b["parent"].as_i64().unwrap_or(-1)))
            .collect())
    };
    matches!((bones(a), bones(b)), (Some(x), Some(y)) if x == y)
}

/// Finds the cached ped whose folder name hashes to `model_hash`.
pub fn find(cache_root: &Path, model_hash: u32) -> Option<PathBuf> {
    std::fs::read_dir(cache_root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_dir()
                && p.file_name()
                    .is_some_and(|n| joaat(&n.to_string_lossy()) == model_hash)
        })
}

fn read_name(b: &[u8], at: &mut usize) -> Result<String, String> {
    let n = *b.get(*at).ok_or("truncated svpc variant")? as usize;
    let s = b
        .get(*at + 1..*at + 1 + n)
        .ok_or("truncated svpc variant")?;
    *at += 1 + n;
    Ok(String::from_utf8_lossy(s).into_owned())
}

/// The worn diffuse texture of a component: (dictionary stem, texture name).
type WornTexture = Option<(String, String)>;

/// A component drawable: vertices, triangles, worn texture.
type Svpc = (Vec<Vert>, Vec<[u32; 3]>, WornTexture);

fn read_svpc(path: &Path, letter: u8) -> Result<Svpc, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let u32_at = |i: usize| -> Result<u32, String> {
        b.get(i..i + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
            .ok_or("truncated svpc".into())
    };
    if b.get(0..4) != Some(b"SVPC") || u32_at(4)? != 4 {
        return Err(format!(
            "{}: not an SVPC v4 file; re-run tools/export-peds.ps1",
            path.display()
        ));
    }
    let (nv, nt, nvar) = (
        u32_at(8)? as usize,
        u32_at(12)? as usize,
        u32_at(16)? as usize,
    );
    let mut o = 20;
    if b.len() < o + nv * 44 + nt * 12 {
        return Err(format!("{}: truncated", path.display()));
    }
    let f = |i: usize| f32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let h = |i: usize| u16::from_le_bytes(b[i..i + 2].try_into().unwrap());
    let mut verts = Vec::with_capacity(nv);
    for i in 0..nv {
        let at = o + i * 44;
        verts.push(Vert {
            p: Vec3::new(f(at), f(at + 4), f(at + 8)),
            b: [h(at + 12), h(at + 14), h(at + 16), h(at + 18)],
            w: [f(at + 20), f(at + 24), f(at + 28), f(at + 32)],
            c: [0.7; 3],
            a: 1.0,
            uv: [f(at + 36), f(at + 40)],
        });
    }
    o += nv * 44;
    let tris = (0..nt)
        .map(|i| {
            let t = o + i * 12;
            [
                u32_at(t).unwrap(),
                u32_at(t + 4).unwrap(),
                u32_at(t + 8).unwrap(),
            ]
        })
        .collect();
    o += nt * 12;
    // Variants are variable length: letter, dictionary, texture, colours.
    let mut chosen: Option<(usize, String, String)> = None;
    for _ in 0..nvar {
        let here = *b.get(o).ok_or("truncated svpc variant")?;
        let mut at = o + 1;
        let dict = read_name(&b, &mut at)?;
        let texture = read_name(&b, &mut at)?;
        if b.len() < at + nv * 4 {
            return Err(format!("{}: truncated", path.display()));
        }
        if chosen.is_none() || here == letter {
            chosen = Some((at, dict, texture));
        }
        if here == letter {
            break;
        }
        o = at + nv * 4;
    }
    let mut worn = None;
    if let Some((at, dict, texture)) = chosen {
        if !dict.is_empty() && !texture.is_empty() {
            worn = Some((dict, texture));
        }
        for (i, v) in verts.iter_mut().enumerate() {
            let c = &b[at + i * 4..at + 4 + i * 4];
            v.c = [
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
            ];
            v.a = c[3] as f32 / 255.0;
        }
    }
    Ok((verts, tris, worn))
}

impl PedModel {
    pub fn load(dir: &Path, variation: &Variation, budget: usize) -> Result<Self, String> {
        let sk: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("skeleton.json"))
                .map_err(|e| format!("{}: {e}", dir.display()))?,
        )
        .map_err(|e| format!("skeleton.json: {e}"))?;
        let bones = sk["bones"].as_array().ok_or("skeleton.json has no bones")?;
        let names: Vec<String> = bones
            .iter()
            .map(|b| b["name"].as_str().unwrap_or("").to_string())
            .collect();
        let v3 = |v: &serde_json::Value| {
            Vec3::new(
                v[0].as_f64().unwrap_or(0.0) as f32,
                v[1].as_f64().unwrap_or(0.0) as f32,
                v[2].as_f64().unwrap_or(0.0) as f32,
            )
        };
        let parents: Vec<Option<usize>> = bones
            .iter()
            .map(|b| b["parent"].as_i64().filter(|&p| p >= 0).map(|p| p as usize))
            .collect();
        let local: Vec<Mat4> = bones
            .iter()
            .map(|b| {
                let r = &b["r"];
                let q = Quat::from_xyzw(
                    r[0].as_f64().unwrap_or(0.0) as f32,
                    r[1].as_f64().unwrap_or(0.0) as f32,
                    r[2].as_f64().unwrap_or(0.0) as f32,
                    r[3].as_f64().unwrap_or(1.0) as f32,
                )
                .normalize();
                Mat4::from_scale_rotation_translation(v3(&b["s"]), q, v3(&b["t"]))
            })
            .collect();
        // Parents before children.
        let mut order = Vec::with_capacity(names.len());
        let mut placed = vec![false; names.len()];
        while order.len() < names.len() {
            let before = order.len();
            for i in 0..names.len() {
                if !placed[i] && parents[i].is_none_or(|p| placed[p]) {
                    placed[i] = true;
                    order.push(i);
                }
            }
            if order.len() == before {
                return Err("skeleton has a parent cycle".into());
            }
        }
        let mut bind = vec![Mat4::IDENTITY; names.len()];
        for &i in &order {
            bind[i] = parents[i].map_or(local[i], |p| bind[p] * local[i]);
        }
        let inv_bind = bind.iter().map(|m| m.inverse()).collect();

        let idx = |n: &str| names.iter().position(|x| x == n);
        // Skate's pose needs a human rig: every mapped bone and segment end.
        // Animal actors (Director Mode) keep GTA's own animation.
        if let Some(missing) = MAP.iter()
            .flat_map(|m| std::iter::once(m.0).chain(m.2.map(|s| s.0)))
            .find(|n| idx(n).is_none())
        {
            return Err(format!("{}: not a human skeleton (no {missing})", dir.display()));
        }
        let pelvis = idx("SKEL_Pelvis").ok_or("skeleton has no SKEL_Pelvis")?;
        // Quadrupeds name their bones the same: an upright rig has the head
        // within 45 degrees of straight above the pelvis (ped model z is up).
        let spine = bind[idx("SKEL_Head").unwrap_or(pelvis)].w_axis.truncate() - bind[pelvis].w_axis.truncate();
        if spine.z <= spine.truncate().length() {
            return Err(format!("{}: not an upright skeleton (head not above pelvis)", dir.display()));
        }
        let mut sources: Vec<String> = Vec::new();
        let mut src = |n: &str| -> usize {
            if let Some(i) = sources.iter().position(|s| s == n) {
                return i;
            }
            sources.push(n.to_string());
            sources.len() - 1
        };
        let mut mapped = vec![None; names.len()];
        for (bone, source, _) in MAP {
            mapped[idx(bone).unwrap()] = Some(src(source));
        }
        let mut fit_from: Vec<usize> = (0..names.len()).collect();
        for (bone, from) in TERMINAL_FIT.iter().chain(TOE_FIT.iter()) {
            if let (Some(b), Some(f)) = (idx(bone), idx(from)) { fit_from[b] = f; }
        }
        let hip_sources = [src("LEFTUPLEG"), src("RIGHTUPLEG")];
        let truck_sources = [src("TRUCK_FRONT"), src("TRUCK_BACK")];
        let board_source = src("SKATEBOARD_ROOT");
        let body_sources = ["HIPS", "SPINE3", "HEAD", "LEFTLEG", "RIGHTLEG", "LEFTFOOT", "RIGHTFOOT", "LEFTTOEBASE", "RIGHTTOEBASE"]
            .iter().map(|n| src(n)).collect();
        let thighs = [idx("SKEL_L_Thigh").unwrap(), idx("SKEL_R_Thigh").unwrap()];
        let limbs = LIMBS.iter().map(|l| [idx(l.0).unwrap(), idx(l.1).unwrap(), idx(l.2).unwrap()]).collect();
        // GTA drives shirt/belt/bum helpers parented to the thighs by
        // expressions blending body and leg; following the thigh rigidly
        // drags the jacket hem with every knee lift (measured: 30-47 cm
        // stretched edges mid-ollie). They follow the pelvis instead.
        let under_leg = |mut i: usize| {
            while let Some(p) = parents[i] {
                if names[p].starts_with("SKEL_")
                    && (names[p].contains("Thigh") || names[p].contains("Calf"))
                {
                    return true;
                }
                i = p;
            }
            false
        };
        let followers: Vec<Option<(usize, Mat4)>> = (0..names.len())
            .map(|i| {
                (!names[i].starts_with("SKEL_")
                    && ["Shirt", "Belt", "Bum"]
                        .iter()
                        .any(|k| names[i].contains(k))
                    && parents[i].is_some_and(|p| {
                        !(["Shirt", "Belt", "Bum"]
                            .iter()
                            .any(|k| names[p].contains(k)))
                    })
                    && under_leg(i))
                .then(|| (pelvis, bind[pelvis].inverse() * bind[i]))
            })
            .collect();
        // Roll bones: twist share from where the roll sits along its segment.
        let segment_end = |bone: usize| MAP.iter().find(|m| m.0 == names[bone]).and_then(|m| m.2).and_then(|s| idx(s.0));
        let rolls = ROLLS.iter().filter_map(|&(roll, base, twisted, swing)| {
            let (bone, base, twisted) = (idx(roll)?, idx(base)?, idx(twisted)?);
            let (from, to) = if swing { (twisted, segment_end(twisted)?) } else { (base, twisted) };
            let p = |i: usize| bind[i].w_axis.truncate();
            let axis = (p(to) - p(from)).try_normalize()?;
            let share = ((p(bone) - p(from)).dot(axis) / p(to).distance(p(from))).clamp(0.0, 1.0);
            Some(Roll { bone, base, twisted, swing, share, axis })
        }).collect();

        // The worn outfit: one drawable per component slot, chosen texture.
        let ped_name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut verts = Vec::new();
        let mut tris = Vec::new();
        let mut owner = Vec::new();
        let mut tri_texture = Vec::new();
        let mut tri_cutout = Vec::new();
        let mut textures: Vec<(String, String)> = Vec::new();
        for (slot, comp) in COMPONENTS.iter().enumerate() {
            let n = variation.drawable[slot];
            let file = ["u", "r"]
                .iter()
                .map(|race| dir.join(format!("{comp}_{n:03}_{race}.svpc")))
                .find(|p| p.is_file());
            let Some(file) = file else { continue };
            let (v, t, worn) = read_svpc(&file, b'a' + variation.texture[slot])?;
            // Streamed-ped textures live in the texture store as "<ped>/<ytd>"
            // (GTA5.exe formats "%s%s/%s_diff_%03d_%c_%s").
            let slot_texture = worn.map_or(NO_TEXTURE, |(dict, texture)| {
                textures.push((format!("{ped_name}/{dict}"), texture));
                (textures.len() - 1) as u16
            });
            let base = verts.len() as u32;
            verts.extend(v);
            for tri in t {
                let alpha: f32 = tri
                    .iter()
                    .map(|&i| verts[(base + i) as usize].a)
                    .sum::<f32>()
                    / 3.0;
                let cutout = alpha < CUTOUT_ALPHA;
                if cutout && slot_texture == NO_TEXTURE {
                    continue;
                }
                tris.push(tri.map(|i| i + base));
                owner.push(slot);
                tri_texture.push(slot_texture);
                tri_cutout.push(cutout);
            }
        }
        // A skeleton-only entry (`skatev-ped-export --skeletons`) has no
        // meshes at all: usable only where GTA renders the ped itself.
        let skeleton_only = !std::fs::read_dir(dir).is_ok_and(|mut d| {
            d.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "svpc")))
        });
        if tris.is_empty() && !skeleton_only {
            return Err(format!(
                "{}: no component meshes for this outfit",
                dir.display()
            ));
        }
        let mut model = Self {
            bone_names: names.clone(),
            name: ped_name,
            parents,
            local,
            bind,
            inv_bind,
            order,
            sources,
            mapped,
            fit_from,
            pelvis,
            hip_sources,
            truck_sources,
            board_source,
            body_sources,
            thighs,
            limbs,
            rolls,
            fit: None,
            shoe_vertices: Default::default(),
            hands: [None, None],
            contact: Default::default(),
            followers,
            verts,
            tris,
            tri_texture,
            tri_cutout,
            textures,
        };
        for (side, name) in ["SKEL_L_Foot", "SKEL_R_Foot"].iter().enumerate() {
            let Some(foot) = model.bone_names.iter().position(|n| n == name) else { continue };
            model.shoe_vertices[side] = model.verts.iter()
                .filter(|v| crate::skin::mostly_under(&model.parents, foot, &v.b, &v.w))
                .copied().collect();
        }
        for (side, l) in ["L", "R"].iter().enumerate() {
            model.hands[side] = Hand::new(&model, l);
        }
        if budget > 0 && model.tris.len() > budget {
            model.simplify(&owner, budget);
        }
        Ok(model)
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    pub fn textures(&self) -> &[(String, String)] {
        &self.textures
    }

    /// Vertex clustering per component and skinning signature; a cluster
    /// sits at the mean position of its members.
    fn simplify(&mut self, owner: &[usize], budget: usize) {
        let mut slot = vec![usize::MAX; self.verts.len()];
        for (t, tri) in self.tris.iter().enumerate() {
            for &v in tri {
                slot[v as usize] = slot[v as usize].min(owner[t]);
            }
        }
        let mut cell = 0.01f32;
        loop {
            // UV cell (1/16) in the key: vertices across a texture seam never
            // merge (measured: no merged vertex moves its UV by > 0.1; 1/4
            // moved 311 of them, smearing the texture).
            let (map, first) = crate::skin::cluster(self.verts.len(), |i| {
                let v = &self.verts[i];
                (
                    slot[i],
                    (v.p / cell).floor().as_ivec3().to_array(),
                    crate::skin::signature(&v.b, &v.w),
                    [(v.uv[0] * 16.0).floor() as i32, (v.uv[1] * 16.0).floor() as i32],
                )
            });
            let mut sums = vec![(Vec3::ZERO, [0.0f32; 3], [0.0f32; 2], 0.0f32); first.len()];
            for (i, v) in self.verts.iter().enumerate() {
                let acc = &mut sums[map[i] as usize];
                acc.0 += v.p;
                for k in 0..3 {
                    acc.1[k] += v.c[k];
                }
                acc.2[0] += v.uv[0];
                acc.2[1] += v.uv[1];
                acc.3 += 1.0;
            }
            let out: Vec<Vert> = first
                .iter()
                .zip(&sums)
                .map(|(&f, (p, c, uv, n))| Vert {
                    p: *p / *n,
                    c: [c[0] / n, c[1] / n, c[2] / n],
                    uv: [uv[0] / n, uv[1] / n],
                    ..self.verts[f]
                })
                .collect();
            let kept = crate::skin::remap(&self.tris, &map);
            let tex: Vec<u16> = kept.iter().map(|&(k, _)| self.tri_texture[k]).collect();
            let cut: Vec<bool> = kept.iter().map(|&(k, _)| self.tri_cutout[k]).collect();
            let tris: Vec<[u32; 3]> = kept.into_iter().map(|(_, t)| t).collect();
            if tris.len() <= budget || cell > 0.2 {
                self.verts = out;
                self.tris = tris;
                self.tri_texture = tex;
                self.tri_cutout = cut;
                return;
            }
            cell *= 1.25;
        }
    }

    /// Fits this rig to Skate's bind skeleton: `skate_bind` holds Skate's
    /// bind joint positions in GTA axes, indexed like `sources`.
    pub fn calibrate(&mut self, skate_bind: &[Option<Vec3>]) -> Result<(), String> {
        let source = |n: &str| {
            let i = self.sources.iter().position(|s| s == n)?;
            skate_bind.get(i).copied().flatten()
        };
        let need = |n: &str| source(n).ok_or_else(|| format!("Skate bind has no {n}"));
        let idx = |n: &str| self.bone_names.iter().position(|x| x == n);
        let gp = |n: &str| idx(n).map(|i| self.bind[i].w_axis.truncate());
        let g = |n: &str| gp(n).ok_or_else(|| format!("{} has no {n}", self.name));
        let facing = frame(need("SPINE3")? - need("HIPS")?, need("LEFTUPLEG")? - need("RIGHTUPLEG")?)
            .zip(frame(g("SKEL_Spine3")? - g("SKEL_Pelvis")?, g("SKEL_L_Thigh")? - g("SKEL_R_Thigh")?))
            .map(|(s, t)| s * t.transpose())
            .ok_or("degenerate pelvis frames")?;
        let horizontal = |v: Vec3| Vec3::new(v.x, v.y, 0.0).try_normalize();
        let mut fit_rotation = vec![facing; self.bone_names.len()];
        for (bone, skate, segment) in MAP {
            let Some((child, skate_child)) = segment else { continue };
            let (from, to) = (facing * (g(child)? - g(bone)?), need(skate_child)? - need(skate)?);
            let swing = if GROUND_FIT.contains(&bone) {
                horizontal(from).zip(horizontal(to)).map(|(a, b)| Quat::from_rotation_arc(a, b))
            } else {
                from.try_normalize().zip(to.try_normalize()).map(|(a, b)| Quat::from_rotation_arc(a, b))
            };
            fit_rotation[idx(bone).unwrap()] = Mat3::from_quat(swing.unwrap_or(Quat::IDENTITY)) * facing;
        }
        let offset = (0..self.bone_names.len())
            .map(|i| self.mapped[i].map(|_| fit_rotation[self.fit_from[i]] * Mat3::from_mat4(self.bind[i])))
            .collect();
        let p = |i: usize| self.bind[i].w_axis.truncate();
        let hips = Mat3::from_mat4(self.bind[self.pelvis]).transpose()
            * ((p(self.thighs[0]) + p(self.thighs[1])) * 0.5 - p(self.pelvis));
        let hinge = self.limbs.iter().map(|&[upper, lower, end]| {
            let axis = (p(lower) - p(upper)).cross(p(end) - p(lower)).try_normalize()
                .unwrap_or(Mat3::from_mat4(self.bind[upper]) * Vec3::Z);
            Mat3::from_mat4(self.bind[upper]).transpose() * axis
        }).collect();
        let source_bind = (0..self.sources.len()).map(|i| skate_bind.get(i).copied().flatten()).collect();
        self.fit = Some(Fit { offset, source_bind, hips, hinge, sole: [Vec3::ZERO; 2], deck: None });
        Ok(())
    }

    /// `calibrate` from Skate's skinned skater (its inverse bind matrices),
    /// then fit the hands on hand geometry: the forearm's fit left GTA's
    /// palm 29 degrees off Skate's, so fingers went through the deck the
    /// skater carries.
    pub fn calibrate_to(&mut self, reference: &crate::skin::SkinMesh) -> Result<(), String> {
        let joint = |n: &str| reference.inverse_bind_for(n).map(|ib| crate::coords::from_skate(ib.inverse().w_axis.truncate()));
        let bind: Vec<Option<Vec3>> = self.sources.iter().map(|n| joint(n)).collect();
        self.calibrate(&bind)?;
        let bind_world: Vec<Option<Mat4>> = reference.joint_names.iter()
            .map(|n| reference.inverse_bind_for(n).map(|ib| ib.inverse()))
            .collect();
        let verts: Vec<Vec3> = reference.posed_vertices(&bind_world).into_iter().map(crate::coords::from_skate).collect();
        for (side, (hand, forearm)) in [("LEFTHAND", "LEFTFOREARM"), ("RIGHTHAND", "RIGHTFOREARM")].into_iter().enumerate() {
            if let Some(palm) = joint(hand).zip(joint(forearm)).and_then(|(w, f)| palm(&verts, w, f)) {
                self.fit_hand(side, palm);
            }
        }
        let board: Vec<[Vec3; 3]> = reference.skin(&bind_world, Vec3::Z, true).into_iter()
            .map(|t| t.points.map(crate::coords::from_skate)).collect();
        if let Some(f) = self.fit.as_mut() { f.deck = Deck::new(&board); }
        // Soles: Skate's lowest shoe point in bind against GTA's worn shoe
        // with its ankle on Skate's (the ground fit turns feet about the
        // vertical only, so heights carry over unchanged).
        let skate_soles = reference.foot_support(&bind_world);
        for side in 0..2 {
            let ankle = self.limbs[side][2];
            let source = self.mapped[ankle].and_then(|j| bind.get(j).copied().flatten());
            let (Some(source), Some((_, plane))) = (source, skate_soles[side]) else { continue };
            let gta = self.shoe_vertices[side].iter().map(|v| v.p.z).fold(f32::INFINITY, f32::min);
            if !gta.is_finite() { continue; }
            let lowest = source.z + gta - self.bind[ankle].w_axis.z;
            // `plane` is Skate's lowest shoe point along its bind up (Skate
            // y), which is GTA z.
            if let Some(f) = self.fit.as_mut() { f.sole[side] = Vec3::Z * (plane - lowest); }
        }
        Ok(())
    }

    /// Fit GTA hand `side` (0 left) onto Skate's palm (normal, finger
    /// direction); GTA's palm comes from its finger joints, whatever the outfit.
    fn fit_hand(&mut self, side: usize, (normal, fingers): (Vec3, Vec3)) {
        let g = ["L", "R"][side];
        let idx = |n: &str| self.bone_names.iter().position(|x| *x == n);
        let p = |n: String| idx(&n).map(|i| self.bind[i].w_axis.truncate());
        let Some(hand) = idx(&format!("SKEL_{g}_Hand")) else { return };
        let Some(Some(offset)) = self.fit.as_ref().map(|f| f.offset[hand]) else { return };
        let wrist = self.bind[hand].w_axis.truncate();
        let Some(mid) = (1..5).map(|k| p(format!("SKEL_{g}_Finger{k}1"))).sum::<Option<Vec3>>() else { return };
        let (Some(index), Some(little)) = (p(format!("SKEL_{g}_Finger10")), p(format!("SKEL_{g}_Finger40"))) else { return };
        let g_fingers = mid / 4.0 - wrist;
        let Some(g_normal) = g_fingers.cross(little - index).try_normalize() else { return };
        // The least-spread axis has no sign: take the one the forearm fit
        // already points GTA's palm along.
        let forearm_fit = offset * Mat3::from_mat4(self.bind[hand]).transpose();
        let normal = if normal.dot(forearm_fit * g_normal) < 0.0 { -normal } else { normal };
        if let (Some(s), Some(t)) = (frame(normal, fingers), frame(g_normal, g_fingers)) {
            let fit = s * t.transpose();
            if let Some(f) = self.fit.as_mut() { f.offset[hand] = Some(fit * Mat3::from_mat4(self.bind[hand])); }
        }
    }

    /// Skate's skin transform (bind -> posed) of each source joint in GTA
    /// axes, from the skin mesh's joint world matrices (Skate space).
    pub fn skate_pose(&self, reference: &crate::skin::SkinMesh, reference_world: &[Option<Mat4>]) -> Vec<Option<Mat4>> {
        let to_gta = Mat4::from_mat3(crate::coords::basis());
        self.sources.iter().map(|n| {
            let j = reference.joint_names.iter().position(|x| x == n)?;
            let skin = reference_world.get(j).copied().flatten()? * reference.inverse_bind_for(n)?;
            Some(to_gta * skin * to_gta.inverse())
        }).collect()
    }

    /// World matrices (GTA space) for every bone from Skate's skin
    /// transforms (GTA axes, indexed like `sources`; `None` where Skate has
    /// no such joint). Needs `calibrate`.
    pub fn pose(&self, skate: &[Option<Mat4>]) -> Option<Vec<Mat4>> {
        self.solve(skate, [Vec3::ZERO; 4])
    }

    /// `shift`: per limb (feet, then hands), added to where the end lands.
    fn solve(&self, skate: &[Option<Mat4>], shift: [Vec3; 4]) -> Option<Vec<Mat4>> {
        /// Beyond reach the end still meets its target up to this far: the
        /// legs differ by a few millimetres (Skate 0.824 m, GTA 0.820 m).
        const STRETCH: f32 = 0.03;
        let fit = self.fit.as_ref()?;
        let delta = |j: usize| skate.get(j).copied().flatten();
        let target = |j: usize| Some(delta(j)?.transform_point3(fit.source_bind.get(j).copied().flatten()?));
        let n = self.local.len();
        let mut rot: Vec<Option<Mat3>> = (0..n)
            .map(|i| Some(Mat3::from_mat4(delta(self.mapped[i]?)?) * fit.offset[i]?))
            .collect();
        let mut pos: Vec<Option<Vec3>> = vec![None; n];
        // GTA's pelvis joint sits above its hips: place it so the GTA hip
        // joints straddle Skate's.
        let pelvis_rot = rot[self.pelvis]?;
        let hips = (target(self.hip_sources[0])? + target(self.hip_sources[1])?) * 0.5;
        let pelvis = rigid(pelvis_rot, hips - pelvis_rot * fit.hips);
        let mut world = self.fk(pelvis, &rot, &pos);
        // A hand in contact beyond the GTA arm's reach (its shoulder sits
        // ~7 cm above Skate's) dips the clavicle toward it, up to 25 degrees,
        // instead of locking the elbow straight (pops while carrying the board).
        let mut dipped = false;
        for &[upper, lower, end] in &self.limbs[2..] {
            let (Some(j), Some(clavicle)) = (self.mapped[end], self.parents[upper]) else { continue };
            let (Some(goal), Some(r)) = (target(j), rot[clavicle]) else { continue };
            let p = |i: usize| world[i].w_axis.truncate();
            let full = p(upper).distance(p(lower)) + p(lower).distance(p(end));
            let excess = (goal.distance(p(upper)) - 0.98 * full) * self.hand_contact(goal, &target);
            let arm = p(upper) - p(clavicle);
            let Some(axis) = arm.cross(goal - p(clavicle)).try_normalize() else { continue };
            if excess > 0.0 && arm.length() > 1e-3 {
                let angle = (excess / arm.length()).min(25f32.to_radians());
                rot[clavicle] = Some(Mat3::from_axis_angle(axis, angle) * r);
                dipped = true;
            }
        }
        if dipped { world = self.fk(pelvis, &rot, &pos); }
        for (k, &[upper, lower, end]) in self.limbs.iter().enumerate() {
            let (Some(j), Some(r_upper), Some(r_lower)) = (self.mapped[end], rot[upper], rot[lower]) else { continue };
            let Some(mut goal) = target(j) else { continue };
            if k < 2 { goal += shift[k]; }
            let p = |i: usize| world[i].w_axis.truncate();
            let (hip, knee, ankle) = (p(upper), p(lower), p(end));
            // Feet always meet Skate's (deck, ground). A hand meets Skate's
            // only in contact: GTA's shoulders sit ~7 cm above Skate's, and
            // pinning a free hand straightened the elbow 20-30 degrees and
            // amplified every arm swing; free, the arm keeps Skate's angles.
            if k >= 2 {
                let w = self.hand_contact(goal, &target);
                goal = ankle.lerp(goal, w) + shift[k];
            }
            // The chain's own hinge, easing to the transferred bone's bind
            // hinge as the limb straightens (about 1..6 degrees of bend), so
            // the bend plane never flips between ticks.
            // A nearly straight Skate limb bending slightly backwards flips
            // the chain's hinge: keep it on the bind hinge's side, so the
            // blend never cancels out (arm pops up to 14 degrees per tick).
            let fallback = (r_upper * fit.hinge[k]).normalize();
            let chain = (knee - hip).cross(ankle - knee);
            let chain = if chain.dot(fallback) < 0.0 { -chain } else { chain };
            let bent = chain.length() / (hip.distance(knee) * knee.distance(ankle)).max(1e-9);
            let w = ((bent - 0.02) / 0.08).clamp(0.0, 1.0);
            let bend = (fallback * (1.0 - w) + chain.normalize_or_zero() * w).try_normalize().unwrap_or(fallback);
            if let Some(l) = solve_limb(hip, knee, ankle, goal, bend, r_upper, r_lower, STRETCH) {
                rot[upper] = Some(l.upper);
                rot[lower] = Some(l.lower);
                pos[end] = Some(l.end);
            }
        }
        let world = self.fk(pelvis, &rot, &pos);
        for r in &self.rolls {
            rot[r.bone] = Some(self.roll(r, &world));
        }
        let world = self.fk(pelvis, &rot, &pos);
        world.iter().all(|m| m.is_finite()).then_some(world)
    }

    /// How much Skate's hand at `hand` is in contact (0..1): near the deck
    /// (grabs, carrying the board) or at the body's lowest level (plants,
    /// bails). Faded over a band so the arm never snaps between the two.
    fn hand_contact(&self, hand: Vec3, target: &dyn Fn(usize) -> Option<Vec3>) -> f32 {
        // Presentation bands (m): the deck's half width is ~0.11 and the
        // trucks sit ~0.2 m in from its ends.
        let fade = |d: f32, near: f32, far: f32| 1.0 - ((d - near) / (far - near)).clamp(0.0, 1.0);
        let board = match (target(self.truck_sources[0]), target(self.truck_sources[1])) {
            (Some(a), Some(b)) => {
                let axis = (a - b).normalize_or_zero();
                let t = (hand - b).dot(axis).clamp(-0.2, a.distance(b) + 0.2);
                fade(hand.distance(b + axis * t) - 0.11, 0.08, 0.2)
            }
            _ => 0.0,
        };
        let lowest = self.body_sources.iter().filter_map(|&j| target(j)).map(|p| p.z).fold(f32::INFINITY, f32::min);
        let ground = if lowest.is_finite() { fade(hand.z - lowest, 0.15, 0.35) } else { 0.0 };
        board.max(ground)
    }

    /// Forward kinematics from the pelvis with rotation/position overrides;
    /// every other bone keeps its GTA bind offset from its parent (or leader).
    fn fk(&self, pelvis: Mat4, rot: &[Option<Mat3>], pos: &[Option<Vec3>]) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.local.len()];
        let mut done = vec![false; self.local.len()];
        world[self.pelvis] = pelvis;
        done[self.pelvis] = true;
        if let Some(root) = self.parents[self.pelvis] {
            world[root] = pelvis * self.local[self.pelvis].inverse();
            done[root] = true;
        }
        for &i in &self.order {
            if done[i] {
                continue;
            }
            done[i] = true;
            if let Some((leader, relative)) = self.followers[i] {
                world[i] = world[leader] * relative;
                continue;
            }
            let fk = self.parents[i].map_or(Mat4::IDENTITY, |p| world[p]) * self.local[i];
            let at = pos[i].unwrap_or(fk.w_axis.truncate());
            world[i] = match rot[i] {
                Some(r) => rigid(r, at),
                None if pos[i].is_some() => rigid(Mat3::from_mat4(fk), at),
                None => fk,
            };
        }
        world
    }

    /// A roll bone's rotation: its base's change from bind carried rigidly,
    /// then the twisted bone's swing (for rolls on the twisted bone) and its
    /// share of the twist about the segment.
    fn roll(&self, r: &Roll, world: &[Mat4]) -> Mat3 {
        let rot = |i: usize| Mat3::from_mat4(world[i]);
        let bind = |i: usize| Mat3::from_mat4(self.bind[i]);
        let carried = rot(r.base) * bind(r.base).transpose();
        let delta = Quat::from_mat3(&(rot(r.twisted) * (carried * bind(r.twisted)).transpose())).normalize();
        let turn = twist(delta, (carried * r.axis).normalize());
        let share = Quat::IDENTITY.slerp(turn, r.share);
        let q = if r.swing { delta * turn.inverse() * share } else { share };
        Mat3::from_quat(q.normalize()) * carried * bind(r.bone)
    }

    /// Match the worn GTA shoes to the original animated Skate shoes (the
    /// calibrated sole offset, carried by Skate's foot), then keep them out
    /// of the deck: Skate's own soles sink up to 2 cm into it (the deck was
    /// taken at `calibrate_to`).
    pub fn pose_with_foot_support(&self, skate: &[Option<Mat4>]) -> Option<Vec<Mat4>> {
        let fit = self.fit.as_ref()?;
        let rotation = |j: usize| skate.get(j).copied().flatten().map(|m| Mat3::from_mat4(m));
        let mut shift = [Vec3::ZERO; 4];
        for (side, s) in shift.iter_mut().take(2).enumerate() {
            if let Some(r) = self.mapped[self.limbs[side][2]].and_then(rotation) { *s = r * fit.sole[side]; }
        }
        let mut world = self.solve(skate, shift)?;
        let (Some(deck), Some(board)) = (fit.deck.as_ref(), skate.get(self.board_source).copied().flatten()) else { return Some(world) };
        let (up, to_board) = (Mat3::from_mat4(board) * Vec3::Z, board.inverse());
        let mut lifted = false;
        for side in 0..2 {
            // Only a foot standing on the deck (sole along its face).
            let Some(foot) = self.mapped[self.limbs[side][2]].and_then(rotation) else { continue };
            if (foot * Vec3::Z).dot(up) < 0.6 {
                // Any other shoe in the board (bails, catches) leaves it the
                // shortest way, like a hand.
                let shoe: Vec<Vec3> = self.shoe_points(&world, side).into_iter().map(|p| to_board.transform_point3(p)).collect();
                // Never down: the shoe would go into the ground.
                let rot = Mat3::from_mat4(board);
                if let Some((out, _)) = Self::palm_exit(deck, &shoe, 0.0, &|m| (rot * m).normalize_or_zero().z > -0.2) {
                    shift[side] += Mat3::from_mat4(board) * out;
                    lifted = true;
                }
                continue;
            }
            let depth = self.shoe_points(&world, side).into_iter()
                .filter_map(|p| deck.depth(to_board.transform_point3(p)))
                .fold(0f32, f32::max);
            if depth > 0.0005 {
                shift[side] += up * depth;
                lifted = true;
            }
        }
        // Hands: Skate's mitten hand is smaller than GTA's and its grabs
        // reach into the deck (the deck through the
        // palm while carrying the board). The palm leaves the deck the
        // shortest way, through the arm; the fingers then curl round it.
        // Both ease between ticks (`Contact`).
        let mut contact = self.contact.borrow_mut();
        for side in 0..2 {
            let Some(hand) = self.hands[side].as_ref() else { continue };
            let skin = world[hand.bone] * self.inv_bind[hand.bone];
            let palm: Vec<Vec3> = hand.palm.iter().map(|&p| to_board.transform_point3(skin.transform_point3(p))).collect();
            let (target, side_now) = Self::palm_exit(deck, &palm, contact.side[side], &|_| true).unwrap_or((Vec3::ZERO, 0.0));
            if side_now != 0.0 { contact.side[side] = side_now; }
            let eased = contact.palm[side].lerp(target, 0.35);
            contact.palm[side] = if eased.length() < 1e-4 { Vec3::ZERO } else { eased };
            if contact.palm[side] != Vec3::ZERO {
                shift[2 + side] = Mat3::from_mat4(board) * contact.palm[side];
                lifted = true;
            }
        }
        if lifted { world = self.solve(skate, shift)?; }
        for side in 0..2 {
            if let Some(hand) = self.hands[side].as_ref() {
                self.curl_fingers(hand, &mut world, deck, &board, &mut contact.curl[side]);
            }
        }
        Some(world)
    }

    /// The move (board bind frame) taking points out of the deck along its
    /// normal, on the side of the deck's mid-plane their centre is on (an
    /// exit chosen per tick between up, down and over an edge flipped the
    /// arm between ticks: 25 degree pops). `allow` vetoes a direction (feet:
    /// never down into the ground); `None` when clear.
    /// `prefer`: the side used last tick (kept unless the other side is
    /// clearly nearer). Returns the move and its side (+1 up, -1 down).
    fn palm_exit(deck: &Deck, points: &[Vec3], prefer: f32, allow: &dyn Fn(Vec3) -> bool) -> Option<(Vec3, f32)> {
        // A point entering through the side would otherwise need the full
        // ply at once: its push grows with how far in from the outline it is.
        let inside: Vec<(f32, f32)> = points.iter().filter_map(|&p| {
            let (t, b) = deck.inside(p)?;
            let e = deck.edge(p);
            Some((t.min(e), b.min(e)))
        }).collect();
        if inside.is_empty() { return None; }
        // Mid-plane side from the inside points' own depths: nearer the top
        // on average means the points sit in its upper half.
        let (to_top, to_bottom) = inside.iter().fold((0f32, 0f32), |a, i| (a.0 + i.0, a.1 + i.1));
        let up = Vec3::Z * (inside.iter().map(|i| i.0).fold(0f32, f32::max) + 0.002);
        let down = -Vec3::Z * (inside.iter().map(|i| i.1).fold(0f32, f32::max) + 0.002);
        let go_up = if prefer > 0.0 { to_bottom >= 0.6 * to_top } else if prefer < 0.0 { to_top < 0.6 * to_bottom } else { to_top <= to_bottom };
        let (first, other) = if go_up { ((up, 1.0), (down, -1.0)) } else { ((down, -1.0), (up, 1.0)) };
        if allow(first.0) { Some(first) } else if allow(other.0) { Some(other) } else { None }
    }

    /// Curl each finger about its bind bend just as far as keeps it out of
    /// the deck (0..95 degrees, refined to ~1 degree). `curl_state` carries
    /// each finger's curl between ticks: it closes up to 8 and opens up to 4
    /// degrees a tick; a finger no curl clears holds its curl.
    fn curl_fingers(&self, hand: &Hand, world: &mut [Mat4], deck: &Deck, board: &Mat4, curl_state: &mut [f32; 5]) {
        let to_board = board.inverse();
        let carried = Mat3::from_mat4(world[hand.bone]) * Mat3::from_mat4(self.bind[hand.bone]).transpose();
        for (finger, (chain, axis, verts)) in hand.fingers.iter().enumerate() {
            if verts.is_empty() || finger >= 5 { continue; }
            let axis = (carried * *axis).normalize();
            let skin: Vec<Mat4> = chain.iter().map(|&b| world[b] * self.inv_bind[b]).collect();
            let joints: Vec<Vec3> = chain.iter().map(|&b| world[b].w_axis.truncate()).collect();
            let posed: Vec<(Vec3, usize)> = verts.iter().map(|&(p, l)| (skin[l].transform_point3(p), l)).collect();
            // Joint positions and vertices under a curl of `a` per joint.
            let curl = |a: f32| -> (Vec<Vec3>, usize) {
                // Joint l turns by a * (l + 1).
                let turn: [Quat; 3] = std::array::from_fn(|l| Quat::from_axis_angle(axis, a * (l + 1) as f32));
                let mut at = vec![joints[0]];
                for l in 1..3 {
                    at.push(at[l - 1] + turn[l - 1] * (joints[l] - joints[l - 1]));
                }
                let hits = posed.iter().filter(|(p, l)| {
                    let q = at[*l] + turn[*l] * (*p - joints[*l]);
                    deck.inside(to_board.transform_point3(q)).is_some()
                }).count();
                (at, hits)
            };
            let held = curl_state[finger];
            const MAX: f32 = 95.0 * std::f32::consts::PI / 180.0;
            let step = MAX / 8.0;
            let target = if curl(0.0).1 == 0 { 0.0 } else {
                let (mut lo, mut hi) = (0.0, None);
                for i in 1..=8 {
                    let a = step * i as f32;
                    if curl(a).1 == 0 { hi = Some(a); break; }
                    lo = a;
                }
                match hi {
                    Some(mut hi) => {
                        for _ in 0..4 {
                            let mid = (lo + hi) / 2.0;
                            if curl(mid).1 == 0 { hi = mid } else { lo = mid }
                        }
                        hi
                    }
                    None => held,
                }
            };
            let angle = if target > held { target.min(held + 8f32.to_radians()) } else { target.max(held - 4f32.to_radians()) };
            curl_state[finger] = angle;
            if angle == 0.0 { continue; }
            let (at, _) = curl(angle);
            for (l, &b) in chain.iter().enumerate() {
                let r = Mat3::from_quat(Quat::from_axis_angle(axis, angle * (l + 1) as f32)) * Mat3::from_mat4(world[b]);
                world[b] = rigid(r, at[l]);
            }
        }
    }

    /// The worn shoe's vertices under `world` (GTA space).
    fn shoe_points(&self, world: &[Mat4], side: usize) -> Vec<Vec3> {
        let skin: Vec<Mat4> = world.iter().zip(&self.inv_bind).map(|(w, ib)| *w * *ib).collect();
        self.shoe_vertices[side].iter().map(|v| crate::skin::skin_point(&skin, v.p, &v.b, &v.w)).collect()
    }

    pub fn bone_names(&self) -> &[String] {
        &self.bone_names
    }

    /// Bind world matrix of each bone (GTA model space).
    pub fn bind(&self) -> &[Mat4] {
        &self.bind
    }

    /// The GTA bone that wears Skate joint `skate`'s rotation.
    pub fn bone_for_source(&self, skate: &str) -> Option<usize> {
        let gta = MAP.iter().find(|m| m.1 == skate)?.0;
        self.bone_names.iter().position(|n| n == gta)
    }

    /// Skin transform (world * inverse bind, GTA space) of `bone` under `world`.
    pub fn skin_delta(&self, world: &[Mat4], bone: usize) -> Option<Mat4> {
        Some(*world.get(bone)? * *self.inv_bind.get(bone)?)
    }

    /// Skinned, shaded triangles (GTA space).
    pub fn skin(&self, world: &[Mat4], light: Vec3) -> Vec<ShadedTri> {
        let skin: Vec<Mat4> = world
            .iter()
            .zip(&self.inv_bind)
            .map(|(w, ib)| *w * *ib)
            .collect();
        let positions: Vec<Vec3> = self.verts.iter().map(|v| crate::skin::skin_point(&skin, v.p, &v.b, &v.w)).collect();
        let light = light.normalize_or_zero();
        let smooth = smooth_light(&positions, &self.tris, light);
        self.tris
            .iter()
            .enumerate()
            .filter_map(|(k, t)| {
                let [a, b, c] = t.map(|i| positions[i as usize]);
                let n = (b - a).cross(c - a).normalize_or_zero();
                if n == Vec3::ZERO || !a.is_finite() {
                    return None;
                }
                let lit = light_term(n, light);
                let col = t.iter().fold([0.0f32; 3], |acc, &i| {
                    let c = self.verts[i as usize].c;
                    [
                        acc[0] + c[0] / 3.0,
                        acc[1] + c[1] / 3.0,
                        acc[2] + c[2] / 3.0,
                    ]
                });
                Some(ShadedTri {
                    points: [a, b, c],
                    rgba: to_rgba(col, lit),
                    vertex_rgba: t.map(|i| to_rgba(self.verts[i as usize].c, smooth[i as usize])),
                    uv: t.map(|i| self.verts[i as usize].uv),
                    light: t.map(|i| light_byte(smooth[i as usize])),
                    texture: self.tri_texture[k],
                    cutout: self.tri_cutout[k],
                    board: false,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic human rig (world positions, identity rotations): (name,
    /// parent name, position, Skate joint at that position).
    const RIG: [(&str, &str, [f32; 3], &str); 22] = [
        ("SKEL_ROOT", "", [0., 0., 0.], ""),
        ("SKEL_Pelvis", "SKEL_ROOT", [0., 0., 1.0], "HIPS"),
        ("SKEL_Spine0", "SKEL_Pelvis", [0., 0., 1.1], "SPINE"),
        ("SKEL_Spine1", "SKEL_Spine0", [0., 0.01, 1.2], "SPINE1"),
        ("SKEL_Spine2", "SKEL_Spine1", [0., 0.02, 1.3], "SPINE2"),
        ("SKEL_Spine3", "SKEL_Spine2", [0., 0.02, 1.4], "SPINE3"),
        ("SKEL_Neck_1", "SKEL_Spine3", [0., 0.03, 1.55], "NECK"),
        ("SKEL_Head", "SKEL_Neck_1", [0., 0.04, 1.65], "HEAD"),
        ("SKEL_L_Clavicle", "SKEL_Spine3", [0.05, 0.02, 1.5], "LEFTSHOULDER"),
        ("SKEL_L_UpperArm", "SKEL_L_Clavicle", [0.2, 0., 1.5], "LEFTARM"),
        ("SKEL_L_Forearm", "SKEL_L_UpperArm", [0.25, 0.05, 1.25], "LEFTFOREARM"),
        ("SKEL_L_Hand", "SKEL_L_Forearm", [0.3, 0.2, 1.1], "LEFTHAND"),
        ("SKEL_R_Clavicle", "SKEL_Spine3", [-0.05, 0.02, 1.5], "RIGHTSHOULDER"),
        ("SKEL_R_UpperArm", "SKEL_R_Clavicle", [-0.2, 0., 1.5], "RIGHTARM"),
        ("SKEL_R_Forearm", "SKEL_R_UpperArm", [-0.25, 0.05, 1.25], "RIGHTFOREARM"),
        ("SKEL_R_Hand", "SKEL_R_Forearm", [-0.3, 0.2, 1.1], "RIGHTHAND"),
        ("SKEL_L_Thigh", "SKEL_Pelvis", [0.1, 0., 0.95], "LEFTUPLEG"),
        ("SKEL_L_Calf", "SKEL_L_Thigh", [0.1, -0.05, 0.5], "LEFTLEG"),
        ("SKEL_L_Foot", "SKEL_L_Calf", [0.1, 0., 0.1], "LEFTFOOT"),
        ("SKEL_R_Thigh", "SKEL_Pelvis", [-0.1, 0., 0.95], "RIGHTUPLEG"),
        ("SKEL_R_Calf", "SKEL_R_Thigh", [-0.1, -0.05, 0.5], "RIGHTLEG"),
        ("SKEL_R_Foot", "SKEL_R_Calf", [-0.1, 0., 0.1], "RIGHTFOOT"),
    ];
    const EXTRA: [(&str, &str, [f32; 3], &str); 3] = [
        ("SKEL_L_Toe0", "SKEL_L_Foot", [0.1, -0.15, 0.02], "LEFTTOEBASE"),
        ("SKEL_R_Toe0", "SKEL_R_Foot", [-0.1, -0.15, 0.02], "RIGHTTOEBASE"),
        ("RB_L_ForeArmRoll", "SKEL_L_Forearm", [0.275, 0.125, 1.175], ""),
    ];

    fn write_rig(dir: &Path, rig: &[(&str, &str, [f32; 3], &str)]) {
        let pos = |n: &str| rig.iter().find(|b| b.0 == n).map_or([0.; 3], |b| b.2);
        let bones: Vec<_> = rig.iter().map(|&(name, parent, p, _)| {
            let q = pos(parent);
            serde_json::json!({
                "name": name, "tag": 0,
                "parent": rig.iter().position(|b| b.0 == parent).map_or(-1, |i| i as i64),
                "t": [p[0] - q[0], p[1] - q[1], p[2] - q[2]], "r": [0., 0., 0., 1.], "s": [1., 1., 1.],
            })
        }).collect();
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("skeleton.json"), serde_json::json!({ "bones": bones }).to_string()).unwrap();
    }

    fn rig() -> Vec<(&'static str, &'static str, [f32; 3], &'static str)> {
        RIG.iter().chain(EXTRA.iter()).copied().collect()
    }

    /// The synthetic rig loaded and fitted to a Skate bind at `skate(p)`.
    fn fitted(tag: &str, skate: impl Fn(Vec3) -> Vec3) -> (PedModel, Vec<(&'static str, &'static str, [f32; 3], &'static str)>) {
        let root = std::env::temp_dir().join(format!("skatev-ped-{tag}-{}", std::process::id()));
        let rig = rig();
        write_rig(&root, &rig);
        let mut model = PedModel::load(&root, &Variation::default(), 0).unwrap();
        std::fs::remove_dir_all(&root).ok();
        // Skate's board, held at the left hand (trucks across the palm).
        let board = [("TRUCK_FRONT", [0.3, 0.42, 1.06]), ("TRUCK_BACK", [0.3, -0.02, 1.06])];
        let bind: Vec<Option<Vec3>> = model.sources.iter()
            .map(|s| rig.iter().map(|b| (b.3, b.2)).chain(board).find(|b| b.0 == s).map(|b| skate(Vec3::from(b.1))))
            .collect();
        model.calibrate(&bind).unwrap();
        (model, rig)
    }

    fn at(model: &PedModel, world: &[Mat4], name: &str) -> Mat4 {
        world[model.bone_names.iter().position(|n| n == name).unwrap()]
    }

    #[test]
    fn skeleton_only_entry_poses_a_human_rig_and_rejects_others() {
        let (model, rig) = fitted("bind", |p| p);
        assert_eq!(model.triangle_count(), 0);
        // Skate at its bind gives back the rig.
        let world = model.pose(&vec![Some(Mat4::IDENTITY); model.sources.len()]).unwrap();
        for (i, b) in rig.iter().enumerate() {
            assert!(world[i].w_axis.truncate().distance(Vec3::from(b.2)) < 1e-4, "{}", b.0);
            assert!(Mat3::from_mat4(world[i]).abs_diff_eq(Mat3::IDENTITY, 1e-4), "{}", b.0);
        }
        let root = std::env::temp_dir().join(format!("skatev-ped-test-{}", std::process::id()));
        // No toes: not the rig Skate's pose drives (animals keep GTA's animation).
        write_rig(&root.join("animal"), &RIG);
        let err = PedModel::load(&root.join("animal"), &Variation::default(), 0).err().unwrap();
        assert!(err.contains("not a human skeleton"), "{err}");
        // Head ahead of the pelvis, not above it: a quadruped.
        let lying: Vec<_> = rig.iter().map(|&(n, p, [x, y, z], s)| (n, p, [x, z, y], s)).collect();
        write_rig(&root.join("quadruped"), &lying);
        let err = PedModel::load(&root.join("quadruped"), &Variation::default(), 0).err().unwrap();
        assert!(err.contains("not an upright skeleton"), "{err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_rigid_skate_motion_moves_the_whole_rig_rigidly() {
        let (model, rig) = fitted("rigid", |p| p);
        let m = Mat4::from_rotation_translation(Quat::from_euler(bevy_math::EulerRot::ZXY, 1.2, 0.4, -0.3), Vec3::new(5., -2., 3.));
        let world = model.pose(&vec![Some(m); model.sources.len()]).unwrap();
        for (i, b) in rig.iter().enumerate() {
            assert!(world[i].w_axis.truncate().distance(m.transform_point3(Vec3::from(b.2))) < 1e-4, "{}", b.0);
            assert!(Mat3::from_mat4(world[i]).abs_diff_eq(Mat3::from_mat4(m), 1e-4), "{}", b.0);
        }
    }

    #[test]
    fn ends_land_on_skate_joints_with_gta_bone_lengths() {
        // A smaller Skate skeleton: GTA keeps its own lengths and bends.
        let (model, _) = fitted("scaled", |p| p * 0.96);
        let world = model.pose(&vec![Some(Mat4::IDENTITY); model.sources.len()]).unwrap();
        for (gta, skate) in [("SKEL_L_Hand", [0.3, 0.2, 1.1]), ("SKEL_R_Foot", [-0.1, 0., 0.1]), ("SKEL_L_Foot", [0.1, 0., 0.1])] {
            let p = at(&model, &world, gta).w_axis.truncate();
            assert!(p.distance(Vec3::from(skate) * 0.96) < 1e-4, "{gta} at {p}");
        }
        let len = |a: &str, b: &str| at(&model, &world, a).w_axis.truncate().distance(at(&model, &world, b).w_axis.truncate());
        assert!((len("SKEL_L_Thigh", "SKEL_L_Calf") - Vec3::new(0., -0.05, -0.45).length()).abs() < 1e-4);
        assert!((len("SKEL_L_UpperArm", "SKEL_L_Forearm") - Vec3::new(0.05, 0.05, -0.25).length()).abs() < 1e-4);
        // The knee still bends forward (-y), as both binds do.
        assert!(at(&model, &world, "SKEL_L_Calf").w_axis.y < -0.05);
        // The right hand touches nothing: the arm keeps Skate's angles (here
        // the bind's) rather than reaching for the smaller skeleton's hand.
        for bone in ["SKEL_R_UpperArm", "SKEL_R_Forearm", "SKEL_R_Hand"] {
            assert!(Mat3::from_mat4(at(&model, &world, bone)).abs_diff_eq(Mat3::IDENTITY, 1e-4), "{bone}");
        }
    }

    #[test]
    fn wrist_twist_reaches_the_hand_and_shares_into_the_forearm_roll() {
        let (model, _) = fitted("twist", |p| p);
        let mut skate = vec![Some(Mat4::IDENTITY); model.sources.len()];
        let hand = Vec3::new(0.3, 0.2, 1.1);
        let axis = (hand - Vec3::new(0.25, 0.05, 1.25)).normalize();
        let turn = Mat4::from_translation(hand) * Mat4::from_quat(Quat::from_axis_angle(axis, 1.2)) * Mat4::from_translation(-hand);
        skate[model.sources.iter().position(|s| s == "LEFTHAND").unwrap()] = Some(turn);
        let world = model.pose(&skate).unwrap();
        let angle = |name: &str| Quat::from_mat3(&Mat3::from_mat4(at(&model, &world, name))).angle_between(Quat::IDENTITY);
        assert!((angle("SKEL_L_Hand") - 1.2).abs() < 1e-3);
        assert!(angle("SKEL_L_Forearm") < 1e-3);
        assert!((angle("RB_L_ForeArmRoll") - 0.6).abs() < 1e-3, "roll {}", angle("RB_L_ForeArmRoll"));
    }

    #[test]
    fn joaat_matches_known_model_hashes() {
        // Michael / Franklin / Trevor model hashes (GTA V).
        assert_eq!(joaat("player_zero"), 0x0D7114C9);
        assert_eq!(joaat("PLAYER_ONE"), 0x9B22DBAF);
        assert_eq!(joaat("player_two"), 0x9B810FA2);
    }

    #[test]
    fn limb_solve_keeps_lengths_bend_side_and_world_independence() {
        let (hip, knee, ankle) = (Vec3::new(0., 0., 1.), Vec3::new(0., 0.2, 0.6), Vec3::new(0., 0., 0.2));
        let bend = (knee - hip).cross(ankle - knee).normalize();
        let target = ankle + Vec3::new(0.02, 0., 0.035);
        let l = solve_limb(hip, knee, ankle, target, bend, Mat3::IDENTITY, Mat3::IDENTITY, 0.0).unwrap();
        let new_knee = hip + l.upper * (knee - hip);
        assert!((new_knee.distance(hip) - knee.distance(hip)).abs() < 1e-5);
        assert!(l.end.distance(target) < 1e-6);
        assert!((new_knee + l.lower * (ankle - knee)).distance(target) < 1e-5);
        assert!(new_knee.y > 0.0, "knee stays on its side");
        let r = Quat::from_rotation_x(1.1) * Quat::from_rotation_z(0.7);
        let m = Mat3::from_quat(r);
        let rotated = solve_limb(r * hip, r * knee, r * ankle, r * target, r * bend, m, m, 0.0).unwrap();
        assert!(rotated.end.distance(r * l.end) < 1e-5);
        assert!(rotated.upper.abs_diff_eq(m * l.upper, 1e-5), "the solve must not assume world up");
        // Out of reach: straight, finite, short of the target.
        let far = solve_limb(hip, knee, ankle, Vec3::new(0., 0., -1.), bend, Mat3::IDENTITY, Mat3::IDENTITY, 0.03).unwrap();
        assert!(far.upper.is_finite() && far.end.z > -0.0 && far.end.z < 0.2);
    }
}

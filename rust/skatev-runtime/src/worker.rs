//! The Skate session thread. Mirrors the pinned mashup's `iw4l-skate` worker
//! (crates/render_anim/src/skate.rs): one retained session, fixed-period
//! advancement owned by Skate (`Session::period`), collision rebuilt off the
//! simulation thread as the skater moves.
use crate::coords;
use crate::lifecycle::{self, Monitor, Phase};
use crate::ped;
use crate::quirk::{self, BackwardsManAssist, BackwardsManConfig};
use crate::skin;
use crate::world::{self, StaticWorld};
use bevy_math::{Mat4, Quat, Vec3};
use skate_host::bridge::{CollisionBuilder, InputFrame, Pose, PreparedCollision, Session};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

pub const STATUS_LOADING: u32 = 0;
pub const STATUS_READY: u32 = 1;
pub const STATUS_ACTIVATING: u32 = 2;
pub const STATUS_ACTIVE: u32 = 3;
pub const STATUS_ERROR: u32 = 4;

#[derive(Clone, Copy, Default, Debug)]
pub struct Pad {
    pub connected: bool,
    pub packet: u32,
    pub buttons: u16,
    pub triggers: [u8; 2],
    pub left: [i16; 2],
    pub right: [i16; 2],
}

pub enum Job {
    /// The host found GTA's physics level (None: it is gone): the static map
    /// is read from it (`live.rs`).
    PhysicsLevel(Option<crate::live::Level>),
    Activate {
        spawn: Vec3,
        heading: f32,
        aspect: f32,
        /// Lifecycle entry: off the board, board in hand, awaiting player Y
        /// (patch 0007). False: the original on-board teleport.
        offboard: bool,
    },
    /// Lifecycle: where the GTA player is while Skate is inactive (GTA
    /// space, ground contact). The first one prepares the session there; later
    /// ones keep the collision working set around the player.
    Track {
        at: Vec3,
        heading: f32,
    },
    Step {
        dt: f32,
        pad: Pad,
        aspect: f32,
    },
    Suspend,
    DynamicBodies(Vec<crate::dynamic::HostBody>),
    /// Posed ragdoll parts of the nearby GTA peds (their hitboxes).
    PedParts(Vec<crate::ped_bodies::PedPart>),
    /// Present Skate's pose on this GTA character (None: Skate's own skater).
    Character(Option<CharacterRequest>),
    /// RetailQuirk::BackwardsMan configuration.
    QuirkConfig(BackwardsManConfig),
    /// Host-requested RetailQuirk trigger (keyboard shortcut).
    QuirkTrigger(quirk::RetailQuirk),
    /// The in-game Hall of Meat toggle.
    HallOfMeat(bool),
    /// The automatic bail reset's time limit (seconds; None: retail,
    /// infinity: none).
    BailLimit(Option<f32>),
    /// The air-time teleport limit (None: Skate's own; infinity: none).
    AirLimit(Option<f32>),
    /// The SkateV ramp-lip rule (patch 0033; false: retail admission).
    LipRule(bool),
    Difficulty(u32),
    CameraType(u32),
    /// Metres behind a car's rear the skitch grab line sits.
    SkitchStandoff(f32),
    /// Play a showcase line (`line.rs`) from its start while skating.
    PlayLine(PathBuf),
}

#[derive(Clone, Debug)]
pub struct CharacterRequest {
    pub cache_root: PathBuf,
    pub model_hash: u32,
    pub variation: ped::Variation,
    pub budget: usize,
    pub live_skeleton: Option<PathBuf>,
}

/// Latest publication, already in GTA space.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub tick: u64,
    pub root: Vec3,
    pub root_rotation: Quat,
    pub heading: f32,
    pub deck: Vec3,
    pub deck_rotation: Quat,
    pub velocity: Vec3,
    pub camera: Option<(Vec3, Vec3, f32)>,
    pub state: String,
    pub score: skate_host::ScoreView,
    /// Skate's skinned skater + board, GTA space (presentation only).
    pub mesh: Arc<Vec<skin::ShadedTri>>,
    /// Character texture slots (dictionary, texture) indexed by `ShadedTri::texture`.
    pub textures: Arc<Vec<(String, String)>>,
    /// Ped presentation: character bone world matrices, GTA space.
    pub character_pose: Arc<Vec<Mat4>>,
    pub board_pose: Option<[Mat4; 7]>,
    pub board_entity: Option<Mat4>,
    /// Host entities the board touched: (tag, point, normal), GTA space.
    pub hits: Vec<(u32, Vec3, Vec3)>,
    /// Recent distinct trick labels from Skate's scorer, newest last.
    pub tricks: Vec<String>,
    /// Retail-quirk observation, GTA space.
    pub quirk: Quirk,
    /// RetailQuirk::BackwardsMan assist phase (0 idle).
    pub assist_phase: u32,
    /// Hall of Meat broken-bone x-ray, GTA space triangle list (empty: none).
    pub xray: Arc<Vec<crate::xray::Vertex>>,
}

/// What the host shows this frame: the latest tick blended back towards the
/// one before by the fixed step's leftover (`Shared::blend`). Skate ticks at
/// 60 Hz; at a higher frame rate the ped otherwise held still for 2-3 frames
/// and then jumped, which the Rockstar Editor records as judder. One tick
/// behind; a jump (a teleport, a reset) shows the latest as it is.
pub struct Shown {
    pub root: Vec3,
    pub root_rotation: Quat,
    pub heading: f32,
    pub deck: Vec3,
    pub deck_rotation: Quat,
    pub camera: Option<(Vec3, Vec3, f32)>,
    pub character_pose: Arc<Vec<Mat4>>,
    pub board_pose: Option<[Mat4; 7]>,
    pub board_entity: Option<Mat4>,
}

fn mix_mat(a: Mat4, b: Mat4, t: f32) -> Mat4 {
    let (sa, ra, ta) = a.to_scale_rotation_translation();
    let (sb, rb, tb) = b.to_scale_rotation_translation();
    Mat4::from_scale_rotation_translation(sa.lerp(sb, t), ra.slerp(rb, t), ta.lerp(tb, t))
}

/// The published state right after one pass of the worker.
#[derive(Clone)]
pub struct Held {
    pub step: u64,
    pub snapshot: Snapshot,
    pub previous: Option<Snapshot>,
    pub blend: f32,
}

impl Shared {
    pub fn shown(&self) -> Shown {
        shown(&self.snapshot, self.previous.as_ref(), self.blend)
    }

    /// The snapshot and what to show as they stood after host step `step`
    /// (the newest kept pass at or before it), or the latest when none is.
    /// sv_get_output asks for the step before the frame's own: that one had a
    /// whole frame to finish, so GTA's main thread never waits for Skate's
    /// tick (it waited up to 5 ms a frame: 30 fps lost on slower CPUs), and
    /// every frame is exactly one step behind (no uneven motion).
    pub fn at_step(&self, step: u64) -> (&Snapshot, Shown) {
        match self.held.iter().rev().find(|h| h.step <= step) {
            Some(h) => (&h.snapshot, shown(&h.snapshot, h.previous.as_ref(), h.blend)),
            None => (&self.snapshot, self.shown()),
        }
    }
}

fn shown(s: &Snapshot, previous: Option<&Snapshot>, blend: f32) -> Shown {
    {
        let latest = Shown {
            root: s.root,
            root_rotation: s.root_rotation,
            heading: s.heading,
            deck: s.deck,
            deck_rotation: s.deck_rotation,
            camera: s.camera,
            character_pose: Arc::clone(&s.character_pose),
            board_pose: s.board_pose,
            board_entity: s.board_entity,
        };
        let Some(p) = previous.filter(|p| {
            s.tick > p.tick && s.tick - p.tick <= 3 && p.root.distance_squared(s.root) < 4.0
        }) else {
            return latest;
        };
        // blend 1 is the latest tick; 0 the one before.
        let t = blend.clamp(0.0, 1.0);
        let back = |a: Vec3, b: Vec3| a.lerp(b, t);
        Shown {
            root: back(p.root, s.root),
            root_rotation: p.root_rotation.slerp(s.root_rotation, t),
            heading: p.heading + ((s.heading - p.heading + 540.0).rem_euclid(360.0) - 180.0) * t,
            deck: back(p.deck, s.deck),
            deck_rotation: p.deck_rotation.slerp(s.deck_rotation, t),
            camera: match (p.camera, s.camera) {
                (Some((pa, fa, va)), Some((pb, fb, vb))) => {
                    Some((pa.lerp(pb, t), fa.lerp(fb, t).normalize_or(fb), va + (vb - va) * t))
                }
                _ => s.camera,
            },
            character_pose: if p.character_pose.len() == s.character_pose.len() {
                Arc::new(p.character_pose.iter().zip(s.character_pose.iter()).map(|(a, b)| mix_mat(*a, *b, t)).collect())
            } else {
                latest.character_pose
            },
            board_pose: match (p.board_pose, s.board_pose) {
                (Some(a), Some(b)) => Some(std::array::from_fn(|i| mix_mat(a[i], b[i], t))),
                _ => s.board_pose,
            },
            board_entity: match (p.board_entity, s.board_entity) {
                (Some(a), Some(b)) => Some(mix_mat(a, b, t)),
                _ => s.board_entity,
            },
        }
    }
}

/// Skate's off-board/launch internals in GTA space (see overlay patch 0004).
#[derive(Clone, Copy, Default, Debug)]
pub struct Quirk {
    pub board_state: u32,
    pub deck_velocity: Vec3,
    pub com_trajectory_velocity: Vec3,
    pub use_com_velocity: bool,
    pub launch_tick: u64,
    pub launch_start_velocity: Vec3,
    pub launch_com_velocity: Vec3,
    pub launch_com_branch: bool,
    pub launch_overridden: bool,
}

/// Skinned presentation of Skate's own skater + board model.
struct Presenter {
    mesh: skin::SkinMesh,
    map: Option<Vec<Option<usize>>>,
    /// GTA character wearing Skate's pose (fitted to Skate's bind skeleton).
    character: Option<ped::PedModel>,
    textures: Arc<Vec<(String, String)>>,
    /// The host renders the player ped; only Skate's board is presented.
    board_only: bool,
    /// Ped presentation: the character's bone world matrices (GTA space),
    /// one per skeleton bone, for the host to pose the real ped with.
    pose: Arc<Vec<Mat4>>,
}

impl Presenter {
    fn load(
        data_root: &std::path::Path,
        budget: usize,
        board_only: bool,
        log: &Log,
    ) -> Option<Self> {
        let path = data_root.join("private").join("skater.glb");
        match skin::SkinMesh::load(&path, budget) {
            Ok(mesh) => {
                log(&format!(
                    "skater mesh {}: {} triangles ({} board, budget {budget}), {} joints",
                    path.display(),
                    mesh.triangle_count(),
                    mesh.board_triangle_count(),
                    mesh.joint_names.len()
                ));
                Some(Self {
                    mesh,
                    map: None,
                    character: None,
                    textures: Arc::default(),
                    board_only,
                    pose: Arc::default(),
                })
            }
            Err(e) => {
                log(&format!(
                    "skater mesh unavailable ({e}); presentation falls back to the host"
                ));
                None
            }
        }
    }

    fn draw(&mut self, p: &Pose, log: &Log) -> Vec<skin::ShadedTri> {
        if self.map.is_none() {
            let map: Vec<Option<usize>> = self
                .mesh
                .joint_names
                .iter()
                .map(|n| p.names.iter().position(|x| x == n))
                .collect();
            let missing: Vec<&str> = self
                .mesh
                .joint_names
                .iter()
                .zip(&map)
                .filter(|(_, m)| m.is_none())
                .map(|(n, _)| n.as_str())
                .collect();
            log(&format!(
                "skater joints posed by Skate: {}/{}; following ancestors: {missing:?}",
                map.iter().filter(|m| m.is_some()).count(),
                map.len()
            ));
            for name in [
                "HIPS",
                "HEAD",
                "LEFTFOOT",
                "RIGHTFOOT",
                "SKATEBOARD_ROOT",
                "TRUCK_FRONT",
            ] {
                if let Some(i) = p.names.iter().position(|n| n == name) {
                    let w = coords::from_skate((p.root * p.bones[i]).w_axis.truncate());
                    log(&format!(
                        "pose joint {name} at GTA ({:.3}, {:.3}, {:.3})",
                        w.x, w.y, w.z
                    ));
                }
            }
            self.map = Some(map);
        }
        let map = self.map.as_ref().unwrap();
        // Pose bones are relative to the animation root (measured: unrooted
        // skinning lands at the map origin), so world = root * bone. The GLB
        // went through Blender: its bone-local axes are rotated -90 deg about
        // X from Skate's native frames (documented by the donor renderer), so
        // a skinning joint is native_bone * basis(X, -Z, Y).
        let blender = Mat4::from_cols(
            bevy_math::Vec4::X,
            -bevy_math::Vec4::Z,
            bevy_math::Vec4::Y,
            bevy_math::Vec4::W,
        );
        let world: Vec<Option<Mat4>> = map
            .iter()
            .map(|i| i.and_then(|i| p.bones.get(i).map(|b| p.root * *b * blender)))
            .collect();
        let light = coords::to_skate(Vec3::new(0.35, 0.25, 0.9));
        let board_only = self.board_only || self.character.is_some();
        let mut tris: Vec<skin::ShadedTri> = self
            .mesh
            .skin(&world, light, board_only)
            .into_iter()
            .map(|t| skin::ShadedTri {
                points: t.points.map(coords::from_skate),
                ..t
            })
            .collect();
        if let Some(model) = self.character.as_ref() {
            // Skate's skin transforms (rotation and position) drive the GTA skeleton.
            let skate = model.skate_pose(&self.mesh, &world);
            if let Some(world) = model.pose_with_foot_support(&skate) {
                if self.board_only {
                    // GTA renders the real ped; publish the pose instead of skinning.
                    self.pose = Arc::new(world);
                } else {
                    tris.extend(model.skin(&world, Vec3::new(0.35, 0.25, 0.9)));
                }
            }
        }
        tris
    }

    fn set_character(&mut self, request: Option<CharacterRequest>, log: &Log) {
        self.character = None;
        self.textures = Arc::default();
        self.pose = Arc::default();
        let Some(r) = request else { return };
        // With Presentation=Ped GTA draws the ped: without a usable model it
        // keeps GTA's own animation (the host only writes a pose published
        // for this skeleton). Otherwise the runtime draws Skate's skater.
        let fallback = if self.board_only { "the ped keeps GTA's animation" } else { "showing Skate's skater" };
        // The cache holds the models setup read from GTA's archives; a mod
        // that replaces the model (or an add-on ped) brings its own skeleton,
        // and a pose solved on the cached one is never written to it.
        let dir = match (ped::find(&r.cache_root, r.model_hash), r.live_skeleton.clone()) {
            (Some(dir), Some(live)) if !ped::same_skeleton(&dir, &live) => {
                log(&format!("character model {:#010x}: the live skeleton differs from the ped cache's (a replaced model?); posing the live one", r.model_hash));
                live
            }
            (Some(dir), _) => dir,
            (None, Some(live)) => {
                log(&format!("character model {:#010x} not in ped cache {}; posing its live skeleton", r.model_hash, r.cache_root.display()));
                live
            }
            (None, None) => {
                log(&format!(
                    "character model {:#010x} not in ped cache {}; {fallback}",
                    r.model_hash,
                    r.cache_root.display()
                ));
                return;
            }
        };
        match ped::PedModel::load(&dir, &r.variation, r.budget) {
            Ok(model) if model.triangle_count() == 0 && !self.board_only => log(&format!(
                "character {} is skeleton-only in the ped cache; {fallback}",
                model.name
            )),
            Ok(mut model) => {
                if let Err(e) = model.calibrate_to(&self.mesh) {
                    log(&format!("character {} cannot be fitted to Skate's skeleton ({e}); {fallback}", model.name));
                    return;
                }
                log(&format!(
                    "character {} wears Skate's pose: {} triangles, outfit {:?} / {:?}",
                    model.name,
                    model.triangle_count(),
                    r.variation.drawable,
                    r.variation.texture
                ));
                log(&format!("character textures: {:?}", model.textures()));
                self.textures = Arc::new(model.textures().to_vec());
                self.character = Some(model);
            }
            Err(e) => log(&format!("character load failed ({e}); {fallback}")),
        }
    }
}

pub struct Shared {
    pub status: u32,
    pub message: String,
    pub snapshot: Snapshot,
    /// The tick before `snapshot`, and how far the host's frame is from it
    /// towards `snapshot` (the fixed step's leftover, 0..1). See `Shown`.
    pub previous: Option<Snapshot>,
    pub blend: f32,
    /// `Job::Step`s the worker has finished.
    pub steps_done: u64,
    /// The published state as each of the last two passes left it, by
    /// `steps_done`: sv_get_output shows the previous frame's step (see `at_step`).
    pub held: std::collections::VecDeque<Held>,
    pub lifecycle: LifecycleShared,
    /// Original HUD publication (hud.rs).
    pub hud: crate::hud::HudShared,
    /// Player records (best bails and lines) and Skate's labels for them.
    pub records: crate::records::Records,
    pub record_labels: std::collections::HashMap<String, String>,
    /// Contact exchange: solved impulses on host entities not yet drained.
    pub host_impulses: Vec<crate::SvDynamicImpulse>,
}

/// Lifecycle publication (ABI 8 `SvLifecycleState`).
#[derive(Clone, Copy, Default, Debug)]
pub struct LifecycleShared {
    /// The Skate session exists (built once per process).
    pub prepared: bool,
    /// The session is being built in the background.
    pub preparing: bool,
    /// A collision working set is being built off the simulation thread.
    pub building: bool,
    pub phase: u32,
    pub skate_state: u32,
    pub board_possession: u32,
    /// GTA handle of the vehicle being skitched (0: none).
    pub skitch_vehicle: u32,
    /// Session builds this process (1 after preparation, never more).
    pub session_builds: u32,
    /// Collision working sets installed (preparation + streaming).
    pub collision_builds: u32,
    pub last_prepare_ms: f32,
    pub last_activate_ms: f32,
    pub collision_centre: Vec3,
}

pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// VerboseLog (Dev menu): the periodic lines (perf every 2 s, the skater
/// trace, collision streaming) go to the log only while this is set.
pub static VERBOSE_LOG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn verbose() -> bool {
    VERBOSE_LOG.load(std::sync::atomic::Ordering::Relaxed)
}

/// Thread priority inside GTA's busy process: the Skate tick must not be
/// starved by game threads; collision rebuilds must not compete with them.
#[cfg(windows)]
fn set_thread_priority(priority: i32) {
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    unsafe { SetThreadPriority(GetCurrentThread(), priority) };
}
#[cfg(not(windows))]
fn set_thread_priority(_: i32) {}
const THREAD_PRIORITY_ABOVE_NORMAL: i32 = 1;
const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;

pub fn spawn(
    data_root: PathBuf,
    cache: PathBuf,
    mesh_budget: usize,
    board_only: bool,
    hall_of_meat: (bool, bool),
    natural_stance: u32,
    log: Log,
) -> (mpsc::Sender<Job>, Arc<Mutex<Shared>>) {
    let shared = Arc::new(Mutex::new(Shared {
        status: STATUS_LOADING,
        message: "loading Skate data".into(),
        snapshot: Snapshot::default(),
        previous: None,
        blend: 1.0,
        steps_done: 0,
        held: Default::default(),
        lifecycle: LifecycleShared::default(),
        hud: Default::default(),
        records: Default::default(),
        record_labels: Default::default(),
        host_impulses: Vec::new(),
    }));
    let (send, receive) = mpsc::channel();
    let thread_shared = Arc::clone(&shared);
    let fail_shared = Arc::clone(&shared);
    let fail_log = Arc::clone(&log);
    let started = std::thread::Builder::new()
        .name("skatev-skate".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            set_thread_priority(THREAD_PRIORITY_ABOVE_NORMAL);
            let result = catch_unwind(AssertUnwindSafe(|| {
                run(
                    data_root,
                    cache,
                    mesh_budget,
                    board_only,
                    hall_of_meat,
                    natural_stance,
                    receive,
                    &thread_shared,
                    &log,
                )
            }));
            let message = match result {
                Ok(Ok(())) => return,
                Ok(Err(e)) => e,
                Err(panic) => format!(
                    "Skate worker panicked: {}",
                    panic
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown")
                ),
            };
            fail_log(&format!("ERROR {message}"));
            if let Ok(mut s) = fail_shared.lock() {
                s.status = STATUS_ERROR;
                s.message = message;
            }
        });
    if let Err(e) = started {
        let mut s = shared.lock().unwrap();
        s.status = STATUS_ERROR;
        s.message = format!("cannot start Skate worker: {e}");
    }
    (send, shared)
}

fn set(shared: &Mutex<Shared>, status: u32, message: impl Into<String>) {
    let mut s = shared.lock().unwrap();
    s.status = status;
    s.message = message.into();
}

type Built = (Vec3, Result<(PreparedCollision, usize, usize), String>);

fn run(
    data_root: PathBuf,
    cache: PathBuf,
    mesh_budget: usize,
    board_only: bool,
    hall_of_meat: (bool, bool),
    natural_stance: u32,
    jobs: mpsc::Receiver<Job>,
    shared: &Mutex<Shared>,
    log: &Log,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    Session::preload(&data_root)?;
    log(&format!(
        "Skate animation banks preloaded in {}ms",
        started.elapsed().as_millis()
    ));
    let static_world = Arc::new(Mutex::new(StaticWorld::open(&cache)));
    let mut prop_templates = crate::dynamic::Templates::new(&cache);
    // Vehicle models whose skitch line was logged (once each).
    let mut skitch_models = std::collections::HashSet::new();
    log(&format!(
        "static map: GTA's loaded collision (live physics level); sidecars beside {}",
        cache.display()
    ));
    log(static_world.lock().unwrap().recovered_status());
    log(static_world.lock().unwrap().ground_status());
    let mut presenter = Presenter::load(&data_root, mesh_budget, board_only, log);
    let mut hom = crate::hom::Driver::load(&data_root, hall_of_meat.0, hall_of_meat.1, log);
    shared.lock().unwrap().record_labels = crate::records::load_labels(&data_root);
    let mut hud = crate::hud::Driver::load(&data_root, hom.available(), shared, log);
    // Skate collision group of each host entity proxy (Hall of Meat hits).
    let mut host_groups: std::collections::HashMap<u32, u32> = Default::default();
    let mut tricks: Vec<String> = Vec::new();
    set(shared, STATUS_READY, "ready");

    let mut session: Option<Session> = None;
    let mut audio_state = crate::audio::map::State::default();
    let mut built_at = Vec3::ZERO;
    let mut builder_jobs: Option<mpsc::Sender<Vec3>> = None;
    let (built_send, built) = mpsc::channel::<Built>();
    // Preload: the one Skate session is built and warmed on a helper thread
    // from now on, while the collision builder already streams the player's
    // area as soon as the host tracks the player; the first board press then
    // finds both. A failed preload leaves the on-demand preparation in place.
    let mut preload: Option<mpsc::Receiver<Result<Session, String>>> = None;
    // A/B diagnostic: SKATEV_PRELOAD=0 restores preparation on first track.
    let preload_on = std::env::var("SKATEV_PRELOAD").map_or(true, |v| v != "0");
    match CollisionBuilder::load(&data_root).and_then(|b| {
        if preload_on { Ok(b) } else { Err("SKATEV_PRELOAD=0".into()) }
    })
        .and_then(|b| start_builder(b, Arc::clone(&static_world), built_send.clone(), log.clone()))
    {
        Ok(sender) => {
            builder_jobs = Some(sender);
            built_at = PRELOAD_AT;
            let (send, receive) = mpsc::channel();
            let (root, preload_log) = (data_root.clone(), Arc::clone(log));
            let spawned = std::thread::Builder::new()
                .name("skatev-preload".into())
                .stack_size(32 * 1024 * 1024)
                .spawn(move || {
                    let result = catch_unwind(AssertUnwindSafe(|| preload_session(&root, natural_stance, &preload_log)))
                        .unwrap_or_else(|_| Err("session preload panicked".into()));
                    let _ = send.send(result);
                });
            match spawned {
                Ok(_) => preload = Some(receive),
                Err(e) => log(&format!("session preload not started: {e}")),
            }
        }
        Err(e) => log(&format!("collision builder not started: {e}; preparing when the player is known")),
    }
    // Last streamed build that failed (open sea, uncached interior): not
    // requested again until the player has moved on.
    let mut stream_failed_at: Option<Vec3> = None;
    // Centre of the collision build in flight off the simulation thread.
    let mut pending_centre: Option<Vec3> = None;
    let mut active = false;
    let mut accumulated = 0.0f32;
    let mut steps_done = 0u64;
    let mut skater_at = Vec3::ZERO;
    // The skater's GTA velocity (zero while inactive) and how long recent
    // collision builds took, for speed-aware streaming.
    let mut skater_velocity = Vec3::ZERO;
    let mut build_secs = 3.0f32;
    let mut requested_at = std::time::Instant::now();
    // Lifecycle: the GTA player's position while Skate is inactive.
    let mut tracked: Option<Vec3> = None;
    // GTA's collision in the area changed: rebuild the working set in place.
    let mut rebuild_area = false;
    // GTA's physics level (live collision): the newest report from the host
    // not yet applied, the level in use, and the fingerprint of the bounds
    // loaded in the installed area (when it changes, GTA streamed collision
    // in or out and the area is rebuilt).
    let mut pending_live: Option<Option<crate::live::Level>> = None;
    let mut live_level: Option<crate::live::Level> = None;
    let mut live_sig: Option<(Vec3, u64)> = None;
    let mut live_checked = std::time::Instant::now();
    let mut live_rebuilt = std::time::Instant::now();
    let mut prepare_failed_at: Option<Vec3> = None;
    let mut monitor = Monitor::new();
    let mut logged_tick = 0u64;
    // Grind entries are logged as they happen (rolling along
    // sidewalk / planter edges caught grinds); the 2 s state line misses them.
    let mut last_state = String::new();
    let mut bail_limit: Option<f32> = None;
    let mut air_limit: Option<f32> = None;
    let mut lip_rule = true;
    let mut difficulty = 0u32;
    let mut camera_type = 1u32;
    let mut last_speed = 0.0f32;
    let mut last_root = bevy_math::Vec3::ZERO;
    let mut perf = Perf::new();
    let mut assist = BackwardsManAssist::new(BackwardsManConfig::default());
    let mut assist_packet = 0u32;
    let mut line_player = crate::line::Player::default();
    let mut latest_dynamic = Vec::new();
    // Skitch grab lines as last sampled by the host, and when. Skate ticks at
    // 60 Hz while host batches arrive at the game's frame rate; each tick
    // carries the lines forward by the car's velocity (`extrapolated_lines`),
    // so a held line moves every tick instead of standing still and jumping.
    let mut skitch_lines: Vec<skate_host::bridge::GrabLine> = Vec::new();
    let mut skitch_lines_at = std::time::Instant::now();
    // The lines Skate is given: `skitch_lines` carried forward, then smoothed
    // (`smoothed_lines`) so sampling-time noise does not shake the held arms.
    let mut skitch_smooth: Vec<skate_host::bridge::GrabLine> = Vec::new();
    // The vehicle being skitched (lifecycle publication).
    let mut skitch_held: Option<u32> = None;
    let mut impulse_log = ImpulseLog::default();
    // Host proxies: vehicles/props (DynamicBodies) and peds (PedParts) are
    // sent separately and solved together.
    let mut entity_proxies: Vec<skate_host::bridge::HostBody> = Vec::new();
    let mut ped_proxies: Vec<skate_host::bridge::HostBody> = Vec::new();

    loop {
        crate::crash::WORKER.leave();
        // Every job taken so far is done (each pass ends here): what the host
        // shows next frame (Shared::at_step).
        {
            let mut s = shared.lock().unwrap();
            if s.steps_done != steps_done {
                s.steps_done = steps_done;
                let held = Held { step: steps_done, snapshot: s.snapshot.clone(), previous: s.previous.clone(), blend: s.blend };
                s.held.push_back(held);
                if s.held.len() > 2 {
                    s.held.pop_front();
                }
            }
        }
        // Coalesce queued frames: every pad packet is collected, time summed.
        // Without jobs the worker still wakes to install a finished collision
        // build (the preloaded session's first one arrives with no host job).
        let mut batch = match jobs.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(first) => vec![first],
            Err(mpsc::RecvTimeoutError::Timeout) => Vec::new(),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        while let Ok(more) = jobs.try_recv() {
            batch.push(more);
        }
        if !batch.is_empty() {
            perf.batch(batch.len());
        }
        crate::crash::WORKER.enter(1);
        // The preloaded session joins once built (an activation waits for it).
        let wait_preload = batch.iter().any(|j| matches!(j, Job::Activate { .. }));
        if let Some(receive) = preload.as_ref() {
            let result = if wait_preload {
                set(shared, STATUS_ACTIVATING, "loading Skate session");
                receive.recv().map_err(|e| e.to_string()).and_then(|r| r)
            } else {
                match receive.try_recv() {
                    Ok(r) => r,
                    Err(mpsc::TryRecvError::Empty) => Err(String::new()),
                    Err(e) => Err(e.to_string()),
                }
            };
            match result {
                Ok(mut s) => {
                    apply_settings(&mut s, hom.enabled(), bail_limit, air_limit, lip_rule, difficulty, camera_type);
                    session = Some(s);
                    life(shared, |l| {
                        l.session_builds += 1;
                        l.preparing = !l.prepared && tracked.is_some();
                    });
                    preload = None;
                }
                Err(e) if e.is_empty() => {}
                Err(e) => {
                    log(&format!("session preload failed: {e}; preparing when the player is known"));
                    preload = None;
                    builder_jobs = None;
                    built_at = Vec3::ZERO;
                    pending_centre = None;
                }
            }
        }
        let mut stepped = false;
        let mut dynamic: Option<Vec<([[f32; 3]; 3], u32, u32)>> = None;
        let mut host_bodies: Option<Vec<skate_host::bridge::HostBody>> = None;
        let mut grab_lines: Option<Vec<skate_host::bridge::GrabLine>> = None;
        let mut track: Option<(Vec3, f32)> = None;
        for job in batch {
            match job {
                Job::Track { at, heading } => track = Some((at, heading)),
                Job::Activate {
                    spawn,
                    heading,
                    aspect,
                    offboard,
                } => {
                    set(shared, STATUS_ACTIVATING, "building Los Santos collision");
                    let t = std::time::Instant::now();
                    let skate_spawn = coords::to_skate(spawn).to_array();
                    let skate_heading = coords::skate_heading(heading);
                    if session.is_none() {
                        // Cold path (no background preparation happened).
                        set(shared, STATUS_ACTIVATING, "loading Skate session");
                        let (mut s, sender) = prepare_session(
                            &data_root,
                            &static_world,
                            &built_send,
                            spawn,
                            heading,
                            natural_stance,
                            shared,
                            log,
                        )?;
                        apply_settings(&mut s, hom.enabled(), bail_limit, air_limit, lip_rule, difficulty, camera_type);
                        builder_jobs = Some(sender);
                        session = Some(s);
                        audio_state = crate::audio::map::State::default();
                        built_at = spawn;
                    } else if world::needs_recentre(built_at, spawn) {
                        let s = session.as_mut().expect("session");
                        // The working set may already be on its way here
                        // (tracked while driving): wait for it instead of
                        // building the same area twice.
                        if pending_centre.is_some_and(|p| !world::needs_recentre(p, spawn)) {
                            set(
                                shared,
                                STATUS_ACTIVATING,
                                "finishing Los Santos collision here",
                            );
                            match built.recv_timeout(std::time::Duration::from_secs(60)) {
                                Ok((centre, Ok((prepared, n, r)))) => {
                                    s.install_collision(prepared)?;
                                    built_at = centre;
                                    life(shared, |l| {
                                        l.collision_builds += 1;
                                        l.collision_centre = centre;
                                        l.prepared = true;
                                        l.preparing = false;
                                    });
                                    log(&format!(
                                        "collision for activation arrived from the stream: {n} triangles, {r} rails, waited {}ms",
                                        t.elapsed().as_millis()
                                    ));
                                }
                                Ok((_, Err(e))) => log(&format!("collision stream failed: {e}")),
                                Err(e) => log(&format!("collision stream wait: {e}")),
                            }
                            pending_centre = None;
                        }
                        if world::needs_recentre(built_at, spawn) {
                            // GTA's collision may not be readable at this
                            // moment: activate on the area already built.
                            match static_world.lock().unwrap().around(spawn) {
                                Ok(area) => {
                                    log_source(log, spawn, &area.note);
                                    let (n, r) = (area.triangles.len(), area.rails.len());
                                    s.install_collision(s.collision_builder().build_tagged(
                                        area.triangles,
                                        area.tags,
                                        area.rails,
                                    )?)?;
                                    built_at = spawn;
                                    life(shared, |l| {
                                        l.collision_builds += 1;
                                        l.collision_centre = spawn;
                                        l.prepared = true;
                                        l.preparing = false;
                                    });
                                    log(&format!(
                                        "collision rebuilt for activation: {n} triangles, {r} rails in {}ms",
                                        t.elapsed().as_millis()
                                    ));
                                }
                                Err(e) => log(&format!("collision for activation unavailable: {e}; activating on the area already built")),
                            }
                        }
                    }
                    let s = session.as_mut().expect("session");
                    s.set_aspect_ratio(aspect);
                    if offboard {
                        s.activate_offboard(skate_spawn, skate_heading)?;
                        monitor.begin_held();
                    } else {
                        s.activate(skate_spawn, skate_heading)?;
                        monitor.begin_onboard();
                    }
                    for line in monitor.log.drain(..) {
                        log(&line);
                    }
                    accumulated = 0.0;
                    skater_at = spawn;
                    active = true;
                    publish(s, s.pose(), shared, 1.0, presenter.as_mut(), None, &mut tricks, log); // entry: no blend from before it
                    hud.activate(s, shared, log);
                    let ms = t.elapsed().as_secs_f32() * 1000.0;
                    let view = s.lifecycle_view();
                    life(shared, |l| {
                        l.last_activate_ms = ms;
                        l.phase = monitor.phase() as u32;
                        l.skate_state = view.state;
                        l.board_possession = view.board_possession;
                    });
                    set(shared, STATUS_ACTIVE, "skating");
                    log(&format!(
                        "activated {} at ({:.2}, {:.2}, {:.2}) heading {heading:.1} in {ms:.0}ms (Skate state {} board {})",
                        if offboard {
                            "off the board"
                        } else {
                            "on the board"
                        },
                        spawn.x,
                        spawn.y,
                        spawn.z,
                        view.state,
                        view.board_possession
                    ));
                }
                Job::DynamicBodies(bodies) => {
                    let (triangles, messages) = prop_templates.triangles(&bodies);
                    for message in messages { log(&message); }
                    dynamic = Some(triangles);
                    entity_proxies = prop_templates.proxies(&bodies);
                    let lines = prop_templates.grab_lines(&bodies);
                    for l in &lines {
                        let Some(b) = bodies.iter().find(|b| (b.fallback.tag | crate::dynamic::DYNAMIC_TAG) == l.tag) else { continue };
                        if skitch_models.insert(b.model) {
                            let under = prop_templates.extent(b.model).map_or(f32::NAN, |(lo, _)| lo.z);
                            log(&format!(
                                "skitch line model {:08x}: {:.2} m above the underside ({}), {:.2} m wide",
                                b.model,
                                l.points[0][2] - under,
                                if b.grab_point.is_some() { "bumper_r" } else { "no bumper_r bone" },
                                l.points[1][0] - l.points[0][0]
                            ));
                        }
                    }
                    grab_lines = Some(lines);
                    host_bodies = Some(entity_proxies.iter().chain(&ped_proxies).cloned().collect());
                }
                Job::PedParts(parts) => {
                    ped_proxies = crate::ped_bodies::bodies(&parts);
                    host_bodies = Some(entity_proxies.iter().chain(&ped_proxies).cloned().collect());
                }
                Job::QuirkConfig(config) => {
                    log(&format!("RetailQuirk::BackwardsMan config {config:?}"));
                    assist.config = config;
                }
                Job::HallOfMeat(on) => hom.set_enabled(session.as_mut(), on, log),
                Job::AirLimit(limit) => {
                    log(&format!("air time limit: {}", bail_limit_text(limit)));
                    air_limit = limit;
                    if let Some(s) = session.as_mut() {
                        s.set_air_time_limit(limit);
                    }
                }
                Job::LipRule(on) => {
                    log(if on {
                        "ramp lips: SkateV rule (stall only riding up slowly; no catch dropping in or carrying speed over)"
                    } else {
                        "ramp lips: retail grind admission"
                    });
                    lip_rule = on;
                    if let Some(s) = session.as_mut() {
                        s.set_lip_rule(on);
                    }
                }
                Job::Difficulty(index) => {
                    const NAMES: [&str; 4] = ["easy", "normal", "hardcore", "motorized"];
                    let Some(name) = NAMES.get(index as usize) else {
                        log(&format!("difficulty: unknown index {index}, unchanged"));
                        continue;
                    };
                    log(&format!("difficulty: {name}"));
                    difficulty = index;
                    if let Some(s) = session.as_mut() {
                        s.set_difficulty(index);
                    }
                }
                Job::CameraType(t) => {
                    log(&format!("camera: {}", if t == 0 { "low" } else { "high" }));
                    camera_type = t;
                    if let Some(s) = session.as_mut() {
                        s.set_camera_type(t);
                    }
                }
                Job::PhysicsLevel(level) => pending_live = Some(level),
                Job::SkitchStandoff(metres) => {
                    log(&format!("skitch standoff: {metres:.2} m behind the rear"));
                    prop_templates.standoff = metres;
                }
                Job::BailLimit(limit) => {
                    log(&format!("bail time limit: {}", bail_limit_text(limit)));
                    bail_limit = limit;
                    if let Some(s) = session.as_mut() {
                        s.set_bail_maximum_time(limit);
                    }
                }
                Job::QuirkTrigger(quirk::RetailQuirk::BackwardsMan) => {
                    if active && !line_player.driving() {
                        assist.request();
                    }
                }
                Job::PlayLine(path) => {
                    let Some(s) = session.as_mut().filter(|_| active) else {
                        log("line: not skating; activate Skate first");
                        continue;
                    };
                    if assist.phase() != quirk::Phase::Idle {
                        log("line: an assist is driving; try again");
                        continue;
                    }
                    let line = match crate::line::Line::load(&path) {
                        Ok(line) => line,
                        Err(e) => {
                            log(&format!("line: {e}"));
                            continue;
                        }
                    };
                    // The line's collision must be the working set before
                    // Skate's teleport puts the skater there.
                    if world::needs_recentre(built_at, line.start) {
                        let t = std::time::Instant::now();
                        let area = static_world.lock().unwrap().around(line.start)?;
                        let (n, r) = (area.triangles.len(), area.rails.len());
                        s.install_collision(s.collision_builder().build_tagged(
                            area.triangles,
                            area.tags,
                            area.rails,
                        )?)?;
                        s.set_dynamic_surfaces(&latest_dynamic);
                        built_at = line.start;
                        life(shared, |l| {
                            l.collision_builds += 1;
                            l.collision_centre = line.start;
                        });
                        log(&format!(
                            "line: collision built at the start: {n} triangles, {r} rails in {}ms",
                            t.elapsed().as_millis()
                        ));
                    }
                    let (at, heading) = (
                        coords::to_skate(line.start).to_array(),
                        coords::skate_heading(line.heading),
                    );
                    // On foot, the line itself mounts with Y (the monitor
                    // follows Skate back to Riding, as for a player mount).
                    if line.offboard {
                        s.activate_offboard(at, heading)?;
                        monitor.begin_held();
                    } else {
                        s.activate(at, heading)?;
                        monitor.begin_onboard();
                    }
                    accumulated = 0.0;
                    skater_at = line.start;
                    log(&format!(
                        "line '{}' playing from ({:.2}, {:.2}, {:.2}) heading {:.1}{}: {} ticks; any controller input stops it",
                        line.name,
                        line.start.x,
                        line.start.y,
                        line.start.z,
                        line.heading,
                        if line.offboard { " on foot" } else { "" },
                        line.max_ticks()
                    ));
                    line_player.start(line);
                }
                Job::Character(request) => {
                    if let Some(pr) = presenter.as_mut() {
                        pr.set_character(request, log);
                    }
                }
                Job::Suspend => {
                    if line_player.stop().is_some() {
                        log("line stopped: Skate deactivated");
                    }
                    if let Some(s) = session.as_mut() {
                        s.suspend_input();
                    }
                    if active {
                        // The player stands where the skater left off.
                        tracked = Some(skater_at);
                        monitor.stop();
                        for line in monitor.log.drain(..) {
                            log(&line);
                        }
                    }
                    active = false;
                    accumulated = 0.0;
                    crate::audio::deactivate();
                    hud.suspend(shared);
                    life(shared, |l| l.phase = Phase::Inactive as u32);
                    set(shared, STATUS_READY, "ready");
                }
                Job::Step { dt, pad, aspect } => {
                    steps_done += 1;
                    let Some(s) = session.as_mut().filter(|_| active) else {
                        continue;
                    };
                    s.set_aspect_ratio(aspect);
                    // A playing line feeds Skate's controller until it ends
                    // or the player takes over.
                    if line_player.driving() {
                        if pad.connected
                            && crate::line::player_input(pad.buttons, pad.triggers, pad.left, pad.right)
                        {
                            line_player.stop();
                            log("line stopped: controller input");
                        } else {
                            accumulated = (accumulated + dt.max(0.0)).min(0.15);
                            stepped = true;
                            continue;
                        }
                    }
                    if pad.connected {
                        assist.check_trigger(pad.buttons);
                    }
                    // While the backwards-man assist drives, it alone feeds
                    // Skate's controller input (one sample per Skate tick).
                    if assist.driving() {
                        accumulated = (accumulated + dt.max(0.0)).min(0.15);
                        stepped = true;
                        continue;
                    }
                    monitor.player_buttons(if pad.connected { pad.buttons } else { 0 });
                    for line in monitor.log.drain(..) {
                        log(&line);
                    }
                    let frame = if pad.connected {
                        InputFrame::from_pad(
                            pad.buttons,
                            pad.triggers,
                            pad.left,
                            pad.right,
                            pad.packet,
                        )
                    } else {
                        InputFrame::neutral()
                    };
                    s.collect(frame, dt);
                    // Same clamp as the mashup host: a hitch never fast-forwards Skate.
                    accumulated = (accumulated + dt.max(0.0)).min(0.15);
                    stepped = true;
                }
            }
        }
        // Lifecycle: background preparation, then the working set follows
        // the GTA player (on foot or driving) while Skate is inactive.
        if let Some((at, heading)) = track.filter(|_| !active) {
            tracked = Some(at);
            // A failed preparation (no cached collision under the player:
            // open sea, an uncached interior) is retried once the player has
            // moved on; it never takes the worker down.
            let retry = prepare_failed_at.is_none_or(|f| world::needs_recentre(f, at));
            if built_at == PRELOAD_AT {
                life(shared, |l| l.preparing = !l.prepared);
            }
            if session.is_none() && preload.is_none() && builder_jobs.is_none() && retry {
                match prepare_session(
                    &data_root,
                    &static_world,
                    &built_send,
                    at,
                    heading,
                    natural_stance,
                    shared,
                    log,
                ) {
                    Ok((mut s, sender)) => {
                        apply_settings(&mut s, hom.enabled(), bail_limit, air_limit, lip_rule, difficulty, camera_type);
                        builder_jobs = Some(sender);
                        session = Some(s);
                        audio_state = crate::audio::map::State::default();
                        built_at = at;
                        prepare_failed_at = None;
                    }
                    Err(e) => {
                        log(&format!(
                            "background preparation at ({:.0}, {:.0}) failed: {e}; retrying elsewhere",
                            at.x, at.y
                        ));
                        life(shared, |l| l.preparing = false);
                        prepare_failed_at = Some(at);
                    }
                }
            }
        }
        if let (Some(s), Some(triangles)) = (session.as_mut(), dynamic) {
            latest_dynamic = triangles;
            s.set_dynamic_surfaces(&latest_dynamic);
        }
        if let (Some(s), Some(bodies)) = (session.as_mut(), host_bodies) {
            host_groups = bodies.iter().map(|b| (b.tag, b.collision_group)).collect();
            s.set_host_bodies(bodies);
        }
        if let (Some(s), Some(lines)) = (session.as_mut(), grab_lines) {
            if let Err(e) = s.set_grab_lines(&lines) {
                log(&format!("skitch grab lines rejected: {e}"));
            }
            skitch_lines = lines;
            skitch_lines_at = std::time::Instant::now();
        }
        let target = if active { Some(skater_at) } else { tracked };
        // Finished builds wait in the channel until the session exists.
        if let Some(s) = session.as_mut() {
            if let Ok((centre, result)) = built.try_recv() {
                if pending_centre.take().is_some() {
                    let took = requested_at.elapsed().as_secs_f32().clamp(0.2, 15.0);
                    build_secs = build_secs * 0.6 + took * 0.4;
                }
                match result {
                    // A build the player has already left (driving on, or
                    // an activation elsewhere) is dropped; the next request
                    // replaces it. Within three quarters of its half-size it
                    // still covers the player and is installed (fast riding).
                    Ok(_)
                        if target.is_some_and(|t| !world::worth_installing(centre, built_at, t)) =>
                    {
                        log(&format!(
                            "collision around ({:.0}, {:.0}) superseded before install",
                            centre.x, centre.y
                        ))
                    }
                    Ok((prepared, n, r)) => {
                        let t = std::time::Instant::now();
                        s.install_collision(prepared)?;
                        s.set_dynamic_surfaces(&latest_dynamic);
                        perf.install(t.elapsed());
                        let first = built_at == PRELOAD_AT;
                        built_at = centre;
                        stream_failed_at = None;
                        life(shared, |l| {
                            l.collision_builds += 1;
                            l.collision_centre = centre;
                            if first {
                                l.prepared = true;
                                l.preparing = false;
                            }
                        });
                        if first {
                            log(&format!(
                                "Skate prepared: the preloaded session has the player's collision ({:.0}s after the runtime started)",
                                started.elapsed().as_secs_f32()
                            ));
                        }
                        if verbose() {
                            log(&format!(
                                "collision streamed around ({:.0}, {:.0}){}: {n} triangles, {r} rails",
                                centre.x,
                                centre.y,
                                if active { "" } else { " while inactive" }
                            ));
                        }
                    }
                    Err(e) => {
                        // Once until a stream succeeds (it retries while GTA's collision loads).
                        if stream_failed_at.is_none() {
                            log(&format!("collision stream failed: {e}"));
                        }
                        stream_failed_at = Some(centre);
                    }
                }
            }
        }
        if let Some(level) = pending_live {
            match static_world.try_lock() {
                Ok(mut w) => {
                    w.set_live(level);
                    pending_live = None;
                    live_level = level;
                    live_sig = None;
                    rebuild_area = true;
                    prepare_failed_at = None;
                    stream_failed_at = None;
                    log(&match level {
                        Some(l) => format!("live collision: GTA's physics level at {:#x}; the static map is read from it", l.base),
                        None => "live collision: physics level lost; no new areas until it is found again".to_string(),
                    });
                }
                Err(std::sync::TryLockError::WouldBlock) => {}
                Err(std::sync::TryLockError::Poisoned(_)) => return Err("world cache poisoned".into()),
            }
        }
        // GTA streams collision in and out: a changed set of static bounds in
        // the installed area asks for a rebuild (checked twice a second at
        // most, rebuilt at most every 6 s).
        if let Some(level) = live_level
            && pending_centre.is_none()
            && built_at != PRELOAD_AT
            && live_checked.elapsed().as_secs_f32() >= 0.5
        {
            live_checked = std::time::Instant::now();
            if let Ok(sig) = level.signature([built_at.x, built_at.y], world::RADIUS) {
                match live_sig {
                    Some((at, was)) if at == built_at => {
                        if was != sig && live_rebuilt.elapsed().as_secs_f32() >= 6.0 {
                            rebuild_area = true;
                            live_rebuilt = std::time::Instant::now();
                            live_sig = None;
                            if verbose() {
                                log("live collision: GTA loaded or dropped collision in the area, rebuilding");
                            }
                        }
                    }
                    _ => live_sig = Some((built_at, sig)),
                }
            }
        }
        // Requests go out before the preloaded session exists, too.
        // Collision GTA streamed in or out rebuilds the working set where the
        // player is. A moving skater's next area is requested early and centred ahead
        // (world::needs_recentre_moving, lead_centre).
        let velocity = if active { skater_velocity } else { Vec3::ZERO };
        if let Some(t) = target
            && pending_centre.is_none()
            && (world::needs_recentre_moving(built_at, t, velocity, build_secs) || rebuild_area)
            && stream_failed_at.is_none_or(|f| world::needs_recentre(f, t) || rebuild_area)
        {
            let centre = world::lead_centre(t, velocity, build_secs);
            if builder_jobs.as_ref().is_some_and(|b| b.send(centre).is_ok()) {
                pending_centre = Some(centre);
                requested_at = std::time::Instant::now();
                rebuild_area = false;
            }
        }
        if builder_jobs.is_some() {
            let building = pending_centre.is_some();
            life(shared, |l| l.building = building);
        }
        let Some(s) = session.as_mut().filter(|_| active && stepped) else {
            continue;
        };

        let period = s.period();
        // The pose after the last tick advanced (published below).
        let mut advanced: Option<Pose> = None;
        while accumulated >= period {
            accumulated -= period;
            if monitor.phase() != Phase::Inactive {
                let v = s.lifecycle_view();
                monitor.step(lifecycle::View {
                    state: v.state,
                    board_possession: v.board_possession,
                });
                for line in monitor.log.drain(..) {
                    log(&line);
                }
            }
            // Skate's pose before this tick, for the line player and the assist.
            let before = (line_player.driving() || assist.phase() != quirk::Phase::Idle).then(|| s.pose());
            if let Some(pose) = before.as_ref().filter(|_| line_player.driving()) {
                let next = line_player.next(&crate::line::Obs::from_pose(pose));
                for line in line_player.log.drain(..) {
                    log(&line);
                }
                match next {
                    crate::line::Next::Frame(f) => {
                        assist_packet = assist_packet.wrapping_add(1);
                        s.collect(
                            InputFrame::from_pad(f.buttons, f.triggers, f.left, f.right, assist_packet),
                            period,
                        );
                    }
                    crate::line::Next::Done => {
                        line_player.stop();
                        log("line finished; controller back to the player");
                    }
                    crate::line::Next::Missed(why) => {
                        line_player.stop();
                        log(&format!("line missed: {why}; controller back to the player"));
                    }
                }
            }
            if let Some(pose) = before.as_ref().filter(|_| assist.phase() != quirk::Phase::Idle) {
                let obs = quirk::Observation {
                    state: &pose.state,
                    position: pose.root.w_axis.truncate(),
                    velocity: pose.velocity,
                };
                let (injected, event) = assist.step(obs);
                if event == quirk::Event::Disarm {
                    s.arm_offboard_launch(None);
                }
                if let Some(i) = injected {
                    assist_packet = assist_packet.wrapping_add(1);
                    s.collect(
                        InputFrame::from_pad(
                            i.buttons,
                            [i.left_trigger, i.right_trigger],
                            i.left_stick,
                            i.right_stick,
                            assist_packet,
                        ),
                        period,
                    );
                }
                for line in assist.log.drain(..) {
                    log(&line);
                }
            }
            let t = std::time::Instant::now();
            let air_frames = s.air_frames();
            if !skitch_lines.is_empty() {
                let ahead = skitch_lines_at.elapsed().as_secs_f32();
                // A line set the host has not refreshed for a while is left as is.
                if ahead > 0.0 && ahead < 0.25 {
                    let lines = smoothed_lines(&mut skitch_smooth, &extrapolated_lines(&skitch_lines, ahead), period);
                    let _ = s.set_grab_lines(&lines);
                }
            }
            s.advance()?;
            perf.tick(t.elapsed());
            skitch_held = s.skitch_held_tag();
            if s.air_teleport_requested() {
                log(&format!(
                    "air reset: airborne {:.2}s ({} frames), Skate returns the skater to the last checkpoint (limit {})",
                    (air_frames + 1) as f32 * period,
                    air_frames + 1,
                    bail_limit_text(air_limit)
                ));
            }
            let pose = s.pose();
            let struck: Vec<(u32, u32)> = exchange_impulses(s, &pose, shared, log, &ped_proxies, &mut impulse_log)
                .into_iter()
                .filter_map(|tag| host_groups.get(&tag).map(|g| (tag, *g)))
                .collect();
            if hom.tick(s, &struck, log).is_some_and(|o| o.ended) {
                let total = hom.bail_total();
                let now = crate::records::now();
                shared.lock().unwrap().records.add(crate::records::Category::HallOfMeat, total, now, log);
            }
            hud.tick(s, &hom, shared, log);
            crate::audio::on_tick(s, &pose, &mut audio_state);
            advanced = Some(pose);
        }
        if advanced.is_none() {
            shared.lock().unwrap().blend = accumulated / period;
        }
        if let Some(pose) = advanced {
            let t = std::time::Instant::now();
            let blend = accumulated / period;
            let snap = publish(s, pose, shared, blend, presenter.as_mut(), Some(&mut hom), &mut tricks, log);
            let now = crate::records::now();
            shared.lock().unwrap().records.observe_lines(snap.score.completed_lines, now, log);
            hud.publish(shared, log);
            shared.lock().unwrap().snapshot.assist_phase = assist.phase() as u32;
            let view = s.lifecycle_view();
            life(shared, |l| {
                l.phase = monitor.phase() as u32;
                l.skate_state = view.state;
                l.board_possession = view.board_possession;
                l.skitch_vehicle = skitch_held.map_or(0, |t| t & 0x0FFF_FFFF);
            });
            if let Some(l) = s
                .quirk_state()
                .last_launch
                .filter(|l| l.tick == snap.tick && l.com_branch)
            {
                log(&format!(
                    "off-board launch tick {}: start {:?} com {:?} overridden={}",
                    l.tick, l.start_velocity, l.com_velocity, l.overridden
                ));
            }
            perf.publish(t.elapsed());
            if let Some(line) = perf.report(period)
                && verbose()
            {
                log(&line);
            }
            skater_at = snap.root;
            skater_velocity = snap.velocity;
            if snap.state != last_state {
                if snap.state.starts_with("Grind") && !last_state.starts_with("Grind") {
                    log(&format!(
                        "grind entry tick={} pos=({:.2}, {:.2}, {:.2}) speed={:.2} from={} to={} trick='{}'",
                        snap.tick,
                        snap.root.x,
                        snap.root.y,
                        snap.root.z,
                        snap.velocity.length(),
                        last_state,
                        snap.state,
                        snap.score.trick_name
                    ));
                }
                if snap.state.starts_with("Wipeout") && !last_state.starts_with("Wipeout") {
                    log(&format!(
                        "bail tick={} pos=({:.2}, {:.2}, {:.2}) speed {:.2} -> {:.2} from={} to={}",
                        snap.tick,
                        snap.root.x,
                        snap.root.y,
                        snap.root.z,
                        last_speed,
                        snap.velocity.length(),
                        last_state,
                        snap.state
                    ));
                }
                if last_state.starts_with("Wipeout") && !snap.state.starts_with("Wipeout") {
                    let r = s.bail_recovery();
                    log(&format!(
                        "bail ended tick={} after {:.2}s ({}) to={} settled {:.2}s since-response {:.2}s limit {}",
                        snap.tick,
                        r.time,
                        bail_end_reason(&r),
                        snap.state,
                        r.settled_time,
                        r.response_time,
                        bail_limit_text(Some(r.maximum_time))
                    ));
                }
                last_state.clone_from(&snap.state);
            }
            // A sudden stop while rolling (board caught on an edge): over 3 m/s
            // lost in one published frame, on the ground.
            let speed = snap.velocity.length();
            if last_speed - speed > 3.0
                && snap.state.contains("Ground")
                && !snap.state.starts_with("Wipeout")
            {
                log(&format!(
                    "stop tick={} pos=({:.2}, {:.2}, {:.2}) prev=({:.2}, {:.2}, {:.2}) speed {:.2} -> {:.2} state={}",
                    snap.tick,
                    snap.root.x,
                    snap.root.y,
                    snap.root.z,
                    last_root.x,
                    last_root.y,
                    last_root.z,
                    last_speed,
                    speed,
                    snap.state
                ));
            }
            last_speed = speed;
            last_root = snap.root;
            if snap.tick / 120 != logged_tick / 120 && verbose() {
                logged_tick = snap.tick;
                log(&format!(
                    "tick={} pos=({:.2}, {:.2}, {:.2}) speed={:.2} state={} score={:.0} line={:.0} x{:.2} trick='{}'",
                    snap.tick,
                    snap.root.x,
                    snap.root.y,
                    snap.root.z,
                    snap.velocity.length(),
                    snap.state,
                    snap.score.sequence_score,
                    snap.score.line_score,
                    snap.score.multiplier,
                    snap.score.trick_name
                ));
            }
        }
    }
    Ok(())
}

/// Contact exchange bookkeeping kept across ticks.
#[derive(Default)]
struct ImpulseLog {
    /// Host impacts logged recently: (entity, when).
    impacts: Vec<(u32, std::time::Instant)>,
    /// The vehicle let go of, and when (its pushes stay withheld 0.5 s).
    released: Option<(u32, std::time::Instant)>,
    /// The vehicle held and how many pushes on it were withheld.
    hold: (u32, usize),
}

/// Contact exchange: publish this tick's impulses on host entities (GTA
/// space, entity tag without the dynamic bit) for the host to apply once.
/// Returns the entity tags (as the session knows them) pushed this tick.
fn exchange_impulses(
    s: &mut Session,
    pose: &Pose,
    shared: &Mutex<Shared>,
    log: &Log,
    peds: &[skate_host::bridge::HostBody],
    state: &mut ImpulseLog,
) -> Vec<u32> {
    // Host-driven impacts (a car striking the rider), logged once per entity
    // per quarter second.
    {
        let last = &mut state.impacts;
        for i in s.take_host_impacts() {
            let now = std::time::Instant::now();
            let tag = i.tag & !crate::dynamic::DYNAMIC_TAG;
            if last.iter().any(|(t, at)| *t == tag && now.duration_since(*at).as_millis() < 250) {
                continue;
            }
            last.retain(|(t, at)| *t != tag && now.duration_since(*at).as_secs() < 5);
            last.push((tag, now));
            let j = (i.impulse[0].powi(2) + i.impulse[1].powi(2) + i.impulse[2].powi(2)).sqrt();
            log(&format!(
                "host impact tick={} entity {tag:08x}: closing {:.1} m/s, impulse {j:.0} N s on the {:.0} kg board+rider, {} skeleton parts struck, state={}",
                pose.tick, i.closing, i.assembly_mass, i.struck_parts, pose.state
            ));
        }
    }
    // The vehicle being skitched is GTA's to drive: the rider's contact
    // pushes on it are withheld, as Skate's own tow reaction already is
    // (otherwise the rider pushes the car, which then tows the rider).
    let held = s.skitch_held_tag();
    let mut impulses = s.take_host_impulses();
    let before = impulses.len();
    // ... and for 0.5 s after the release (the exit tick and the separation
    // still produce pushes on the car the rider just let go of).
    if state.released.is_some_and(|(_, at)| at.elapsed().as_secs_f32() > 0.5) {
        state.released = None;
    }
    let released = state.released.map(|(tag, _)| tag);
    impulses.retain(|i| Some(i.tag) != held && Some(i.tag) != released);
    {
        let hold = &mut state.hold;
        let tag = held.unwrap_or(0);
        if tag != hold.0 {
            if hold.0 != 0 {
                state.released = Some((hold.0, std::time::Instant::now()));
                log(&format!(
                    "skitch released vehicle {:08x}: {} contact pushes on it withheld",
                    hold.0 & !crate::dynamic::DYNAMIC_TAG,
                    hold.1
                ));
            }
            *hold = (tag, 0);
        }
        hold.1 += before - impulses.len();
    }
    if impulses.is_empty() {
        return Vec::new();
    }
    let mut struck: Vec<u32> = impulses.iter().map(|i| i.tag).collect();
    struck.sort_unstable();
    struck.dedup();
    let tick = pose.tick;
    let masses = s.host_body_masses();
    let mut shared = shared.lock().unwrap();
    for i in impulses {
        // Unbounded growth is impossible: the host drains every frame, and a
        // host that stops draining loses the oldest impulses, never the newest.
        if shared.host_impulses.len() >= 1024 {
            shared.host_impulses.remove(0);
        }
        let v = |a: [f32; 3]| {
            let g = coords::from_skate(Vec3::from_array(a));
            crate::SvVec3 { x: g.x, y: g.y, z: g.z }
        };
        let inv = masses.get(&i.tag).copied().unwrap_or(0.0);
        // A ped struck off its centre of mass turns as well (its posed body).
        let spin = peds.iter().find(|b| b.tag == i.tag)
            .map_or([0.0; 3], |b| crate::ped_bodies::angular_change(b, i.point, i.impulse).to_array());
        shared.host_impulses.push(crate::SvDynamicImpulse {
            size: std::mem::size_of::<crate::SvDynamicImpulse>() as u32,
            tag: i.tag & !crate::dynamic::DYNAMIC_TAG,
            tick,
            point: v(i.point),
            impulse: v(i.impulse),
            mass_kg: if inv > 0.0 { 1.0 / inv } else { 0.0 },
            reserved: 0,
            angular_velocity_change: v(spin),
            reserved2: 0,
        });
    }
    struck
}

/// Why Skate's WipeoutGround asked for the reset (its recovery state at the
/// end of the wipeout; recovery.rs `should_teleport`, in that order).
fn bail_end_reason(r: &skate_host::bridge::bail::BailRecovery) -> &'static str {
    if !r.countdown {
        "left the wipeout without a reset"
    } else if r.impaled {
        "stuck in geometry (impaled)"
    } else if r.recover_pressed && r.recovery_eligible {
        "A/X recover pressed"
    } else if r.automatic_pending && r.time > r.maximum_time {
        "time limit"
    } else if r.automatic_pending {
        "body settled"
    } else {
        "reset requested"
    }
}

fn bail_limit_text(limit: Option<f32>) -> String {
    match limit {
        None => "retail".into(),
        Some(t) if t.is_infinite() => "none".into(),
        Some(t) => format!("{t:.1}s"),
    }
}

fn life(shared: &Mutex<Shared>, f: impl FnOnce(&mut LifecycleShared)) {
    f(&mut shared.lock().unwrap().lifecycle);
}

/// The host settings a new session starts with; later changes are applied
/// by their jobs (plain session fields, kept across activations).
fn apply_settings(s: &mut Session, hall_of_meat: bool, bail: Option<f32>, air: Option<f32>, lip: bool, difficulty: u32,
                  camera_type: u32) {
    s.set_hall_of_meat(hall_of_meat);
    s.set_difficulty(difficulty);
    s.set_camera_type(camera_type);
    s.set_bail_maximum_time(bail);
    s.set_air_time_limit(air);
    s.set_lip_rule(lip);
}

/// Builds the one Skate session of this process around GTA point `at`
/// (collision, session, streaming builder), then warms it with one off-board
/// entry so the first real activation runs no first-use work. Runs on the
/// Skate worker, never on GTA's thread; the status stays READY meanwhile.
fn prepare_session(
    data_root: &std::path::Path,
    static_world: &Arc<Mutex<StaticWorld>>,
    built_send: &mpsc::Sender<Built>,
    at: Vec3,
    heading: f32,
    natural_stance: u32,
    shared: &Mutex<Shared>,
    log: &Log,
) -> Result<(Session, mpsc::Sender<Vec3>), String> {
    let t = std::time::Instant::now();
    life(shared, |l| l.preparing = true);
    let area = static_world.lock().unwrap().around(at)?;
    log_source(log, at, &area.note);
    log(&format!(
        "collision around ({:.1}, {:.1}, {:.1}): {} triangles, rails: {} candidates {} lips {} runs {} rails; curb bevels {} on {} top edges",
        at.x,
        at.y,
        at.z,
        area.triangles.len(),
        area.census.candidates,
        area.census.lips,
        area.census.runs,
        area.census.rails,
        area.bevels.bevels,
        area.bevels.edges
    ));
    if area.triangles.is_empty() {
        life(shared, |l| l.preparing = false);
        return Err(format!(
            "no GTA collision around ({:.0}, {:.0})",
            at.x, at.y
        ));
    }
    let skate_at = coords::to_skate(at).to_array();
    let skate_heading = coords::skate_heading(heading);
    let mut s = Session::new_tagged(
        data_root,
        area.triangles,
        area.tags,
        area.rails,
        skate_at,
        skate_heading,
    )?;
    s.set_natural_stance(natural_stance);
    let sender = start_builder(
        s.collision_builder(),
        Arc::clone(static_world),
        built_send.clone(),
        log.clone(),
    )?;
    let built_ms = t.elapsed().as_millis();
    // Warm-up: Skate's off-board entry and a few ticks at the preparation
    // point, then suspended. Disable with SKATEV_PREPARE_WARMUP=0.
    let warm = std::env::var("SKATEV_PREPARE_WARMUP").map_or(true, |v| v != "0");
    if warm {
        s.activate_offboard(skate_at, skate_heading)?;
        for _ in 0..30 {
            s.collect(InputFrame::neutral(), s.period());
            s.advance()?;
        }
    }
    s.suspend_input();
    let ms = t.elapsed().as_secs_f32() * 1000.0;
    life(shared, |l| {
        l.preparing = false;
        l.prepared = true;
        l.session_builds += 1;
        l.collision_builds += 1;
        l.collision_centre = at;
        l.last_prepare_ms = ms;
    });
    log(&format!(
        "Skate session prepared around ({:.1}, {:.1}, {:.1}) in {ms:.0}ms (session {built_ms}ms, warm-up {})",
        at.x,
        at.y,
        at.z,
        if warm { "on" } else { "off" }
    ));
    Ok((s, sender))
}

/// Collision centre of a preloaded session that has no player collision yet:
/// far enough from any GTA point that the first tracked position streams.
const PRELOAD_AT: Vec3 = Vec3::new(1.0e7, 1.0e7, 0.0);

/// Builds the one Skate session before the player is known, on a helper
/// thread (see `run`): its data, physics, skater and camera do not depend on
/// the player's area. The
/// world it starts with is a 4 m floor at GTA's origin, only so the session
/// can be built and warmed (off-board entry and 30 ticks, as
/// `prepare_session`); it is never used for riding: `prepared` stays false
/// until the player's collision is installed.
fn preload_session(data_root: &std::path::Path, natural_stance: u32, log: &Log) -> Result<Session, String> {
    let t = std::time::Instant::now();
    let at = coords::to_skate(Vec3::ZERO);
    let h = 2.0;
    let corner = |x: f32, z: f32| [at.x + x, at.y, at.z + z];
    // Counterclockwise seen from above (Skate space, Y up).
    let floor = vec![
        [corner(-h, -h), corner(-h, h), corner(h, h)],
        [corner(-h, -h), corner(h, h), corner(h, -h)],
    ];
    let mut s = Session::new_tagged(data_root, floor, vec![0; 2], Vec::new(), at.to_array(), 0.0)?;
    s.set_natural_stance(natural_stance);
    s.activate_offboard(at.to_array(), 0.0)?;
    for _ in 0..30 {
        s.collect(InputFrame::neutral(), s.period());
        s.advance()?;
    }
    s.suspend_input();
    log(&format!("Skate session preloaded in {}ms", t.elapsed().as_millis()));
    Ok(s)
}

/// Worker timing over a 2 s window: Skate tick cost against its fixed period.
struct Perf {
    window: std::time::Instant,
    ticks: u32,
    tick_total: f64,
    tick_max: f64,
    publish_max: f64,
    batches: u32,
    batch_max: usize,
    install_max: f64,
}

impl Perf {
    fn new() -> Self {
        Self {
            window: std::time::Instant::now(),
            ticks: 0,
            tick_total: 0.0,
            tick_max: 0.0,
            publish_max: 0.0,
            batches: 0,
            batch_max: 0,
            install_max: 0.0,
        }
    }
    fn tick(&mut self, d: std::time::Duration) {
        let ms = d.as_secs_f64() * 1000.0;
        self.ticks += 1;
        self.tick_total += ms;
        self.tick_max = self.tick_max.max(ms);
    }
    fn publish(&mut self, d: std::time::Duration) {
        self.publish_max = self.publish_max.max(d.as_secs_f64() * 1000.0);
    }
    fn batch(&mut self, n: usize) {
        self.batches += 1;
        self.batch_max = self.batch_max.max(n);
    }
    fn install(&mut self, d: std::time::Duration) {
        self.install_max = self.install_max.max(d.as_secs_f64() * 1000.0);
    }
    fn report(&mut self, period: f32) -> Option<String> {
        let elapsed = self.window.elapsed().as_secs_f64();
        if elapsed < 2.0 {
            return None;
        }
        let line = format!(
            "perf {:.1}s: {} ticks ({:.1}/s, period {:.2}ms) tick avg {:.2}ms max {:.2}ms, publish max {:.2}ms, {} batches max {} frames, install max {:.1}ms",
            elapsed,
            self.ticks,
            self.ticks as f64 / elapsed,
            period * 1000.0,
            self.tick_total / self.ticks.max(1) as f64,
            self.tick_max,
            self.publish_max,
            self.batches,
            self.batch_max,
            self.install_max
        );
        *self = Self::new();
        Some(line)
    }
}

/// Where an area's triangles came from (the live physics level census);
/// nothing when the area has no note.
fn log_source(log: &Log, centre: Vec3, note: &str) {
    if !note.is_empty() && verbose() {
        log(&format!("collision source around ({:.0}, {:.0}): {note}", centre.x, centre.y));
    }
}

fn start_builder(
    builder: CollisionBuilder,
    static_world: Arc<Mutex<StaticWorld>>,
    out: mpsc::Sender<Built>,
    log: Log,
) -> Result<mpsc::Sender<Vec3>, String> {
    let (send, jobs) = mpsc::channel::<Vec3>();
    std::thread::Builder::new()
        .name("skatev-collision".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            set_thread_priority(THREAD_PRIORITY_BELOW_NORMAL);
            while let Ok(mut centre) = jobs.recv() {
                while let Ok(newer) = jobs.try_recv() {
                    centre = newer;
                }
                let result = catch_unwind(AssertUnwindSafe(|| -> Result<_, String> {
                    let area = static_world
                        .lock()
                        .map_err(|_| "world cache poisoned")?
                        .around(centre)?;
                    log_source(&log, centre, &area.note);
                    let (n, r) = (area.triangles.len(), area.rails.len());
                    Ok((
                        builder.build_tagged(area.triangles, area.tags, area.rails)?,
                        n,
                        r,
                    ))
                }))
                .unwrap_or_else(|_| Err("collision builder panicked".into()));
                if out.send((centre, result)).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(send)
}

/// DRAW_POLY neither depth-tests script polygons against each other nor
/// culls in a convention we can rely on, so the runtime does it: both mesh
/// sources are counterclockwise-outward (Skate GLB by glTF convention; GTA ped
/// components measured: positive signed volume on every component). Faces
/// turned away from the camera are dropped, the rest are ordered far to near
/// so nearer faces (an arm in front of the torso) draw last.
fn present_for_camera(mesh: &mut Vec<skin::ShadedTri>, camera: Vec3) {
    mesh.retain(|t| {
        let [a, b, c] = t.points;
        (b - a).cross(c - a).dot(camera - (a + b + c) / 3.0) > 0.0
    });
    let depth = |t: &skin::ShadedTri| {
        ((t.points[0] + t.points[1] + t.points[2]) / 3.0).distance_squared(camera)
    };
    mesh.sort_by(|x, y| depth(y).total_cmp(&depth(x)));
}

/// What the worker reads back from a publication.
struct Published {
    tick: u64,
    root: Vec3,
    velocity: Vec3,
    state: String,
    score: skate_host::ScoreView,
}

/// Publishes Skate's state after a tick; `p` is the session's current pose.
fn publish(
    s: &Session,
    p: Pose,
    shared: &Mutex<Shared>,
    blend: f32,
    presenter: Option<&mut Presenter>,
    hom: Option<&mut crate::hom::Driver>,
    tricks: &mut Vec<String>,
    log: &Log,
) -> Published {
    let board_pose = crate::board_pose::world(&p.names, &p.bones, p.root);
    let mut presenter = presenter;
    let (mut mesh, textures, character_pose, board_entity) = presenter
        .as_deref_mut()
        .map(|pr| {
            let mesh = pr.draw(&p, log);
            let entity = board_pose.and_then(|world| {
                let inverse = pr.mesh.inverse_bind_for("SKATEBOARD_ROOT")?;
                let c = Mat4::from_mat3(coords::basis());
                Some(world[0] * c * inverse * c.transpose())
            });
            (mesh, pr.textures.clone(), pr.pose.clone(), entity)
        })
        .unwrap_or_default();
    // The x-ray rides the posed GTA ped when one is presented.
    let overlay = presenter.as_deref().and_then(|pr| {
        let ped = pr.character.as_ref()?;
        (pr.board_only && pr.pose.len() == ped.bone_names().len())
            .then(|| crate::xray::Overlay { ped, world: &pr.pose })
    });
    let xray = match (hom, p.camera) {
        (Some(h), Some((eye, _, _))) => h.xray(&p.names, &p.bones, p.root, eye, overlay.as_ref()),
        _ => Vec::new(),
    };
    if let Some((pos, _, _)) = p.camera {
        present_for_camera(&mut mesh, coords::from_skate(pos));
    }
    let score = s.score();
    if !score.trick_name.is_empty() && tricks.last() != Some(&score.trick_name) {
        tricks.push(score.trick_name.clone());
        if tricks.len() > 6 {
            tricks.remove(0);
        }
    }
    let hits = s
        .dynamic_hits()
        .into_iter()
        .map(|(tag, point, normal)| {
            (
                tag,
                coords::from_skate(Vec3::from_array(point)),
                coords::basis() * Vec3::from_array(normal),
            )
        })
        .collect();
    let (root, root_rotation) = coords::transform_from_skate(p.root);
    let forward = root_rotation * (coords::basis() * Vec3::Z);
    let (deck, deck_q) = s.deck();
    let snap = Snapshot {
        tick: p.tick,
        root,
        root_rotation,
        heading: coords::gta_heading(forward),
        deck: coords::from_skate(Vec3::from_array(deck)),
        deck_rotation: coords::rotation_from_skate(Quat::from_array(deck_q)),
        velocity: coords::basis() * p.velocity,
        camera: p.camera.map(|(pos, basis, fov)| {
            let b = coords::basis();
            (coords::from_skate(pos), b * basis.z_axis, fov)
        }),
        state: p.state,
        score,
        mesh: Arc::new(mesh),
        textures,
        character_pose,
        board_pose,
        board_entity,
        hits,
        tricks: tricks.clone(),
        assist_phase: 0,
        xray: Arc::new(xray),
        quirk: {
            let q = s.quirk_state();
            let g = |v: [f32; 3]| coords::basis() * Vec3::from_array(v);
            let l = q.last_launch.unwrap_or_default();
            Quirk {
                board_state: q.board_state,
                deck_velocity: g(q.deck_velocity),
                com_trajectory_velocity: g(q.com_trajectory_velocity),
                use_com_velocity: q.use_com_velocity,
                launch_tick: l.tick,
                launch_start_velocity: g(l.start_velocity),
                launch_com_velocity: g(l.com_velocity),
                launch_com_branch: l.com_branch,
                launch_overridden: l.overridden,
            }
        },
    };
    let published = Published {
        tick: snap.tick,
        root: snap.root,
        velocity: snap.velocity,
        state: snap.state.clone(),
        score: snap.score.clone(),
    };
    if snap.root.is_finite() {
        let mut shared = shared.lock().unwrap();
        shared.previous = Some(std::mem::replace(&mut shared.snapshot, snap));
        shared.blend = blend; // with the snapshot, so no frame pairs the new tick with the old blend
    }
    published
}

#[cfg(test)]
mod bail_tests {
    use super::*;
    use skate_host::bridge::bail::BailRecovery;

    fn ended() -> BailRecovery {
        BailRecovery { countdown: true, maximum_time: 10.0, ..Default::default() }
    }

    #[test]
    fn bail_end_reasons_follow_the_recovery_order() {
        let mut r = ended();
        r.recover_pressed = true;
        r.recovery_eligible = true;
        assert_eq!(bail_end_reason(&r), "A/X recover pressed");
        r.impaled = true;
        assert_eq!(bail_end_reason(&r), "stuck in geometry (impaled)");
        let mut r = ended();
        r.automatic_pending = true;
        r.time = 4.0;
        assert_eq!(bail_end_reason(&r), "body settled");
        r.time = 10.5;
        assert_eq!(bail_end_reason(&r), "time limit");
        r.countdown = false;
        assert_eq!(bail_end_reason(&r), "left the wipeout without a reset");
        // A press before eligibility is not the reason.
        let mut r = ended();
        r.recover_pressed = true;
        assert_eq!(bail_end_reason(&r), "reset requested");
    }

    #[test]
    fn bail_limit_text_names_retail_none_and_seconds() {
        assert_eq!(bail_limit_text(None), "retail");
        assert_eq!(bail_limit_text(Some(f32::INFINITY)), "none");
        assert_eq!(bail_limit_text(Some(45.0)), "45.0s");
    }
}

/// Grab lines moved `seconds` ahead along each car's sampled linear velocity
/// (frame row 3 is the position, `velocity` the same Skate-space units per s).
fn extrapolated_lines(lines: &[skate_host::bridge::GrabLine], seconds: f32) -> Vec<skate_host::bridge::GrabLine> {
    lines
        .iter()
        .map(|l| {
            let mut l = *l;
            for i in 0..3 {
                l.frame[3][i] += l.velocity[i] * seconds;
            }
            l
        })
        .collect()
}

/// Tick-to-tick correction of the carried line toward its latest estimate.
/// The estimate is the host's last sample plus velocity times the time since
/// it arrived; that time carries a few ms of jitter (4 cm at 10 m/s)
/// which the held arms' IK followed. The filter keeps the line
/// moving by its own smoothed velocity and only absorbs the noise.
/// ponytail: fixed gains; a lag of a few cm while the car brakes hard.
const SKITCH_LINE_POSITION_GAIN: f32 = 0.3;
const SKITCH_LINE_VELOCITY_GAIN: f32 = 0.15;
/// A line this far from its estimate is a new car or a teleport: take it.
const SKITCH_LINE_SNAP: f32 = 1.0;

fn smoothed_lines(
    state: &mut Vec<skate_host::bridge::GrabLine>,
    target: &[skate_host::bridge::GrabLine],
    dt: f32,
) -> Vec<skate_host::bridge::GrabLine> {
    state.retain(|f| target.iter().any(|t| t.tag == f.tag));
    for t in target {
        let Some(f) = state.iter_mut().find(|f| f.tag == t.tag) else {
            state.push(*t);
            continue;
        };
        let mut far = 0.0f32;
        for i in 0..3 {
            f.frame[3][i] += f.velocity[i] * dt;
            far += (t.frame[3][i] - f.frame[3][i]).powi(2);
        }
        if far.sqrt() > SKITCH_LINE_SNAP {
            *f = *t;
            continue;
        }
        for i in 0..3 {
            f.frame[3][i] += (t.frame[3][i] - f.frame[3][i]) * SKITCH_LINE_POSITION_GAIN;
            f.velocity[i] += (t.velocity[i] - f.velocity[i]) * SKITCH_LINE_VELOCITY_GAIN;
            for row in 0..3 {
                f.frame[row][i] += (t.frame[row][i] - f.frame[row][i]) * SKITCH_LINE_POSITION_GAIN;
            }
        }
        f.points = t.points;
        f.approach = t.approach;
    }
    state.clone()
}

#[cfg(test)]
mod skitch_line_tests {
    #[test]
    fn lines_move_with_the_car_velocity() {
        let line = skate_host::bridge::GrabLine {
            tag: 7,
            frame: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [10., 0., 5., 0.]],
            velocity: [17., 0., -2., 0.],
            points: [[-1., -2., 0.5, 0.], [1., -2., 0.5, 0.]],
            approach: [0., -1., 0., 0.],
        };
        let moved = super::extrapolated_lines(&[line], 0.5)[0];
        assert_eq!(moved.frame[3], [18.5, 0., 4., 0.]);
        assert_eq!(moved.points, line.points);
    }

    fn car(z: f32, v: f32) -> skate_host::bridge::GrabLine {
        skate_host::bridge::GrabLine {
            tag: 7,
            frame: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., z, 0.]],
            velocity: [0., 0., v, 0.],
            points: [[-1., -2., 0.5, 0.], [1., -2., 0.5, 0.]],
            approach: [0., -1., 0., 0.],
        }
    }

    #[test]
    fn smoothing_removes_sampling_jitter_but_follows_the_car_and_snaps_on_a_jump() {
        // A car at 10 m/s whose estimate is off by up to +-4 cm per tick.
        let dt = 1.0 / 60.0;
        let noise = [0.04f32, -0.03, 0.02, -0.04, 0.03, -0.02, 0.04, -0.01];
        let (mut state, mut raw, mut smooth) = (Vec::new(), Vec::new(), Vec::new());
        for k in 0..120 {
            let truth = 10.0 * dt * k as f32;
            let est = car(truth + noise[k % noise.len()], 10.0 + noise[(k + 3) % noise.len()] * 20.0);
            raw.push(est.frame[3][2]);
            smooth.push(super::smoothed_lines(&mut state, &[est], dt)[0].frame[3][2]);
        }
        let jitter = |v: &[f32]| (60..119).map(|i| (v[i + 1] - 2.0 * v[i] + v[i - 1]).abs()).sum::<f32>() / 59.0;
        assert!(jitter(&smooth) < jitter(&raw) * 0.4, "{} vs {}", jitter(&smooth), jitter(&raw));
        // Still on the car (steady state has no lag beyond the noise).
        assert!((smooth[119] - 10.0 * dt * 119.0).abs() < 0.05);
        // A teleport or another car is taken at once.
        let jumped = super::smoothed_lines(&mut state, &[car(50.0, 0.0)], dt)[0];
        assert_eq!(jumped.frame[3][2], 50.0);
        // A line that left the set is forgotten.
        assert!(super::smoothed_lines(&mut state, &[], dt).is_empty() && state.is_empty());
    }
}

#[cfg(test)]
mod shown_tests {
    use super::*;

    fn shared(prev: Snapshot, latest: Snapshot, blend: f32) -> Shared {
        Shared {
            status: 0,
            message: String::new(),
            snapshot: latest,
            previous: Some(prev),
            blend,
            steps_done: 0,
            held: Default::default(),
            lifecycle: Default::default(),
            hud: Default::default(),
            records: Default::default(),
            record_labels: Default::default(),
            host_impulses: Vec::new(),
        }
    }

    #[test]
    fn frames_between_ticks_blend_and_jumps_do_not() {
        let at = |tick, x: f32, heading| Snapshot {
            tick,
            root: Vec3::new(x, 0.0, 0.0),
            heading,
            character_pose: Arc::new(vec![Mat4::from_translation(Vec3::new(x, 0.0, 0.0))]),
            ..Default::default()
        };
        let s = shared(at(1, 0.0, 350.0), at(2, 1.0, 10.0), 0.25).shown();
        assert!((s.root.x - 0.25).abs() < 1e-5);
        assert!((s.character_pose[0].w_axis.x - 0.25).abs() < 1e-5);
        assert!((s.heading.rem_euclid(360.0) - 355.0).abs() < 1e-3, "heading takes the short way: {}", s.heading);
        assert_eq!(shared(at(1, 0.0, 0.0), at(2, 1.0, 0.0), 1.0).shown().root.x, 1.0);
        assert_eq!(shared(at(1, 0.0, 0.0), at(2, 5.0, 0.0), 0.25).shown().root.x, 5.0, "a teleport is not blended");
        assert_eq!(shared(at(7, 0.0, 0.0), at(2, 1.0, 0.0), 0.25).shown().root.x, 1.0, "a reset tick is not blended");
    }

    #[test]
    fn a_frame_shows_the_step_before_its_own() {
        let at = |tick, x: f32| Snapshot { tick, root: Vec3::new(x, 0.0, 0.0), ..Default::default() };
        let mut s = shared(at(4, 4.0), at(5, 5.0), 1.0);
        s.held.extend([3u64, 4].map(|step| Held { step, snapshot: at(step, step as f32), previous: None, blend: 1.0 }));
        assert_eq!(s.at_step(3).0.tick, 3, "its own step done too: still the one before");
        assert_eq!(s.at_step(9).0.tick, 4, "worker behind: the newest it has");
        assert_eq!(s.at_step(1).0.tick, 5, "nothing kept that early: the latest");
    }
}

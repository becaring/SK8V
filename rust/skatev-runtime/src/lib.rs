#![allow(clippy::missing_safety_doc)]
// The library name is the shipped DLL name (SkateVRuntime.dll) the host loads.
#![allow(non_snake_case)]
//! SkateVRuntime.dll: the adopted Skate runtime behind a versioned C ABI.
//! Layout mirrors host/include/skatev_runtime.h; both sides assert sizes.

pub mod coords;
pub mod board_pose;
pub mod curbs;
pub mod dynamic;
pub mod ped;
pub mod ped_bodies;
pub mod quirk;
pub mod line;
pub mod rails;
pub mod skin;
mod worker;
mod crash;
pub mod world;
pub mod live;
pub mod materials;
pub mod surfaces;
pub mod ground;
pub mod grid;
pub mod plies;
#[cfg(windows)]
mod private_heap;
/// All runtime allocations use a private heap (no lock shared with GTA).
#[cfg(windows)]
#[global_allocator]
static ALLOCATOR: private_heap::PrivateHeap = private_heap::PrivateHeap;

use std::ffi::{CStr, c_char, c_void};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use bevy_math::Vec3;
use worker::{Job, Pad, Shared};

pub const SKATEV_ABI_VERSION: u32 = 8;
/// Mashup lock commit; its skate/ crates come from donor cb79689.
const SKATE_SOURCE: &str = "2010-rust-rewrite-mashup@ab43b8a9 (skate cb7968930)";

pub use worker::{STATUS_ACTIVATING, STATUS_ACTIVE, STATUS_ERROR, STATUS_LOADING, STATUS_READY};

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SvVec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SvQuat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SvCreateInfo {
    pub size: u32,
    pub abi_version: u32,
    /// Formerly world callbacks; ignored.
    pub reserved: [u8; 32],
    /// Converted Skate data root (`<dir>/assets`), UTF-8.
    pub data_root_utf8: *const c_char,
    /// SVWC path; the ground, surface and prop-bound sidecars sit beside it.
    pub world_cache_utf8: *const c_char,
    /// Runtime log file, UTF-8; null disables logging.
    pub log_path_utf8: *const c_char,
    /// Triangle budget for the skinned Skate skater mesh (0 = full detail).
    pub skater_triangle_budget: u32,
    /// Bit 0: present only Skate's board; GTA renders the player ped itself.
    /// Bit 1: Hall of Meat (bail anytime, bone damage scoring, broken-bone
    /// slow-mo, the `homscoring` HUD). Bit 2: its metric panel. Bit 3: goofy
    /// profile stance (clear: regular).
    pub presentation_flags: u32,
}

/// One shaded presentation triangle, GTA space: a flat colour and a smooth
/// (Gouraud) colour per corner.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvColorTri {
    pub a: SvVec3,
    pub b: SvVec3,
    pub c: SvVec3,
    pub rgba: [u8; 4],
    pub rgba_a: [u8; 4],
    pub rgba_b: [u8; 4],
    pub rgba_c: [u8; 4],
    /// Texture coordinates u,v for corners a, b, c (character only).
    pub uv: [f32; 6],
    /// Lighting per corner a, b, c (+ pad), to multiply a real texture.
    pub light: [u8; 4],
    /// Character texture slot (`sv_get_character_texture`), 0xFFFF none.
    pub texture: u16,
    /// Bit 0: alpha cutout, draw only with its texture.
    pub flags: u16,
}

/// A moving GTA entity as an oriented box, GTA space. `tag` identifies the
/// entity in hits (the runtime sets the high bit internally).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvBox {
    pub tag: u32,
    pub center: SvVec3,
    pub rotation: SvQuat,
    pub half_extents: SvVec3,
}

/// The GTA character to present wearing Skate's pose: the player's model and
/// its worn outfit (component drawable/texture per `GET_PED_*_VARIATION`
/// slot), looked up in the local ped cache. A null cache root selects
/// Skate's own skater.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SvCharacter {
    pub size: u32,
    pub model_hash: u32,
    pub cache_root_utf8: *const c_char,
    pub drawable: [u16; 12],
    pub texture: [u8; 12],
    pub triangle_budget: u32,
    /// Folder holding the live ped's `skeleton.json` (read by the host from
    /// GTA's memory), or null. Posed instead of the cached model's skeleton
    /// when the two differ: a model replaced by a mod, or an add-on ped.
    pub live_skeleton_utf8: *const c_char,
}

/// RetailQuirk::BackwardsMan settings.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvQuirkConfig {
    pub size: u32,
    pub enabled: u32,
    /// XInput button mask held together (rising edge) to trigger.
    pub chord: u32,
    /// Ignored: only the retail routine exists (formerly the launch model).
    pub remount_delay: u32,
    pub model: u32,
    pub speed: f32,
    pub lift: f32,
    pub backward: u32,
}

/// Skate's off-board / launch internals (GTA space) for retail-quirk work.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvQuirkState {
    pub size: u32,
    pub board_state: u32,
    pub deck_velocity: SvVec3,
    pub com_trajectory_velocity: SvVec3,
    pub launch_start_velocity: SvVec3,
    pub launch_com_velocity: SvVec3,
    pub launch_tick: u64,
    /// bit0 use COM velocity now, bit1 last launch on COM branch, bit2 last launch overridden
    pub flags: u32,
    /// RetailQuirk::BackwardsMan assist phase (0 idle).
    pub assist_phase: u32,
}

/// A host entity the board touched, GTA space.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvDynamicHit {
    pub tag: u32,
    pub point: SvVec3,
    pub normal: SvVec3,
}

/// One XInput controller, unmodified (the source engine's raw transport).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvPad {
    pub connected: u32,
    pub packet: u32,
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvInput {
    pub size: u32,
    pub dt_seconds: f32,
    pub aspect_ratio: f32,
    pub pad: SvPad,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvSpawn {
    pub size: u32,
    /// GTA world position the board is placed at (ground contact).
    pub position: SvVec3,
    pub heading_degrees: f32,
    pub aspect_ratio: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SvOutput {
    pub size: u32,
    pub status: u32,
    pub tick: u64,
    pub skater_position: SvVec3,
    pub skater_rotation: SvQuat,
    pub skater_heading_degrees: f32,
    pub board_position: SvVec3,
    pub board_rotation: SvQuat,
    pub velocity: SvVec3,
    pub camera_valid: u32,
    pub camera_position: SvVec3,
    /// GTA script-camera rotation, degrees (pitch, roll, yaw), order 2.
    pub camera_rotation: SvVec3,
    pub camera_fov: f32,
    pub state_utf8: [u8; 64],
}

impl Default for SvOutput {
    fn default() -> Self {
        Self {
            size: size_of::<Self>() as u32,
            status: STATUS_LOADING,
            tick: 0,
            skater_position: SvVec3::default(),
            skater_rotation: SvQuat {
                w: 1.0,
                ..Default::default()
            },
            skater_heading_degrees: 0.0,
            board_position: SvVec3::default(),
            board_rotation: SvQuat {
                w: 1.0,
                ..Default::default()
            },
            velocity: SvVec3::default(),
            camera_valid: 0,
            camera_position: SvVec3::default(),
            camera_rotation: SvVec3::default(),
            camera_fov: 0.0,
            state_utf8: [0; 64],
        }
    }
}

pub const SCORE_SWITCH: u32 = 1 << 0;
pub const SCORE_FAKIE: u32 = 1 << 1;
pub const SCORE_NOLLIE: u32 = 1 << 2;
pub const SCORE_CLEAN: u32 = 1 << 3;
pub const SCORE_SKETCHY: u32 = 1 << 4;

/// Presentation snapshot of Skate's own scorer. The host renders it only.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SvScoreState {
    pub size: u32,
    pub sequence_active: u32,
    pub sequence_score: f32,
    pub line_score: f32,
    pub completed_lines: f32,
    pub multiplier: f32,
    pub line_timer: f32,
    pub flags: u32,
    pub trick_utf8: [u8; 96],
}

impl Default for SvScoreState {
    fn default() -> Self {
        Self {
            size: size_of::<Self>() as u32,
            sequence_active: 0,
            sequence_score: 0.0,
            line_score: 0.0,
            completed_lines: 0.0,
            multiplier: 1.0,
            line_timer: 0.0,
            flags: 0,
            trick_utf8: [0; 96],
        }
    }
}

struct Runtime {
    jobs: mpsc::Sender<Job>,
    shared: Arc<Mutex<Shared>>,
    /// Signalled with `Shared::steps_done`; `steps_sent` counts sv_step's jobs.
    steps_sent: AtomicU64,
    log: worker::Log,
    /// The presentation of the snapshot `sv_get_output` last returned. The
    /// host reads output (camera, ped placement), character pose and board
    /// pose with separate calls; the worker publishes in between some frames
    /// (each 16.7 ms), which put the ped one Skate tick off its board and
    /// camera (~16 cm at 10 m/s). The pose getters answer from this frame.
    frame: Mutex<Option<Frame>>,
}

struct Frame {
    character_pose: Arc<Vec<bevy_math::Mat4>>,
    board_pose: Option<[bevy_math::Mat4; 7]>,
    board_entity: Option<bevy_math::Mat4>,
    tick: u64,
}

fn v3(v: Vec3) -> SvVec3 {
    SvVec3 {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

/// Copies `s` into a host buffer of `capacity` bytes, NUL-terminated and cut
/// on a char boundary; returns the bytes written without the NUL.
unsafe fn copy_c(buf: *mut c_char, capacity: u32, s: &str) -> u32 {
    if buf.is_null() || capacity == 0 {
        return 0;
    }
    let mut n = s.len().min(capacity as usize - 1);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    unsafe {
        ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, n);
        *buf.add(n) = 0;
    }
    n as u32
}

/// Host time limit in seconds: NaN or negative restores Skate's own (None),
/// 0 removes the limit (infinity).
fn time_limit(seconds: f32) -> Option<f32> {
    if seconds.is_nan() || seconds < 0.0 {
        None
    } else if seconds == 0.0 {
        Some(f32::INFINITY)
    } else {
        Some(seconds)
    }
}

fn copy_str<const N: usize>(dst: &mut [u8; N], s: &str) {
    *dst = [0; N];
    let mut n = s.len().min(N - 1);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    dst[..n].copy_from_slice(&s.as_bytes()[..n]);
}

unsafe fn path_arg(p: *const c_char) -> Option<PathBuf> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn file_logger(path: Option<PathBuf>) -> worker::Log {
    let file = path.and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    });
    let file = Mutex::new(file);
    let start = std::time::Instant::now();
    Arc::new(move |msg: &str| {
        if let Ok(mut f) = file.lock()
            && let Some(f) = f.as_mut()
        {
            let line = format!("[{:>9.3}] {msg}\n", start.elapsed().as_secs_f64());
            let _ = f.write_all(line.as_bytes());
        }
    })
}

/// Runs `f`, turning a panic into `fallback` so it never unwinds into C++.
fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

unsafe fn runtime<'a>(rt: *const c_void) -> Option<&'a Runtime> {
    unsafe { (rt as *const Runtime).as_ref() }
}

#[unsafe(no_mangle)]
pub extern "C" fn sv_api_version() -> u32 {
    SKATEV_ABI_VERSION
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_create(info: *const SvCreateInfo) -> *mut c_void {
    guard(ptr::null_mut(), || {
        let Some(info) = (unsafe { info.as_ref() }) else {
            return ptr::null_mut();
        };
        if info.abi_version != SKATEV_ABI_VERSION
            || (info.size as usize) < size_of::<SvCreateInfo>()
        {
            return ptr::null_mut();
        }
        let (Some(root), Some(cache)) = (unsafe { path_arg(info.data_root_utf8) }, unsafe {
            path_arg(info.world_cache_utf8)
        }) else {
            return ptr::null_mut();
        };
        let log_path = unsafe { path_arg(info.log_path_utf8) };
        crash::install(log_path.as_deref());
        let log = file_logger(log_path);
        log(&format!(
            "SkateVRuntime ABI {SKATEV_ABI_VERSION}, Skate source {SKATE_SOURCE}"
        ));
        log(&format!(
            "data {} cache {}",
            root.display(),
            cache.display()
        ));
        let budget = info.skater_triangle_budget as usize;
        let board_only = info.presentation_flags & 1 != 0;
        let hall_of_meat = (info.presentation_flags & 2 != 0, info.presentation_flags & 4 != 0);
        // Skate profile stance: 0 regular (default), 1 goofy (flag 8).
        let natural_stance = u32::from(info.presentation_flags & 8 != 0);
        log(&format!("profile stance: {}", if natural_stance == 0 { "regular" } else { "goofy" }));
        let (jobs, shared) =
            worker::spawn(root, cache, budget, board_only, hall_of_meat, natural_stance, Arc::clone(&log));
        Box::into_raw(Box::new(Runtime {
            jobs,
            shared,
            steps_sent: AtomicU64::new(0),
            log,
            frame: Mutex::new(None),
        })) as *mut c_void
    })
}

/// Releases the handle. The worker exits once its job channel closes; it is
/// not joined so this is safe to call from any host thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_destroy(rt: *mut c_void) {
    if rt.is_null() {
        return;
    }
    guard((), || {
        let rt = unsafe { Box::from_raw(rt as *mut Runtime) };
        audio::shutdown();
        (rt.log)("runtime released");
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_request_activate(rt: *mut c_void, spawn: *const SvSpawn) -> u32 {
    guard(0, || unsafe { request_activate(rt, spawn, false) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_deactivate(rt: *mut c_void) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else {
            return 0;
        };
        rt.jobs.send(Job::Suspend).is_ok() as u32
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_step(rt: *mut c_void, input: *const SvInput) -> u32 {
    guard(0, || {
        let (Some(rt), Some(input)) = (unsafe { runtime(rt) }, unsafe { input.as_ref() }) else {
            return 0;
        };
        if (input.size as usize) < size_of::<SvInput>() || !input.dt_seconds.is_finite() {
            return 0;
        }
        let p = input.pad;
        let pad = Pad {
            connected: p.connected != 0,
            packet: p.packet,
            buttons: p.buttons,
            triggers: [p.left_trigger, p.right_trigger],
            left: [p.left_x, p.left_y],
            right: [p.right_x, p.right_y],
        };
        let sent = rt
            .jobs
            .send(Job::Step {
                dt: input.dt_seconds,
                pad,
                aspect: input.aspect_ratio,
            })
            .is_ok();
        if sent {
            rt.steps_sent.fetch_add(1, Ordering::Release);
        }
        sent as u32
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_output(rt: *const c_void, out: *mut SvOutput) -> u32 {
    guard(0, || {
        let (Some(rt), Some(out)) = (unsafe { runtime(rt) }, unsafe { out.as_mut() }) else {
            return 0;
        };
        // The frame shows the step before its own (Shared::at_step), never
        // waiting: a frame that showed by turns its own step or the one
        // before moved the skater, ped and camera in uneven steps (the
        // Rockstar Editor records them so), and waiting for its own held
        // GTA's main thread for Skate's whole tick.
        let sent = rt.steps_sent.load(Ordering::Acquire);
        let shared = rt.shared.lock().unwrap();
        let (s, shown) = shared.at_step(sent.saturating_sub(1)); // shown: between two ticks (worker::Shown)
        *rt.frame.lock().unwrap() = Some(Frame {
            character_pose: Arc::clone(&shown.character_pose),
            board_pose: shown.board_pose,
            board_entity: shown.board_entity,
            tick: s.tick,
        });
        let mut o = SvOutput {
            status: shared.status,
            tick: s.tick,
            ..Default::default()
        };
        o.skater_position = v3(shown.root);
        let q = shown.root_rotation;
        o.skater_rotation = SvQuat {
            x: q.x,
            y: q.y,
            z: q.z,
            w: q.w,
        };
        o.skater_heading_degrees = shown.heading;
        o.board_position = v3(shown.deck);
        let q = shown.deck_rotation;
        o.board_rotation = SvQuat {
            x: q.x,
            y: q.y,
            z: q.z,
            w: q.w,
        };
        o.velocity = v3(s.velocity);
        if let Some((pos, forward, fov)) = shown.camera {
            o.camera_valid = 1;
            o.camera_position = v3(pos);
            o.camera_rotation = v3(coords::gta_camera_rotation(forward));
            o.camera_fov = fov;
        }
        copy_str(&mut o.state_utf8, &s.state);
        *out = o;
        1
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_score_state(rt: *const c_void, out: *mut SvScoreState) -> u32 {
    guard(0, || {
        let (Some(rt), Some(out)) = (unsafe { runtime(rt) }, unsafe { out.as_mut() }) else {
            return 0;
        };
        let shared = rt.shared.lock().unwrap();
        let v = &shared.snapshot.score;
        let mut o = SvScoreState {
            sequence_active: v.sequence_active as u32,
            sequence_score: v.sequence_score,
            line_score: v.line_score,
            completed_lines: v.completed_lines,
            multiplier: v.multiplier,
            line_timer: v.line_timer,
            ..Default::default()
        };
        for (on, bit) in [
            (v.switch, SCORE_SWITCH),
            (v.fakie, SCORE_FAKIE),
            (v.nollie, SCORE_NOLLIE),
            (v.clean, SCORE_CLEAN),
            (v.sketchy, SCORE_SKETCHY),
        ] {
            if on {
                o.flags |= bit;
            }
        }
        copy_str(&mut o.trick_utf8, &v.trick_name);
        *out = o;
        1
    })
}

/// Selects the GTA character that wears Skate's pose (or Skate's own skater).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_character(rt: *mut c_void, character: *const SvCharacter) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else {
            return 0;
        };
        let request = match unsafe { character.as_ref() } {
            Some(c) if (c.size as usize) >= size_of::<SvCharacter>() => {
                unsafe { path_arg(c.cache_root_utf8) }.map(|root| worker::CharacterRequest {
                    cache_root: root,
                    model_hash: c.model_hash,
                    variation: ped::Variation {
                        drawable: c.drawable,
                        texture: c.texture,
                    },
                    budget: c.triangle_budget as usize,
                    live_skeleton: unsafe { path_arg(c.live_skeleton_utf8) },
                })
            }
            _ => None,
        };
        rt.jobs.send(Job::Character(request)).is_ok() as u32
    })
}

/// Host entities the board touched during the last published ticks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_dynamic_hits(
    rt: *const c_void,
    out: *mut SvDynamicHit,
    capacity: u32,
) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else {
            return 0;
        };
        if out.is_null() {
            return 0;
        }
        let shared = rt.shared.lock().unwrap();
        let hits = &shared.snapshot.hits;
        let n = hits.len().min(capacity as usize);
        for (i, (tag, p, nrm)) in hits.iter().take(n).enumerate() {
            unsafe {
                *out.add(i) = SvDynamicHit {
                    tag: tag & !dynamic::DYNAMIC_TAG,
                    point: v3(*p),
                    normal: v3(*nrm),
                };
            }
        }
        n as u32
    })
}

/// Skate's skinned skater + board for presentation; returns triangles copied.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_skater_mesh(
    rt: *const c_void,
    out: *mut SvColorTri,
    capacity: u32,
) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else {
            return 0;
        };
        if out.is_null() {
            return 0;
        }
        let shared = rt.shared.lock().unwrap();
        let mesh = &shared.snapshot.mesh;
        let n = mesh.len().min(capacity as usize);
        for (i, t) in mesh.iter().take(n).enumerate() {
            unsafe {
                *out.add(i) = SvColorTri {
                    a: v3(t.points[0]),
                    b: v3(t.points[1]),
                    c: v3(t.points[2]),
                    rgba: t.rgba,
                    rgba_a: t.vertex_rgba[0],
                    rgba_b: t.vertex_rgba[1],
                    rgba_c: t.vertex_rgba[2],
                    uv: [
                        t.uv[0][0], t.uv[0][1], t.uv[1][0], t.uv[1][1], t.uv[2][0], t.uv[2][1],
                    ],
                    light: [t.light[0], t.light[1], t.light[2], 0],
                    texture: t.texture,
                    flags: u16::from(t.cutout) | (u16::from(t.board) << 1),
                };
            }
        }
        n as u32
    })
}

/// Ped presentation: the character's bone world matrices (GTA space), one
/// per skeleton bone in the ped's own bone order, 16 floats each laid out as
/// RAGE stores them (X axis, Y axis, Z axis, translation; 4 floats per row).
/// Returns the bone count written (0 when no pose is available).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_character_pose(
    rt: *const c_void,
    out: *mut f32,
    capacity_bones: u32,
) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else {
            return 0;
        };
        if out.is_null() {
            return 0;
        }
        // The frame `sv_get_output` returned, so the ped matches its board
        // and camera (the live snapshot before any output was read).
        let framed = rt.frame.lock().unwrap().as_ref().map(|f| Arc::clone(&f.character_pose));
        let pose = framed.unwrap_or_else(|| rt.shared.lock().unwrap().snapshot.character_pose.clone());
        let n = pose.len().min(capacity_bones as usize);
        for (i, m) in pose.iter().take(n).enumerate() {
            let cols = m.to_cols_array();
            unsafe { ptr::copy_nonoverlapping(cols.as_ptr(), out.add(16 * i), 16) };
        }
        n as u32
    })
}

/// Optional ABI 8 native board presentation. Fixed bone order is defined in
/// board_pose::NAMES; matrices are GTA world transforms, RAGE row layout.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SvBoardPose {
    pub size: u32,
    pub bone_count: u32,
    pub tick: u64,
    pub entity: [f32; 16],
    pub world: [[f32; 16]; 7],
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_board_pose(rt: *const c_void, out: *mut SvBoardPose) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0; };
        if out.is_null() || unsafe { (*out).size } != std::mem::size_of::<SvBoardPose>() as u32 {
            return 0;
        }
        let shared = rt.shared.lock().unwrap();
        if shared.status != STATUS_ACTIVE { return 0; }
        // The frame `sv_get_output` returned (see `Runtime::frame`).
        let (pose, entity, tick) = match rt.frame.lock().unwrap().as_ref() {
            Some(f) => (f.board_pose, f.board_entity, f.tick),
            None => (shared.snapshot.board_pose, shared.snapshot.board_entity, shared.snapshot.tick),
        };
        let (Some(pose), Some(entity)) = (pose, entity) else { return 0; };
        unsafe { *out = SvBoardPose {
            size: std::mem::size_of::<SvBoardPose>() as u32,
            bone_count: 7,
            tick,
            entity: entity.to_cols_array(),
            world: pose.map(|m| m.to_cols_array()),
        }; }
        1
    })
}

/// Character texture slot `index` (as in `SvColorTri::texture`): the
/// streamed texture dictionary and the texture inside it, NUL-terminated.
/// Returns 1 when the slot exists.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_character_texture(
    rt: *const c_void,
    index: u32,
    dict: *mut c_char,
    dict_capacity: u32,
    texture: *mut c_char,
    texture_capacity: u32,
) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else {
            return 0;
        };
        let textures = rt.shared.lock().unwrap().snapshot.textures.clone();
        let Some((d, t)) = textures.get(index as usize) else {
            return 0;
        };
        for (s, buf, cap) in [(d, dict, dict_capacity), (t, texture, texture_capacity)] {
            if buf.is_null() || cap == 0 || s.len() >= cap as usize {
                return 0;
            }
            unsafe {
                ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, s.len());
                *buf.add(s.len()) = 0;
            }
        }
        1
    })
}

/// Recent distinct trick labels from Skate's scorer, newest last, one per
/// line (NUL-terminated); returns the byte length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_trick_history(
    rt: *const c_void,
    buf: *mut c_char,
    capacity: u32,
) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else {
            return 0;
        };
        let text = rt.shared.lock().unwrap().snapshot.tricks.join("\n");
        unsafe { copy_c(buf, capacity, &text) }
    })
}

/// Configures RetailQuirk::BackwardsMan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_configure_quirk(rt: *mut c_void, config: *const SvQuirkConfig) -> u32 {
    guard(0, || {
        let (Some(rt), Some(c)) = (unsafe { runtime(rt) }, unsafe { config.as_ref() }) else {
            return 0;
        };
        if (c.size as usize) < size_of::<SvQuirkConfig>() {
            return 0;
        }
        // Only the retail routine exists; the launch-model fields are ignored.
        let config = quirk::BackwardsManConfig {
            enabled: c.enabled != 0,
            chord: c.chord as u16,
        };
        rt.jobs.send(Job::QuirkConfig(config)).is_ok() as u32
    })
}

/// Host-requested RetailQuirk trigger (0 = BackwardsMan), e.g. a keyboard shortcut.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_trigger_quirk(rt: *mut c_void, quirk_id: u32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else {
            return 0;
        };
        if quirk_id != 0 {
            return 0;
        }
        rt.jobs
            .send(Job::QuirkTrigger(quirk::RetailQuirk::BackwardsMan))
            .is_ok() as u32
    })
}

/// Play a showcase line file (`line.rs`) from its start pose while skating;
/// the worker logs the outcome. 1 = request queued.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_play_line(rt: *mut c_void, path_utf8: *const c_char) -> u32 {
    guard(0, || {
        let (Some(rt), Some(path)) = (unsafe { runtime(rt) }, unsafe { path_arg(path_utf8) }) else {
            return 0;
        };
        rt.jobs.send(Job::PlayLine(path)).is_ok() as u32
    })
}

/// Skate's off-board / launch internals for diagnostics and regression tests.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_quirk_state(rt: *const c_void, out: *mut SvQuirkState) -> u32 {
    guard(0, || {
        let (Some(rt), Some(out)) = (unsafe { runtime(rt) }, unsafe { out.as_mut() }) else {
            return 0;
        };
        let shared = rt.shared.lock().unwrap();
        let q = shared.snapshot.quirk;
        *out = SvQuirkState {
            size: size_of::<SvQuirkState>() as u32,
            board_state: q.board_state,
            deck_velocity: v3(q.deck_velocity),
            com_trajectory_velocity: v3(q.com_trajectory_velocity),
            launch_start_velocity: v3(q.launch_start_velocity),
            launch_com_velocity: v3(q.launch_com_velocity),
            launch_tick: q.launch_tick,
            flags: u32::from(q.use_com_velocity)
                | (u32::from(q.launch_com_branch) << 1)
                | (u32::from(q.launch_overridden) << 2),
            assist_phase: shared.snapshot.assist_phase,
        };
        1
    })
}

/// Copies the current status message (NUL-terminated); returns its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_status_text(
    rt: *const c_void,
    buf: *mut c_char,
    capacity: u32,
) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else {
            return 0;
        };
        let msg = rt.shared.lock().unwrap().message.clone();
        unsafe { copy_c(buf, capacity, &msg) }
    })
}

// Every export below is optional on the host (looked up with GetProcAddress).
pub mod lifecycle;

/// Lifecycle phases published in `SvLifecycleState::phase`.
pub const LIFECYCLE_INACTIVE: u32 = 0;
pub const LIFECYCLE_RIDING: u32 = 2;
pub const LIFECYCLE_DISMOUNTING: u32 = 3;
pub const LIFECYCLE_RELEASED: u32 = 4;

/// Where the GTA player is while Skate is inactive (GTA space).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvLifecycleTrack {
    pub size: u32,
    /// Bit 0: the player is in a vehicle (diagnostic only).
    pub flags: u32,
    /// Ground contact under the player (or the vehicle).
    pub position: SvVec3,
    pub heading_degrees: f32,
}

/// Lifecycle publication: preparation, collision working set, Skate's own
/// on/off-board state and the monitor phase.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvLifecycleState {
    pub size: u32,
    /// `LIFECYCLE_*`.
    pub phase: u32,
    /// The Skate session exists (activation needs no session build).
    pub prepared: u32,
    /// The session is being built in the background.
    pub preparing: u32,
    /// A collision working set is being built off the simulation thread.
    pub building: u32,
    /// Skate `PhysicalStateId` (100 PhysicsGround, 500 BipedGround, ...).
    pub skate_state: u32,
    /// SkateboardController board possession (1 held, 2 released, ...).
    pub board_possession: u32,
    pub session_builds: u32,
    pub collision_builds: u32,
    pub last_prepare_ms: f32,
    pub last_activate_ms: f32,
    /// Centre of the installed collision working set (GTA space).
    pub collision_center: SvVec3,
}

/// Lifecycle: the GTA player's position while Skate is inactive. The first
/// call prepares the Skate session there in the background (once per
/// process); later calls keep the collision working set around the player.
/// Ignored while Skate is active. Returns 1 when queued.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_lifecycle_track(
    rt: *mut c_void,
    track: *const SvLifecycleTrack,
) -> u32 {
    guard(0, || {
        let (Some(rt), Some(t)) = (unsafe { runtime(rt) }, unsafe { track.as_ref() }) else {
            return 0;
        };
        if (t.size as usize) < size_of::<SvLifecycleTrack>() {
            return 0;
        }
        let at = Vec3::new(t.position.x, t.position.y, t.position.z);
        if !at.is_finite() || !t.heading_degrees.is_finite() {
            return 0;
        }
        rt.jobs
            .send(Job::Track {
                at,
                heading: t.heading_degrees,
            })
            .is_ok() as u32
    })
}

/// Retired map-states export, kept because hosts still call it: does nothing
/// and returns 0 (static collision comes from GTA's live physics level).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_map_states_set(_rt: *mut c_void, _hashes: *const u32, _count: u32) -> u32 {
    0
}

/// The in-game Hall of Meat toggle (bail scoring, broken-bone slow-mo and
/// its HUD). Switching off abandons a bail in progress. Returns 1 when
/// queued.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_hall_of_meat(rt: *mut c_void, enabled: u32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        rt.jobs.send(Job::HallOfMeat(enabled != 0)).is_ok() as u32
    })
}

/// Skate's air-time teleport (a skater airborne too long is returned to the
/// last checkpoint) in seconds: 0 removes it, a negative value restores
/// Skate's own. Returns 1 when queued.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_air_limit(rt: *mut c_void, seconds: f32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        rt.jobs.send(Job::AirLimit(time_limit(seconds))).is_ok() as u32
    })
}

/// Skate 3's camera (overlay patch 0044): 0 Low, 1 High (retail's default);
/// applied live and to every later session. Returns 1 when queued.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_camera_type(rt: *mut c_void, camera_type: u32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        (camera_type < 2 && rt.jobs.send(Job::CameraType(camera_type)).is_ok()) as u32
    })
}

/// Skate 3's difficulty (overlay patch 0043): 0 easy, 1 normal, 2 hardcore,
/// 3 motorized; applied live and to every later session. Returns 1 when queued.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_difficulty(rt: *mut c_void, index: u32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        (index < 4 && rt.jobs.send(Job::Difficulty(index)).is_ok()) as u32
    })
}

/// The SkateV ramp-lip rule (overlay patch 0033): nonzero (the default) lets
/// a rolling board stall on a lip only riding up into it slowly; 0 restores
/// retail grind admission. Returns 1 when queued.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_lip_rule(rt: *mut c_void, enabled: u32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        rt.jobs.send(Job::LipRule(enabled != 0)).is_ok() as u32
    })
}

/// VerboseLog: nonzero writes the periodic lines (perf, skater trace,
/// collision streaming) to the runtime log. Returns 1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_verbose_log(_rt: *mut c_void, enabled: u32) -> u32 {
    worker::VERBOSE_LOG.store(enabled != 0, std::sync::atomic::Ordering::Relaxed);
    1
}

/// How far behind a car's rearmost bound the skitch grab line sits, in metres
/// (negative or NaN: ignored). Returns 1 when queued.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_skitch_standoff(rt: *mut c_void, metres: f32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        if !(metres >= 0.0 && metres <= 5.0) { return 0; }
        rt.jobs.send(Job::SkitchStandoff(metres)).is_ok() as u32
    })
}

/// GTA's physics level (ABI 8 optional export): the address of record 0 of the
/// table of loaded collision objects and the range of GTA's main image. The
/// static map is read from GTA's own loaded collision; `table` 0 withdraws it
/// (no new areas are built until it is set again). Returns 1 when queued.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_physics_level(rt: *mut c_void, table: u64, image_lo: u64, image_hi: u64) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        let level = (table != 0 && image_hi > image_lo).then(|| live::Level {
            base: table as usize,
            image: (image_lo as usize, image_hi as usize),
        });
        rt.jobs.send(Job::PhysicsLevel(level)).is_ok() as u32
    })
}

/// GTA handle of the vehicle being skitched, 0 when none (ABI 8 optional
/// export): lets the host keep its own colliders off that vehicle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_skitch_vehicle(rt: *const c_void) -> u32 {
    guard(0, || unsafe { runtime(rt) }.map_or(0, |rt| rt.shared.lock().unwrap().lifecycle.skitch_vehicle))
}

/// The automatic bail reset's time limit in seconds: 0 removes it (a bail
/// then ends when the body settles or on the player's A/X recover), a
/// negative value restores Skate's own. Returns 1 when queued.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_bail_limit(rt: *mut c_void, seconds: f32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        rt.jobs.send(Job::BailLimit(time_limit(seconds))).is_ok() as u32
    })
}


/// The record book's state: the last placed score (for a call-out) and the
/// bests overall and at the current spot.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvRecordState {
    pub size: u32,
    /// Increments with every score that placed (0: none yet).
    pub sequence: u32,
    /// 1 Hall of Meat bail, 2 banked line.
    pub category: u32,
    pub score: u32,
    /// 1-based rank overall / at its spot (0: outside that top ten).
    pub rank: u32,
    pub spot_rank: u32,
    pub hom_best: u32,
    pub hom_spot_best: u32,
    pub line_best: u32,
    pub line_spot_best: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SvRecordEntry {
    pub score: u32,
    pub reserved: u32,
    /// Unix seconds.
    pub time: u64,
    pub character_utf8: [u8; 24],
    pub spot_name_utf8: [u8; 48],
}

impl Default for SvRecordEntry {
    fn default() -> Self {
        Self { score: 0, reserved: 0, time: 0, character_utf8: [0; 24], spot_name_utf8: [0; 48] }
    }
}

unsafe fn str_arg(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Opens (or starts) the record book at `path_utf8`. Returns 1 when open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_records_open(rt: *mut c_void, path_utf8: *const c_char) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        let Some(path) = (unsafe { path_arg(path_utf8) }) else { return 0 };
        rt.shared.lock().unwrap().records.open(&path, &rt.log);
        1
    })
}

/// Who is skating and where (GTA zone code and display name): the context
/// the next placed score is tagged with. Returns 1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_records_set_context(
    rt: *mut c_void,
    character_utf8: *const c_char,
    spot_utf8: *const c_char,
    spot_name_utf8: *const c_char,
) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        let context = records::Context {
            character: unsafe { str_arg(character_utf8) },
            spot: unsafe { str_arg(spot_utf8) },
            spot_name: unsafe { str_arg(spot_name_utf8) },
        };
        rt.shared.lock().unwrap().records.context = context;
        1
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_records_get_state(rt: *const c_void, out: *mut SvRecordState) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        let Some(out) = (unsafe { out.as_mut() }) else { return 0 };
        if out.size as usize != size_of::<SvRecordState>() {
            return 0;
        }
        let shared = rt.shared.lock().unwrap();
        let r = &shared.records;
        let spot = Some(r.context.spot.as_str());
        use records::Category::{HallOfMeat, Line};
        *out = SvRecordState {
            size: out.size,
            sequence: r.last.sequence,
            category: r.last.category,
            score: r.last.score,
            rank: r.last.placement.rank,
            spot_rank: r.last.placement.spot_rank,
            hom_best: r.book.best(HallOfMeat, None),
            hom_spot_best: r.book.best(HallOfMeat, spot),
            line_best: r.book.best(Line, None),
            line_spot_best: r.book.best(Line, spot),
        };
        1
    })
}

/// Copies the top entries of `category` (1 Hall of Meat, 2 line), overall or
/// (spot_only = 1) at the current spot, best first. Returns the count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_records_get_top(
    rt: *const c_void,
    category: u32,
    spot_only: u32,
    out: *mut SvRecordEntry,
    capacity: u32,
) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        let Some(c) = records::Category::from_u32(category) else { return 0 };
        if out.is_null() || capacity == 0 {
            return 0;
        }
        let shared = rt.shared.lock().unwrap();
        let r = &shared.records;
        let spot = (spot_only != 0).then_some(r.context.spot.as_str());
        let top = r.book.top(c, spot);
        let n = top.len().min(capacity as usize);
        for (i, e) in top.iter().take(n).enumerate() {
            let mut o = SvRecordEntry { score: e.score, time: e.time, ..Default::default() };
            copy_str(&mut o.character_utf8, &e.character);
            copy_str(&mut o.spot_name_utf8, &e.spot_name);
            unsafe { *out.add(i) = o };
        }
        n as u32
    })
}

/// One of Skate 3's record labels (records::LABELS) by language id, UTF-8.
/// Returns its length (0: unavailable).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_records_label(
    rt: *const c_void,
    id_utf8: *const c_char,
    buf: *mut c_char,
    capacity: u32,
) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        if buf.is_null() || capacity == 0 {
            return 0;
        }
        let id = unsafe { str_arg(id_utf8) };
        let Some(text) = rt.shared.lock().unwrap().record_labels.get(&id).cloned() else { return 0 };
        unsafe { copy_c(buf, capacity, &text) }
    })
}

/// One Hall of Meat x-ray vertex: GTA world space, straight-alpha colour as
/// the `defaulthom` shader writes it (display encoded, not linear).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvXrayVertex {
    pub position: SvVec3,
    pub rgba: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvXrayFrame {
    pub size: u32,
    /// Vertices published (a triangle list; 0: no x-ray this tick).
    pub vertex_count: u32,
    pub tick: u64,
}

/// The latest Hall of Meat broken-bone x-ray. Fills `frame` (always, when
/// valid) and copies up to `capacity` vertices; returns the count copied,
/// or 0 when `capacity` is short of `frame.vertex_count` (nothing copied).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_xray(
    rt: *const c_void,
    frame: *mut SvXrayFrame,
    out: *mut SvXrayVertex,
    capacity: u32,
) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0 };
        if frame.is_null() || unsafe { (*frame).size } < size_of::<SvXrayFrame>() as u32 {
            return 0;
        }
        let (xray, tick) = {
            let shared = rt.shared.lock().unwrap();
            (shared.snapshot.xray.clone(), shared.snapshot.tick)
        };
        unsafe {
            *frame = SvXrayFrame { size: size_of::<SvXrayFrame>() as u32, vertex_count: xray.len() as u32, tick };
        }
        if xray.is_empty() || out.is_null() || (capacity as usize) < xray.len() {
            return 0;
        }
        for (i, v) in xray.iter().enumerate() {
            unsafe { *out.add(i) = SvXrayVertex { position: v3(v.position), rgba: v.rgba } };
        }
        xray.len() as u32
    })
}

/// Lifecycle entry: Skate starts with the skater standing at `spawn`, off
/// the board with the board in hand (Skate's own off-board return), and gets
/// on only when the player presses Skate's board action. Same status rules as
/// `sv_request_activate`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_lifecycle_enter(rt: *mut c_void, spawn: *const SvSpawn) -> u32 {
    guard(0, || unsafe { request_activate(rt, spawn, true) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_lifecycle_get_state(
    rt: *const c_void,
    out: *mut SvLifecycleState,
) -> u32 {
    guard(0, || {
        let (Some(rt), Some(out)) = (unsafe { runtime(rt) }, unsafe { out.as_mut() }) else {
            return 0;
        };
        let l = rt.shared.lock().unwrap().lifecycle;
        *out = SvLifecycleState {
            size: size_of::<SvLifecycleState>() as u32,
            phase: l.phase,
            prepared: u32::from(l.prepared),
            preparing: u32::from(l.preparing),
            building: u32::from(l.building),
            skate_state: l.skate_state,
            board_possession: l.board_possession,
            session_builds: l.session_builds,
            collision_builds: l.collision_builds,
            last_prepare_ms: l.last_prepare_ms,
            last_activate_ms: l.last_activate_ms,
            collision_center: v3(l.collision_centre),
        };
        1
    })
}

unsafe fn request_activate(rt: *mut c_void, spawn: *const SvSpawn, offboard: bool) -> u32 {
    let (Some(rt), Some(s)) = (unsafe { runtime(rt) }, unsafe { spawn.as_ref() }) else {
        return 0;
    };
    if (s.size as usize) < size_of::<SvSpawn>() {
        return 0;
    }
    let p = Vec3::new(s.position.x, s.position.y, s.position.z);
    if !p.is_finite() || !s.heading_degrees.is_finite() {
        return 0;
    }
    {
        let mut shared = rt.shared.lock().unwrap();
        if shared.status != STATUS_READY {
            return 0;
        }
        shared.status = STATUS_ACTIVATING;
    }
    rt.jobs
        .send(Job::Activate {
            spawn: p,
            heading: s.heading_degrees,
            aspect: s.aspect_ratio,
            offboard,
        })
        .is_ok() as u32
}

#[cfg(test)]
mod lifecycle_abi_tests {
    use super::*;
    use std::mem::offset_of;

    /// Must match the static_asserts in the header's lifecycle block.
    #[test]
    fn lifecycle_layout_matches_header() {
        assert_eq!(size_of::<SvLifecycleTrack>(), 24);
        assert_eq!(offset_of!(SvLifecycleTrack, position), 8);
        assert_eq!(offset_of!(SvLifecycleTrack, heading_degrees), 20);
        assert_eq!(size_of::<SvLifecycleState>(), 56);
        assert_eq!(offset_of!(SvLifecycleState, skate_state), 20);
        assert_eq!(offset_of!(SvLifecycleState, last_prepare_ms), 36);
        assert_eq!(offset_of!(SvLifecycleState, collision_center), 44);
        assert_eq!(lifecycle::Phase::Released as u32, LIFECYCLE_RELEASED);
    }

    #[test]
    fn lifecycle_exports_reject_bad_input() {
        assert_eq!(
            unsafe { sv_lifecycle_track(ptr::null_mut(), ptr::null()) },
            0
        );
        assert_eq!(
            unsafe { sv_lifecycle_enter(ptr::null_mut(), ptr::null()) },
            0
        );
        assert_eq!(
            unsafe { sv_lifecycle_get_state(ptr::null(), ptr::null_mut()) },
            0
        );
    }
}

// Skate 3's original APT trickdisplay HUD: the movie runs on the Skate worker
// (src/hud.rs); exports sv_set_hud_viewport, sv_get_hud_texture,
// sv_get_hud_frame. Layout asserts: hud::tests::hud_abi_layout_matches_header.
pub mod hud;
pub mod hom;
pub mod xray;
pub mod records;
pub use hud::{
    SvHudDraw, SvHudFrame, SvHudTexture, SvHudVertex, SvHudViewport, sv_get_hud_frame, sv_get_hud_texture,
    sv_set_hud_viewport, sv_show_radio,
};

// Dry Skate 3 gameplay voices per emitter; contract in the header's audio
// block. Engine and rings live in `audio` (one per process).
pub mod audio;
pub use audio::{SvAudioConfig, SvAudioEmitter, SvAudioStatus};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_audio_configure(rt: *mut c_void, config: *const SvAudioConfig) -> u32 {
    guard(0, || {
        let (Some(rt), Some(c)) = (unsafe { runtime(rt) }, unsafe { config.as_ref() }) else {
            return 0;
        };
        if (c.size as usize) < size_of::<SvAudioConfig>() || !(c.sample_rate == 0 || c.sample_rate == audio::SAMPLE_RATE) {
            return 0;
        }
        let Some(dir) = (unsafe { path_arg(c.cache_dir_utf8) }) else {
            (rt.log)("audio: no cache directory configured; Skate audio off");
            return 0;
        };
        if !dir.join("raw").join("audiofiles").is_dir() {
            (rt.log)(&format!(
                "audio: {} is not a prepared Skate audio cache (run tools/prepare-skate-audio.py); Skate audio off",
                dir.display()
            ));
            return 0;
        }
        (rt.log)(&format!("audio: configuring from {}", dir.display()));
        audio::configure(dir, c.master_gain, Arc::clone(&rt.log)) as u32
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_audio_set_paused(rt: *mut c_void, paused: u32) -> u32 {
    guard(0, || {
        if unsafe { runtime(rt) }.is_none() {
            return 0;
        }
        audio::set_paused(paused != 0) as u32
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_audio_emitters(rt: *const c_void, out: *mut SvAudioEmitter, capacity: u32) -> u32 {
    guard(0, || {
        if unsafe { runtime(rt) }.is_none() {
            return 0;
        }
        let out: &mut [SvAudioEmitter] = if out.is_null() || capacity == 0 {
            &mut []
        } else {
            unsafe { std::slice::from_raw_parts_mut(out, capacity as usize) }
        };
        audio::emitters(out)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_audio_pull(
    rt: *mut c_void,
    emitter: u32,
    out: *mut f32,
    frames: u32,
    info: *mut SvAudioEmitter,
) -> u32 {
    guard(0, || {
        if unsafe { runtime(rt) }.is_none() || out.is_null() {
            return 0;
        }
        let buf = unsafe { std::slice::from_raw_parts_mut(out, frames as usize) };
        audio::pull(emitter as usize, buf, unsafe { info.as_mut() })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_audio_get_status(rt: *const c_void, out: *mut SvAudioStatus) -> u32 {
    guard(0, || {
        let (Some(_), Some(out)) = (unsafe { runtime(rt) }, unsafe { out.as_mut() }) else {
            return 0;
        };
        audio::status(out);
        1
    })
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvDynamicBody {
    pub size: u32,
    pub model_hash: u32,
    pub position: SvVec3,
    pub right: SvVec3,
    pub forward: SvVec3,
    pub up: SvVec3,
    pub fallback: SvBox,
    pub flags: u32,
    /// GET_ENTITY_VELOCITY, GTA world, m/s.
    pub linear_velocity: SvVec3,
    /// GET_ENTITY_ROTATION_VELOCITY, GTA world, rad/s.
    pub angular_velocity: SvVec3,
    /// Vehicle `bumper_r` bone, GTA world; valid iff `grab_flags & 1`.
    pub grab_point: SvVec3,
    pub grab_flags: u32,
}

/// Impulse the board/skater applied to a host entity (contact exchange).
/// GTA world space; newton-seconds. `mass_kg` is the mass the solve used
/// (0: unknown, the entity moved kinematically). Each impulse is returned once.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvDynamicImpulse {
    pub size: u32,
    /// Host entity tag (SvBox::tag of the body).
    pub tag: u32,
    /// Skate tick the impulse was solved on.
    pub tick: u64,
    pub point: SvVec3,
    pub impulse: SvVec3,
    pub mass_kg: f32,
    pub reserved: u32,
    /// The angular velocity change (GTA world, rad/s) the impulse gives
    /// the rigid body the solve used (a ped: its posed ragdoll compound,
    /// I^-1 (r x J) about its centre of mass). Zero for other bodies.
    pub angular_velocity_change: SvVec3,
    pub reserved2: u32,
}

/// Optional ABI 8 exact model-bound path. No host entity ID is used as a surface.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_dynamic_bodies(rt: *mut c_void, bodies: *const SvDynamicBody, count: u32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0; };
        if count > 128 || (count > 0 && bodies.is_null()) { return 0; }
        // Every record must carry the size of this struct.
        let mut accepted = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let body = unsafe { bodies.add(i).read_unaligned() };
            if body.size as usize != size_of::<SvDynamicBody>() { return 0; }
            let Some(body) = dynamic::HostBody::from_abi(&body) else { return 0; };
            accepted.push(body);
        }
        if rt.jobs.send(Job::DynamicBodies(accepted)).is_err() { return 0; }
        count
    })
}

/// What Skate's audio engine is playing (sound classes with level and share
/// of time, stream levels): UTF-8 lines, NUL-terminated within `capacity`.
/// Returns the bytes written without the NUL (0: nothing to show).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_audio_debug_text(_rt: *mut c_void, out: *mut u8, capacity: u32) -> u32 {
    guard(0, || {
        unsafe { copy_c(out as *mut c_char, capacity, &audio::debug_text()) }
    })
}

/// One posed part of a GTA ped's ragdoll compound (GTA world space).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SvPedPart {
    pub size: u32,
    /// The ped's tag (`SvBox::tag` convention: kind 1 = ped).
    pub tag: u32,
    /// 1 capsule (along `forward`), 3 box.
    pub kind: u32,
    /// Ragdoll component (fragment child index) of this part.
    pub component: u32,
    pub right: SvVec3,
    pub forward: SvVec3,
    pub up: SvVec3,
    pub centre: SvVec3,
    /// Capsule: (radius, half segment length, radius). Box: outer half extents.
    pub half_extents: SvVec3,
    /// Capsule radius; box edge radius (bound margin).
    pub radius: f32,
    /// The part's authored ragdoll mass, kg.
    pub mass_kg: f32,
    /// The ped's GET_ENTITY_VELOCITY / ROTATION_VELOCITY.
    pub linear_velocity: SvVec3,
    pub angular_velocity: SvVec3,
}

/// Replaces the set of ped parts the next ticks collide with (up to 1024
/// parts; every record must carry the size of this struct). Returns the
/// number accepted (0 on a malformed array).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_ped_parts(rt: *mut c_void, parts: *const SvPedPart, count: u32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0; };
        if count > 1024 || (count > 0 && parts.is_null()) { return 0; }
        let v = |p: SvVec3| bevy_math::Vec3::new(p.x, p.y, p.z);
        let mut accepted = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let p = unsafe { parts.add(i).read_unaligned() };
            if p.size as usize != size_of::<SvPedPart>() { return 0; }
            let part = ped_bodies::PedPart {
                tag: p.tag,
                kind: p.kind,
                axes: [v(p.right), v(p.forward), v(p.up)],
                centre: v(p.centre),
                half_extents: v(p.half_extents),
                radius: p.radius,
                mass: p.mass_kg,
                linear_velocity: v(p.linear_velocity),
                angular_velocity: v(p.angular_velocity),
            };
            if !part.valid() { return 0; }
            accepted.push(part);
        }
        if rt.jobs.send(Job::PedParts(accepted)).is_err() { return 0; }
        count
    })
}

/// Drains up to `capacity` impulses the board/skater applied to host entities
/// since the previous call (oldest first). Returns the number written.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_dynamic_impulses(rt: *mut c_void, out: *mut SvDynamicImpulse, capacity: u32) -> u32 {
    guard(0, || {
        let Some(rt) = (unsafe { runtime(rt) }) else { return 0; };
        if capacity == 0 || out.is_null() { return 0; }
        // The first record's size must be this struct's (the array stride).
        if unsafe { (out as *const u32).read_unaligned() } as usize != size_of::<SvDynamicImpulse>() { return 0; }
        let mut shared = rt.shared.lock().unwrap();
        let n = shared.host_impulses.len().min(capacity as usize);
        for (k, src) in shared.host_impulses.drain(..n).enumerate() {
            unsafe { out.add(k).write_unaligned(src) };
        }
        n as u32
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    /// Must match the static_asserts in host/include/skatev_runtime.h.
    #[test]
    fn abi_layout_matches_header() {
        assert_eq!(size_of::<SvBoardPose>(), 528);
        // Contact exchange (header static_asserts).
        assert_eq!(size_of::<SvDynamicBody>(), 144);
        assert_eq!(offset_of!(SvDynamicBody, linear_velocity), 104);
        assert_eq!(offset_of!(SvDynamicBody, grab_point), 128);
        assert_eq!(size_of::<SvDynamicImpulse>(), 64);
        assert_eq!(offset_of!(SvDynamicImpulse, angular_velocity_change), 48);
        assert_eq!(offset_of!(SvDynamicImpulse, tick), 8);
        assert_eq!(offset_of!(SvDynamicImpulse, point), 16);
        assert_eq!(offset_of!(SvDynamicImpulse, mass_kg), 40);
        assert_eq!(offset_of!(SvBoardPose, entity), 16);
        assert_eq!(offset_of!(SvBoardPose, world), 80);
        assert_eq!(size_of::<SvVec3>(), 12);
        assert_eq!(size_of::<SvQuat>(), 16);
        assert_eq!(offset_of!(SvCreateInfo, reserved), 8);
        assert_eq!(size_of::<SvCreateInfo>(), 72);
        assert_eq!(offset_of!(SvCreateInfo, skater_triangle_budget), 64);
        assert_eq!(size_of::<SvColorTri>(), 84);
        assert_eq!(size_of::<SvXrayVertex>(), 28);
        assert_eq!(size_of::<SvXrayFrame>(), 16);
        assert_eq!(size_of::<SvRecordState>(), 40);
        assert_eq!(size_of::<SvRecordEntry>(), 88);
        assert_eq!(offset_of!(SvXrayFrame, tick), 8);
        assert_eq!(offset_of!(SvColorTri, uv), 52);
        assert_eq!(offset_of!(SvColorTri, light), 76);
        assert_eq!(offset_of!(SvColorTri, texture), 80);
        assert_eq!(offset_of!(SvColorTri, rgba_a), 40);
        assert_eq!(offset_of!(SvColorTri, rgba), 36);
        assert_eq!(size_of::<SvBox>(), 44);
        assert_eq!(offset_of!(SvBox, rotation), 16);
        assert_eq!(size_of::<SvDynamicHit>(), 28);
        assert_eq!(size_of::<SvCharacter>(), 64);
        assert_eq!(size_of::<SvQuirkConfig>(), 32);
        assert_eq!(size_of::<SvQuirkState>(), 72);
        assert_eq!(offset_of!(SvQuirkState, launch_tick), 56);
        assert_eq!(offset_of!(SvCharacter, drawable), 16);
        assert_eq!(offset_of!(SvCharacter, texture), 40);
        assert_eq!(offset_of!(SvCharacter, triangle_budget), 52);
        assert_eq!(offset_of!(SvCharacter, live_skeleton_utf8), 56);
        assert_eq!(offset_of!(SvCreateInfo, data_root_utf8), 40);
        assert_eq!(size_of::<SvPad>(), 20);
        assert_eq!(offset_of!(SvPad, buttons), 8);
        assert_eq!(offset_of!(SvPad, left_x), 12);
        assert_eq!(size_of::<SvInput>(), 32);
        assert_eq!(offset_of!(SvInput, pad), 12);
        assert_eq!(size_of::<SvSpawn>(), 24);
        assert_eq!(size_of::<SvOutput>(), 184);
        assert_eq!(offset_of!(SvOutput, tick), 8);
        assert_eq!(offset_of!(SvOutput, skater_heading_degrees), 44);
        assert_eq!(offset_of!(SvOutput, camera_valid), 88);
        assert_eq!(offset_of!(SvOutput, camera_fov), 116);
        assert_eq!(offset_of!(SvOutput, state_utf8), 120);
        assert_eq!(size_of::<SvScoreState>(), 128);
        assert_eq!(offset_of!(SvScoreState, trick_utf8), 32);
    }

    #[test]
    fn create_rejects_bad_abi_and_missing_paths() {
        assert_eq!(sv_api_version(), SKATEV_ABI_VERSION);
        let mut info = SvCreateInfo {
            size: size_of::<SvCreateInfo>() as u32,
            abi_version: 1,
            reserved: [0; 32],
            data_root_utf8: ptr::null(),
            world_cache_utf8: ptr::null(),
            log_path_utf8: ptr::null(),
            skater_triangle_budget: 0,
            presentation_flags: 0,
        };
        assert!(unsafe { sv_create(&info) }.is_null());
        info.abi_version = SKATEV_ABI_VERSION;
        assert!(unsafe { sv_create(&info) }.is_null(), "paths are required");
        assert!(unsafe { sv_create(ptr::null()) }.is_null());
    }

    #[test]
    fn missing_data_reports_error_status_without_crashing() {
        let root = c"Z:/definitely/missing/skate-data/assets";
        let cache = c"Z:/definitely/missing/los-santos.svwc";
        let info = SvCreateInfo {
            size: size_of::<SvCreateInfo>() as u32,
            abi_version: SKATEV_ABI_VERSION,
            reserved: [0; 32],
            data_root_utf8: root.as_ptr(),
            world_cache_utf8: cache.as_ptr(),
            log_path_utf8: ptr::null(),
            skater_triangle_budget: 0,
            presentation_flags: 0,
        };
        let rt = unsafe { sv_create(&info) };
        assert!(!rt.is_null());
        let mut out = SvOutput::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            assert_eq!(unsafe { sv_get_output(rt, &mut out) }, 1);
            if out.status == STATUS_ERROR || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(out.status, STATUS_ERROR);
        let mut buf = [0 as c_char; 256];
        assert!(unsafe { sv_get_status_text(rt, buf.as_mut_ptr(), 256) } > 0);
        let spawn = SvSpawn {
            size: size_of::<SvSpawn>() as u32,
            ..Default::default()
        };
        assert_eq!(
            unsafe { sv_request_activate(rt, &spawn) },
            0,
            "cannot activate while errored"
        );
        unsafe { sv_destroy(rt) };
    }

    #[test]
    fn strings_are_truncated_on_char_boundaries() {
        let mut b = [0u8; 4];
        copy_str(&mut b, "a\u{e9}\u{20ac}");
        assert_eq!(&b, b"a\xc3\xa9\0");
    }
}

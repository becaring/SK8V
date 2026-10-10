#pragma once
#include <cstdint>
#include <cstddef>

// Mirrors rust/skatev-runtime/src/lib.rs. Layout is asserted on both sides
// (`abi_layout_matches_header` in Rust, static_asserts here).
#define SKATEV_ABI_VERSION 8u

enum SvStatus : std::uint32_t {
    SV_STATUS_LOADING = 0,
    SV_STATUS_READY = 1,
    SV_STATUS_ACTIVATING = 2,
    SV_STATUS_ACTIVE = 3,
    SV_STATUS_ERROR = 4,
};

enum SvScoreFlags : std::uint32_t {
    SV_SCORE_SWITCH = 1u << 0,
    SV_SCORE_FAKIE = 1u << 1,
    SV_SCORE_NOLLIE = 1u << 2,
    SV_SCORE_CLEAN = 1u << 3,
    SV_SCORE_SKETCHY = 1u << 4,
};

struct SvVec3 { float x, y, z; };
struct SvQuat { float x, y, z, w; };
struct SvCreateInfo {
    std::uint32_t size;
    std::uint32_t abi_version;
    std::uint8_t reserved[32];    // formerly world callbacks; ignored (zero)
    const char* data_root_utf8;   // converted Skate data, <dir>/assets
    const char* world_cache_utf8; // SVWC v2 Los Santos collision cache
    const char* log_path_utf8;    // runtime log; null disables
    std::uint32_t skater_triangle_budget; // skinned skater mesh budget (0 = full)
    std::uint32_t presentation_flags; // bit 0: only Skate's board; GTA renders the player ped
                                      // bit 1: Hall of Meat; bit 2: every HoM metric panel
                                      // bit 3: goofy profile stance (clear: regular)
};

// One shaded presentation triangle, GTA space: flat colour, a smooth
// (Gouraud) colour per corner, and for GTA characters the real texture.
struct SvColorTri {
    SvVec3 a, b, c;
    std::uint8_t rgba[4];
    std::uint8_t rgba_a[4], rgba_b[4], rgba_c[4];
    float uv[6];              // u,v for corners a, b, c
    std::uint8_t light[4];    // lighting alone per corner a, b, c (+ pad)
    std::uint16_t texture;    // sv_get_character_texture slot, 0xFFFF none
    std::uint16_t flags;      // bit 0: alpha cutout, draw only textured
};
constexpr std::uint16_t kSvNoTexture = 0xFFFF;
constexpr std::uint16_t kSvTriCutout = 1;
constexpr std::uint16_t kSvTriBoard = 2;

// A moving GTA entity as an oriented box, GTA space.
struct SvBox {
    std::uint32_t tag; // host entity id (high bit reserved by the runtime)
    SvVec3 center;
    SvQuat rotation;
    SvVec3 half_extents;
};

// The GTA character presented wearing Skate's pose: player model + worn outfit
// (GET_PED_DRAWABLE/TEXTURE_VARIATION per slot), looked up in the local ped
// cache. A null cache root selects Skate's own skater.
struct SvCharacter {
    std::uint32_t size;
    std::uint32_t model_hash;
    const char* cache_root_utf8;
    std::uint16_t drawable[12];
    std::uint8_t texture[12];
    std::uint32_t triangle_budget;
    // Folder holding the live ped's skeleton.json (game_probe SkeletonJson), or null: posed
    // instead of the cached model's skeleton when the two differ (a replaced or add-on model).
    const char* live_skeleton_utf8;
};

// RetailQuirk::BackwardsMan settings (Skate 3 backwards-man / speed glitch).
struct SvQuirkConfig {
    std::uint32_t size;
    std::uint32_t enabled;
    std::uint32_t chord;         // XInput button mask held together (rising edge)
    std::uint32_t remount_delay; // ticks airborne before the remount press
    std::uint32_t model;         // 0 Natural (no override), 1 CompatApprox, 2 Retail (recorded inputs)
    float speed;                 // CompatApprox horizontal launch speed, m/s
    float lift;                  // CompatApprox vertical launch speed, m/s
    std::uint32_t backward;      // 1 backward relative to the skater, 0 forward
};

// Skate's off-board / launch internals (GTA space), for diagnostics.
struct SvQuirkState {
    std::uint32_t size;
    std::uint32_t board_state;
    SvVec3 deck_velocity;
    SvVec3 com_trajectory_velocity;
    SvVec3 launch_start_velocity;
    SvVec3 launch_com_velocity;
    std::uint64_t launch_tick;
    std::uint32_t flags;        // bit0 COM velocity now, bit1 last launch COM branch, bit2 overridden
    std::uint32_t assist_phase; // 0 idle
};

// A host entity the board touched, GTA space.
struct SvDynamicHit {
    std::uint32_t tag;
    SvVec3 point;
    SvVec3 normal;
};

// One XInput controller, unmodified (the Skate engine's raw transport).
struct SvPad {
    std::uint32_t connected;
    std::uint32_t packet;
    std::uint16_t buttons;
    std::uint8_t left_trigger, right_trigger;
    std::int16_t left_x, left_y, right_x, right_y;
};

struct SvInput {
    std::uint32_t size;
    float dt_seconds;
    float aspect_ratio;
    SvPad pad;
};

struct SvSpawn {
    std::uint32_t size;
    SvVec3 position; // GTA ground contact point
    float heading_degrees;
    float aspect_ratio;
};

struct SvOutput {
    std::uint32_t size;
    std::uint32_t status;
    std::uint64_t tick;
    SvVec3 skater_position;
    SvQuat skater_rotation;
    float skater_heading_degrees;
    SvVec3 board_position;
    SvQuat board_rotation; // GTA space; long axis = q*(0,-1,0), up = q*(0,0,1)
    SvVec3 velocity;
    std::uint32_t camera_valid;
    SvVec3 camera_position;
    SvVec3 camera_rotation; // degrees: pitch, roll, yaw (rotation order 2)
    float camera_fov;
    char state_utf8[64];
};

struct SvScoreState {
    std::uint32_t size;
    std::uint32_t sequence_active;
    float sequence_score;
    float line_score;
    float completed_lines;
    float multiplier;
    float line_timer;
    std::uint32_t flags;
    char trick_utf8[96];
};

using SvApiVersionFn = std::uint32_t(__cdecl*)();
using SvCreateFn = void*(__cdecl*)(const SvCreateInfo*);
using SvDestroyFn = void(__cdecl*)(void*);
using SvRequestActivateFn = std::uint32_t(__cdecl*)(void*, const SvSpawn*);
using SvDeactivateFn = std::uint32_t(__cdecl*)(void*);
using SvStepFn = std::uint32_t(__cdecl*)(void*, const SvInput*);
using SvGetOutputFn = std::uint32_t(__cdecl*)(const void*, SvOutput*);
using SvGetScoreStateFn = std::uint32_t(__cdecl*)(const void*, SvScoreState*);
using SvGetStatusTextFn = std::uint32_t(__cdecl*)(const void*, char*, std::uint32_t);
using SvGetDynamicHitsFn = std::uint32_t(__cdecl*)(const void*, SvDynamicHit*, std::uint32_t);
using SvGetSkaterMeshFn = std::uint32_t(__cdecl*)(const void*, SvColorTri*, std::uint32_t);
using SvGetTrickHistoryFn = std::uint32_t(__cdecl*)(const void*, char*, std::uint32_t);
using SvSetCharacterFn = std::uint32_t(__cdecl*)(void*, const SvCharacter*);
using SvConfigureQuirkFn = std::uint32_t(__cdecl*)(void*, const SvQuirkConfig*);
using SvTriggerQuirkFn = std::uint32_t(__cdecl*)(void*, std::uint32_t);
using SvGetQuirkStateFn = std::uint32_t(__cdecl*)(const void*, SvQuirkState*);
using SvGetCharacterPoseFn = std::uint32_t(__cdecl*)(const void*, float*, std::uint32_t);
using SvGetCharacterTextureFn = std::uint32_t(__cdecl*)(const void*, std::uint32_t, char*, std::uint32_t, char*,
                                                        std::uint32_t);

static_assert(sizeof(SvVec3) == 12);
static_assert(sizeof(SvQuat) == 16);
static_assert(offsetof(SvCreateInfo, reserved) == 8 && sizeof(SvCreateInfo::reserved) == 32);
static_assert(sizeof(SvCreateInfo) == 72);
static_assert(offsetof(SvCreateInfo, skater_triangle_budget) == 64);
static_assert(sizeof(SvColorTri) == 84);
static_assert(offsetof(SvColorTri, uv) == 52);
static_assert(offsetof(SvColorTri, light) == 76);
static_assert(offsetof(SvColorTri, texture) == 80);
static_assert(offsetof(SvColorTri, rgba_a) == 40);
static_assert(offsetof(SvColorTri, rgba) == 36);
static_assert(sizeof(SvBox) == 44);
static_assert(offsetof(SvBox, rotation) == 16);
static_assert(sizeof(SvDynamicHit) == 28);
static_assert(sizeof(SvCharacter) == 64);
static_assert(sizeof(SvQuirkConfig) == 32);
static_assert(sizeof(SvQuirkState) == 72);
static_assert(offsetof(SvQuirkState, launch_tick) == 56);
static_assert(offsetof(SvCharacter, drawable) == 16);
static_assert(offsetof(SvCharacter, texture) == 40);
static_assert(offsetof(SvCharacter, triangle_budget) == 52);
static_assert(offsetof(SvCharacter, live_skeleton_utf8) == 56);
static_assert(offsetof(SvCreateInfo, data_root_utf8) == 40);
static_assert(sizeof(SvPad) == 20);
static_assert(offsetof(SvPad, buttons) == 8);
static_assert(offsetof(SvPad, left_x) == 12);
static_assert(sizeof(SvInput) == 32);
static_assert(offsetof(SvInput, pad) == 12);
static_assert(sizeof(SvSpawn) == 24);
static_assert(sizeof(SvOutput) == 184);
static_assert(offsetof(SvOutput, tick) == 8);
static_assert(offsetof(SvOutput, skater_heading_degrees) == 44);
static_assert(offsetof(SvOutput, camera_valid) == 88);
static_assert(offsetof(SvOutput, camera_fov) == 116);
static_assert(offsetof(SvOutput, state_utf8) == 120);
static_assert(sizeof(SvScoreState) == 128);
static_assert(offsetof(SvScoreState, trick_utf8) == 32);

// ===== ABI 8 additions (finishing pass). Every export below is optional: the
// host looks each up with GetProcAddress and degrades when it is missing.
// Each workstream owns exactly one block; do not edit another block.

// ---- lifecycle (ABI 8) ----
// Seamless GTA <-> Skate lifecycle (ledger P0-1; docs/DECISIONS.md
// "Lifecycle through Skate's own off-board state"). Host usage:
//  1. Once the player is loaded, and every ~250 ms while skate mode is
//     inactive: sv_lifecycle_track(rt, &track) with the player's position (on
//     foot or in a vehicle). The first call prepares the Skate session there
//     on the runtime's worker (once per process; GTA's thread never waits);
//     later calls keep the collision working set around the player.
//  2. Board action: sv_lifecycle_enter(rt, &spawn) (same status rules as
//     sv_request_activate). Skate starts with the skater standing off the
//     board, board in hand. The player's Y mounts/dismounts normally.
//  3. SV_LIFECYCLE_RELEASED means off-board with Skate still active. Only
//     the host put-away action restores GTA control and calls sv_deactivate;
//     the prepared Skate session remains resident for the next entry.
// Exports: sv_lifecycle_track, sv_lifecycle_enter, sv_lifecycle_get_state
// (optional; resolve by name).
enum SvLifecyclePhase : std::uint32_t {
    SV_LIFECYCLE_INACTIVE = 0,    // Skate not running
    SV_LIFECYCLE_MOUNTING = 1,    // entered off the board; Skate is getting on
    SV_LIFECYCLE_RIDING = 2,      // Skate owns the player (riding, or off-board after a bail)
    SV_LIFECYCLE_DISMOUNTING = 3, // player pressed Y on the board
    SV_LIFECYCLE_RELEASED = 4,    // off the board, board in hand; Skate stays active
};

struct SvLifecycleTrack {
    std::uint32_t size;
    std::uint32_t flags;   // bit 0: in a vehicle (diagnostic)
    SvVec3 position;       // ground contact under the player, GTA space
    float heading_degrees;
};

struct SvLifecycleState {
    std::uint32_t size;
    std::uint32_t phase;            // SvLifecyclePhase
    std::uint32_t prepared;         // the Skate session exists
    std::uint32_t preparing;        // the session is being built in the background
    std::uint32_t building;         // a collision working set is being built
    std::uint32_t skate_state;      // Skate PhysicalStateId (100 PhysicsGround, 500 BipedGround, ...)
    std::uint32_t board_possession; // SkateboardController: 1 held, 2 released, ...
    std::uint32_t session_builds;
    std::uint32_t collision_builds;
    float last_prepare_ms;
    float last_activate_ms;
    SvVec3 collision_center;        // installed working set, GTA space
};

using SvLifecycleTrackFn = std::uint32_t(__cdecl*)(void*, const SvLifecycleTrack*);
using SvLifecycleEnterFn = std::uint32_t(__cdecl*)(void*, const SvSpawn*);
using SvLifecycleGetStateFn = std::uint32_t(__cdecl*)(const void*, SvLifecycleState*);

// ---- map states (ABI 8, optional export) ----
// Script-toggled map states (IPLs) have collision beside the world cache
// (`CACHE.map-states.txt`, one state name per line). The host sends the name
// hashes (rage_joaat) of the states GTA has active near the player, the whole
// nearby set each time it changes and before an activation:
// sv_map_states_set(rt, hashes, count). Export: sv_map_states_set.
using SvMapStatesSetFn = std::uint32_t(__cdecl*)(void*, const std::uint32_t*, std::uint32_t);

// ---- Hall of Meat toggle (ABI 8, optional export) ----
// sv_set_hall_of_meat(rt, enabled): switches Hall of Meat (bail scoring,
// broken-bone slow-mo, its HUD) on or off in game; presentation flag bit 1
// is only the start value. Switching off abandons a bail in progress.
// Returns 1 when queued. Export: sv_set_hall_of_meat.
using SvSetHallOfMeatFn = std::uint32_t(__cdecl*)(void*, std::uint32_t);

// ---- Bail time limit (ABI 8, optional export) ----
// sv_set_bail_limit(rt, seconds): the automatic bail reset's time limit. 0
// removes it (a bail ends when the body settles or on A/X recover); a
// negative value restores Skate's own. Returns 1 when queued.
// Export: sv_set_bail_limit.
using SvSetBailLimitFn = std::uint32_t(__cdecl*)(void*, float);

// ---- Skitch standoff (ABI 8, optional export) ----
// sv_set_skitch_standoff(rt, metres): how far behind a car's rearmost bound the
// skitch grab line sits (the invisible wall the rider grabs), so neither
// Skate's contacts nor GTA's physics on the car touch the rider's hands.
// 0 puts the line on the bodywork. Returns 1 when queued.
// Export: sv_set_skitch_standoff.
using SvSetSkitchStandoffFn = std::uint32_t(__cdecl*)(void*, float);

// ---- Live collision (ABI 8, optional export) ----
// sv_set_physics_level(rt, table, image_lo, image_hi): the address of record 0
// of GTA's physics-level table (every collision object the game has loaded)
// and the range of GTA's main image. The runtime then reads the static map
// from GTA's own loaded collision instead of the offline cache; table 0
// withdraws it. Returns 1 when queued.
// Export: sv_set_physics_level.
using SvSetPhysicsLevelFn = std::uint32_t(__cdecl*)(void*, std::uint64_t, std::uint64_t, std::uint64_t);

// sv_skitch_vehicle(rt): GTA handle of the vehicle being skitched, 0 when none.
// Export: sv_skitch_vehicle.
using SvSkitchVehicleFn = std::uint32_t(__cdecl*)(const void*);

// sv_set_air_limit(rt, seconds): Skate's air-time teleport (a skater airborne
// too long is returned to the last checkpoint). 0 removes it, negative
// restores Skate's own. Returns 1 when queued. Export: sv_set_air_limit.
using SvSetAirLimitFn = std::uint32_t(__cdecl*)(void*, float);

// sv_set_lip_rule(rt, enabled): the SkateV ramp-lip rule (a rolling board
// stalls on a lip only riding up into it slowly). 0 restores retail grind
// admission. Returns 1 when queued. Export: sv_set_lip_rule.
using SvSetLipRuleFn = std::uint32_t(__cdecl*)(void*, std::uint32_t);

// sv_set_camera_type(rt, type): Skate 3's camera, 0 Low, 1 High (default). Export: sv_set_camera_type.
using SvSetCameraTypeFn = std::uint32_t(__cdecl*)(void*, std::uint32_t);
// sv_set_difficulty(rt, index): Skate 3's physics_mode, 0 easy, 1 normal,
// 2 hardcore, 3 motorized; live and for later sessions. Returns 1 when queued.
// Export: sv_set_difficulty.
using SvSetDifficultyFn = std::uint32_t(__cdecl*)(void*, std::uint32_t);

// sv_set_verbose_log(rt, enabled): nonzero writes the periodic lines (perf,
// skater trace, collision streaming) to the runtime log. Returns 1.
// Export: sv_set_verbose_log.
using SvSetVerboseLogFn = std::uint32_t(__cdecl*)(void*, std::uint32_t);

// sv_play_line(rt, path_utf8): play a showcase line file (`line.rs`) while
// Skate is active: Skate's teleport to the line's start, then its input one
// frame per Skate tick; any player input stops it. Returns 1 when queued.
// Export: sv_play_line.
using SvPlayLineFn = std::uint32_t(__cdecl*)(void*, const char*);

static_assert(sizeof(SvLifecycleTrack) == 24);
static_assert(offsetof(SvLifecycleTrack, position) == 8);
static_assert(offsetof(SvLifecycleTrack, heading_degrees) == 20);
static_assert(sizeof(SvLifecycleState) == 56);
static_assert(offsetof(SvLifecycleState, skate_state) == 20);
static_assert(offsetof(SvLifecycleState, last_prepare_ms) == 36);
static_assert(offsetof(SvLifecycleState, collision_center) == 44);
// ---- end lifecycle ----

// ---- hud (ABI 8) ----
// Skate 3's original APT trickdisplay HUD, run by the runtime on the Skate
// worker; the host only rasterises the published draw list (hud_overlay.cpp).
// Mirrors rust/skatev-runtime/src/hud.rs (hud_abi_layout_matches_header).
struct SvHudViewport {
    std::uint32_t size;
    std::uint32_t width, height; // backbuffer pixels
    float safe_zone;             // GET_SAFE_ZONE_SIZE (1.0 = no inset)
    float max_aspect;            // widest HUD region; 0 = default 2.4, < 0 uncapped
    std::uint32_t flags;         // bit 0: HUD enabled
    float hud_area[4];           // GTA's HUD area, normalized x0, y0, x1, y1 (script gfx alignment:
                                 // safe zone and ultrawide placement); all 0 = use the safe-zone model
};
struct SvHudTexture {
    std::uint32_t size;
    std::uint32_t width, height; // RGBA8, straight alpha, sRGB colour
    std::uint32_t generation;
};
struct SvHudVertex { float x, y, u, v; }; // backbuffer pixels, origin top-left
// clamp(texel * multiply + add, 0, 1), straight-alpha blended into a
// transparent sRGB target, then composited premultiplied over the frame.
struct SvHudDraw {
    std::uint32_t texture;
    std::uint32_t first_vertex;
    std::uint32_t vertex_count;
    std::uint32_t flags;
    float multiply[4];
    float add[4];
};
struct SvHudFrame {
    std::uint32_t size;
    std::uint32_t visible;
    std::uint64_t serial;
    std::uint32_t width, height; // backbuffer size the vertices were laid out for
    std::uint32_t draw_count, vertex_count;
    std::uint32_t texture_count, texture_generation;
    float scale;       // pixels per movie unit
    float edge_offset; // Screen_EdgeOffset, movie units
    float region[4];   // HUD region, pixels
};
using SvSetHudViewportFn = std::uint32_t(__cdecl*)(void*, const SvHudViewport*);
using SvGetHudTextureFn = std::uint32_t(__cdecl*)(const void*, std::uint32_t, SvHudTexture*, void*, std::uint32_t);
using SvGetHudFrameFn = std::uint32_t(__cdecl*)(const void*, SvHudFrame*, SvHudDraw*, std::uint32_t, SvHudVertex*,
                                                std::uint32_t);
static_assert(sizeof(SvHudViewport) == 40);
static_assert(offsetof(SvHudViewport, hud_area) == 24);
static_assert(offsetof(SvHudViewport, safe_zone) == 12);
static_assert(offsetof(SvHudViewport, flags) == 20);
static_assert(sizeof(SvHudTexture) == 16);
static_assert(sizeof(SvHudVertex) == 16);
static_assert(sizeof(SvHudDraw) == 48);
static_assert(offsetof(SvHudDraw, multiply) == 16);
static_assert(offsetof(SvHudDraw, add) == 32);
static_assert(sizeof(SvHudFrame) == 64);
static_assert(offsetof(SvHudFrame, serial) == 8);
static_assert(offsetof(SvHudFrame, width) == 16);
static_assert(offsetof(SvHudFrame, draw_count) == 24);
static_assert(offsetof(SvHudFrame, texture_generation) == 36);
static_assert(offsetof(SvHudFrame, scale) == 40);
static_assert(offsetof(SvHudFrame, region) == 48);
// Optional Hall of Meat x-ray (sv_get_xray): the broken-bone skeleton pieces
// Skate 3's HOMSkaterPresEntity draws during a bail, skinned and shaded by
// the runtime (rust/skatev-runtime/src/xray.rs). A triangle list in GTA world
// space; colours are the retail shader's output (display encoded, straight
// alpha). The host projects them with GTA's rendered camera over the frame.
struct SvXrayVertex {
    SvVec3 position;
    float rgba[4];
};
struct SvXrayFrame {
    std::uint32_t size;
    std::uint32_t vertex_count; // published this tick (0: no x-ray)
    std::uint64_t tick;
};
// Fills `frame`; copies the vertices and returns their count only when
// `capacity` holds all of them (else 0: grow to frame.vertex_count).
using SvGetXrayFn = std::uint32_t(__cdecl*)(const void*, SvXrayFrame*, SvXrayVertex*, std::uint32_t);
static_assert(sizeof(SvXrayVertex) == 28);
static_assert(sizeof(SvXrayFrame) == 16);
static_assert(offsetof(SvXrayFrame, tick) == 8);
// ---- end hud ----

// ---- player records (optional exports) ----
// Best Hall of Meat bails and banked lines, overall and per GTA zone, kept
// by the runtime (rust/skatev-runtime/src/records.rs) in a JSON book.
struct SvRecordState {
    std::uint32_t size;
    std::uint32_t sequence; // increments with every score that placed (0: none)
    std::uint32_t category; // 1 Hall of Meat bail, 2 banked line
    std::uint32_t score;
    std::uint32_t rank, spot_rank; // 1-based; 0: outside that top ten
    std::uint32_t hom_best, hom_spot_best, line_best, line_spot_best;
};
struct SvRecordEntry {
    std::uint32_t score;
    std::uint32_t reserved;
    std::uint64_t time; // unix seconds
    char character_utf8[24];
    char spot_name_utf8[48];
};
// sv_records_open(rt, path_utf8): opens or starts the book.
using SvRecordsOpenFn = std::uint32_t(__cdecl*)(void*, const char*);
// sv_records_set_context(rt, character, zone code, zone display name).
using SvRecordsSetContextFn = std::uint32_t(__cdecl*)(void*, const char*, const char*, const char*);
using SvRecordsGetStateFn = std::uint32_t(__cdecl*)(const void*, SvRecordState*);
// sv_records_get_top(rt, category, spot_only, out, capacity) -> count, best first.
using SvRecordsGetTopFn = std::uint32_t(__cdecl*)(const void*, std::uint32_t, std::uint32_t, SvRecordEntry*, std::uint32_t);
// sv_records_label(rt, language id, buf, capacity) -> length (Skate 3's own text).
using SvRecordsLabelFn = std::uint32_t(__cdecl*)(const void*, const char*, char*, std::uint32_t);
static_assert(sizeof(SvRecordState) == 40);
static_assert(sizeof(SvRecordEntry) == 88);
static_assert(offsetof(SvRecordEntry, character_utf8) == 16);

// ---- audio (ABI 8) ----
// Skate 3 gameplay audio, DRY. Contract between audio-skate (runtime, this
// block) and audio-gta (host output). Stable since 2026-10-01; extend only by
// appending struct fields (callers pass `size`) or adding exports.
//
// What the runtime produces. Skate 3's AEMS sound scripts run from live Skate
// state (docs/AEMS.md); every voice goes through EA Audio Core's dry chain
// SndPlayer1 -> Rechannel -> Resample -> HighPassIir2 -> LowPassIir2 -> Gain
// and stops there: no Pan2D1, sends, buses or reverb. GTA's audio engine owns
// panning, distance attenuation, occlusion, reverb, categories and pause.
// Voices are summed per emitter (fixed ids below) into one ring buffer per
// emitter: MONO float32, 48,000 Hz, nominal full scale +-1.0 (not clipped).
// The runtime renders on its own thread in 256-frame blocks paced by the
// wall clock, keeping about SV_AUDIO_LATENCY_FRAMES queued per emitter; the
// oldest frames are dropped when nobody pulls. No call blocks.
//
// Host usage (audio-gta):
//  1. Once after sv_create: sv_audio_configure(rt, &cfg). cache_dir_utf8 is
//     the prepared sample cache (SkateVLegacy.ini `AudioCache=`; made by
//     `python tools/prepare-skate-audio.py --skate-data <Skate_3 dir> --out
//     <cache>`). Returns 1 when accepted (loading continues in the
//     background, see sv_audio_get_status), 0 when audio is unavailable
//     (null/invalid arguments, sample_rate other than 0/48000, no cache).
//     Calling it again reconfigures (the old engine stops).
//  2. Every script frame: sv_audio_set_paused(rt, paused) with 1 while the
//     game is paused (pause menu, phone/menus that freeze the game); the
//     runtime then stops advancing Skate audio and produces nothing. While
//     skate mode is inactive the runtime ends its voices by itself (tails
//     play out, then silence) - no host call needed.
//  3. From the host's audio consumer (any ONE thread):
//     n = sv_audio_emitters(rt, infos, cap) gives the emitter count and
//     state; for each id < n, sv_audio_pull(rt, id, buf, frames, &info)
//     copies up to `frames` queued mono frames into buf and returns how many
//     were copied; buf[n..frames) is zero-filled, so a short pull is an
//     underrun, not garbage. Pull at the real-time rate (e.g. 1024 frames
//     every ~21 ms); pulling faster than real time just returns fewer
//     frames. info (may be null) is the emitter at the newest copied frame.
//  4. Place the emitter's stream at info.position (GTA world space, metres),
//     moving with info.velocity; apply info.gain as linear volume (the PCM
//     is not pre-scaled by it). SV_AUDIO_EMITTER_LISTENER emitters are
//     listener-relative beds in Skate 3 (the camera's wind): play them
//     non-positional / at the camera. When SV_AUDIO_EMITTER_SOUNDING is
//     clear and nothing is queued the stream may be stopped and restarted
//     later (audio continues from the next pull).
//
// The placeholder rolling-grain XAudio2 player (host/src/skate_audio.cpp)
// is RETIRED by this contract (audio-skate decision, docs/DECISIONS.md
// "Dry Skate voices from the runtime"): the normal path must not run it.
// It may stay compiled only behind a developer ini option that defaults off.

#define SV_AUDIO_SAMPLE_RATE 48000u
#define SV_AUDIO_LATENCY_FRAMES 4096u

enum SvAudioEmitterId : std::uint32_t {
    SV_AUDIO_EMITTER_BOARD = 0, // deck centre: rolling, rattle, skids, squeaks, seams, slides, grinds, flips, pops/lands
    SV_AUDIO_EMITTER_BODY = 1,  // skater pelvis: cloth, footsteps, foot drag, body slide, bails/body impacts
    SV_AUDIO_EMITTER_SPEED = 2, // sense-of-speed wind/rattle beds (listener-relative)
    SV_AUDIO_EMITTER_COUNT = 3,
};

enum SvAudioEmitterFlags : std::uint32_t {
    SV_AUDIO_EMITTER_ACTIVE = 1u << 0,   // a Skate session is active and audio is advancing
    SV_AUDIO_EMITTER_SOUNDING = 1u << 1, // at least one voice is alive on this emitter
    SV_AUDIO_EMITTER_LISTENER = 1u << 2, // listener-relative bed: play at the camera / non-positional
};

enum SvAudioState : std::uint32_t {
    SV_AUDIO_OFF = 0,     // not configured
    SV_AUDIO_LOADING = 1, // cache/banks loading in the background
    SV_AUDIO_READY = 2,   // producing (or idle while skate mode is inactive)
    SV_AUDIO_ERROR = 3,   // see message; pulls return 0
};

struct SvAudioConfig {
    std::uint32_t size;          // sizeof(SvAudioConfig)
    std::uint32_t sample_rate;   // 0 or 48000
    const char* cache_dir_utf8;  // prepared sample cache directory
    float master_gain;           // folded into every emitter's info.gain; <= 0 means 1.0
    std::uint32_t flags;         // reserved, 0
};

struct SvAudioEmitter {
    std::uint32_t size;          // set by the caller to sizeof(SvAudioEmitter)
    std::uint32_t id;            // SvAudioEmitterId
    std::uint32_t flags;         // SvAudioEmitterFlags
    std::uint32_t channels;      // 1
    std::uint32_t sample_rate;   // 48000
    std::uint32_t queued_frames; // frames ready to pull (after this pull, for sv_audio_pull)
    SvVec3 position;             // GTA world space
    SvVec3 velocity;             // GTA world space, m/s
    float gain;                  // linear volume to apply
    std::uint32_t voices;        // live voices on this emitter
    std::uint64_t frame;         // stream position of the newest copied frame (frames since configure)
};

struct SvAudioStatus {
    std::uint32_t size;          // set by the caller to sizeof(SvAudioStatus)
    std::uint32_t state;         // SvAudioState
    std::uint32_t voices;        // live voices, all emitters
    std::uint32_t underruns;     // short pulls since configure
    std::uint64_t frames;        // frames rendered since configure
    char message_utf8[96];
};

using SvAudioConfigureFn = std::uint32_t(__cdecl*)(void*, const SvAudioConfig*);
using SvAudioSetPausedFn = std::uint32_t(__cdecl*)(void*, std::uint32_t);
using SvAudioEmittersFn = std::uint32_t(__cdecl*)(const void*, SvAudioEmitter*, std::uint32_t);
using SvAudioPullFn = std::uint32_t(__cdecl*)(void*, std::uint32_t, float*, std::uint32_t, SvAudioEmitter*);
using SvAudioGetStatusFn = std::uint32_t(__cdecl*)(const void*, SvAudioStatus*);
// Exports: sv_audio_configure, sv_audio_set_paused, sv_audio_emitters,
// sv_audio_pull, sv_audio_get_status (all optional; resolve by name).

static_assert(sizeof(SvAudioConfig) == 24);
static_assert(offsetof(SvAudioConfig, cache_dir_utf8) == 8);
static_assert(offsetof(SvAudioConfig, master_gain) == 16);
static_assert(sizeof(SvAudioEmitter) == 64);
static_assert(offsetof(SvAudioEmitter, position) == 24);
static_assert(offsetof(SvAudioEmitter, gain) == 48);
static_assert(offsetof(SvAudioEmitter, frame) == 56);
static_assert(sizeof(SvAudioStatus) == 120);
static_assert(offsetof(SvAudioStatus, frames) == 16);
static_assert(offsetof(SvAudioStatus, message_utf8) == 24);
// ---- end audio ----

// ---- world-stability (ABI 8) ----
// Exact owned model bounds for moving props.
// Templates sit beside WorldCache in <stem>.prop-models/<model hash>.svwc.
// Axes are GTA local-to-world columns and retain entity scale. fallback.tag
// identifies the entity; flag 1 permits a box only if no template is available.
struct SvDynamicBody {
    std::uint32_t size;
    std::uint32_t model_hash;
    SvVec3 position, right, forward, up;
    SvBox fallback;
    std::uint32_t flags;
    // ---- contact exchange ----
    SvVec3 linear_velocity;  // GET_ENTITY_VELOCITY, world m/s
    SvVec3 angular_velocity; // GET_ENTITY_ROTATION_VELOCITY, world rad/s
    // ---- grab line ----
    SvVec3 grab_point;       // vehicle bumper_r bone, GTA world (valid iff grab_flags & 1)
    std::uint32_t grab_flags;
};
static_assert(sizeof(SvDynamicBody) == 144);
using SvSetDynamicBodiesFn = std::uint32_t(__cdecl*)(void*, const SvDynamicBody*, std::uint32_t);

// Contact exchange: impulse the board/skater applied to a host entity in one
// Skate solve. GTA world space, newton-seconds. mass_kg is the mass the solve
// used (0: unknown, the entity moved kinematically). Drained once.
struct SvDynamicImpulse {
    std::uint32_t size;
    std::uint32_t tag;   // SvBox::tag of the body
    std::uint64_t tick;  // Skate tick
    SvVec3 point;
    SvVec3 impulse;
    float mass_kg;
    std::uint32_t reserved;
    // v2: angular velocity change (GTA world, rad/s) the impulse gives the
    // rigid body the solve used (a ped: its posed ragdoll compound, about its
    // centre of mass). Zero for other bodies. v1 records end before it.
    SvVec3 angular_velocity_change;
    std::uint32_t reserved2;
};
static_assert(sizeof(SvDynamicImpulse) == 64);
static_assert(offsetof(SvDynamicImpulse, angular_velocity_change) == 48);
using SvGetDynamicImpulsesFn = std::uint32_t(__cdecl*)(void*, SvDynamicImpulse*, std::uint32_t);
// ---- end world-stability ----

// Optional ABI 8 ped hitboxes (sv_set_ped_parts): one posed part of a GTA
// ped's ragdoll compound, GTA world space.
struct SvPedPart {
    std::uint32_t size;
    std::uint32_t tag;       // the ped's SvBox::tag (kind 1)
    std::uint32_t kind;      // 1 capsule (along forward), 3 box
    std::uint32_t component; // ragdoll component (fragment child index)
    SvVec3 right, forward, up, centre;
    SvVec3 half_extents;     // capsule: (radius, half segment, radius); box: outer half extents
    float radius;            // capsule radius; box edge radius (bound margin)
    float mass_kg;           // authored ragdoll mass of the part
    SvVec3 linear_velocity, angular_velocity; // the ped's
};
static_assert(sizeof(SvPedPart) == 108);
using SvSetPedPartsFn = std::uint32_t(__cdecl*)(void*, const SvPedPart*, std::uint32_t);

// Optional ABI 8 native board pose. GTA world transforms in RAGE row layout.
// Native bone/tag order: SKATEBOARD_ROOT, TRUCK_FRONT, TRUCK_BACK,
// LEFT_WHEELFRONT, LEFT_WHEELBACK, RIGHT_WHEELFRONT, RIGHT_WHEELBACK.
struct SvBoardPose {
    std::uint32_t size, bone_count;
    std::uint64_t tick;
    float entity[16]; // root world * authored root inverse bind; native entity/culling transform
    float world[7][16];
};
static_assert(sizeof(SvBoardPose) == 528);
static_assert(offsetof(SvBoardPose, entity) == 16);
static_assert(offsetof(SvBoardPose, world) == 80);
using SvGetBoardPoseFn = std::uint32_t(__cdecl*)(const void*, SvBoardPose*);

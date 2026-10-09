// SkateVLegacy.asi: GTA V Legacy Story Mode host for the adopted Skate runtime.
// GTA side only: input acquisition, control suppression, camera, presentation
// and HUD drawing. Skating, tricks and scoring are all Skate's.
#include "stall_sampler.h"
#include <windows.h>
#include <xinput.h>
#include <main.h>
#include <algorithm>
#include <cctype>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <string>
#include <vector>
#include <filesystem>
#include <share.h>
#include "board_native.h"
#include "ped_collision_native.h"
#include "dynamic_world.h"
#include "gta_audio.h"
#include "game_probe.h"
#include "host_util.h"
#include "physics_level.h"
#include "hud_overlay.h"
#include "records_ui.h"
#include "lifecycle.h"
#include "menu.h"
#include "map_states.h"
#include "pack_loader.h"
#include "pose_hook.h"
#include "pose_math.h"
#include "live_clip.h"
#include "natives.h"
#include "ped_speech.h"
#include "runtime_bridge.h"
#include "version_guard.h"

namespace {
using gta::Call;

RuntimeBridge g_runtime;
std::atomic<bool> g_toggleRequested{false}; // menu: Skate on/off
FILE* g_log = nullptr;
std::wstring g_gameDir;
std::wstring g_logDir; // %LOCALAPPDATA%\SkateV: the game folder is usually not writable

// Every default lives here; LoadConfig (kKeys) and the menu (MenuPages) read it.
struct Config {
    std::string dataRoot;
    std::string worldCache;
    float pedZOffset = 1.0f;  // ped origin above Skate's ground contact
    float fovScale = 1.0f;
    bool drawBoard = true;    // DrawBoard: the native board object (and its mesh fallback)
    bool showDebug = false;   // ShowDebug=1: old debug text HUD (the original Skate HUD is hud_overlay)
    bool verboseLog = false;  // VerboseLog=1: periodic runtime log lines (perf, skater trace, collision streaming)
    bool loadRuntime = true;  // Runtime=0: ASI + frame-timing log only (A/B testing)
    bool posePed = true;      // PosePed: Skate's pose on the real ped (ped vtable slot 65 wrapper)
    bool liveClip = false;    // LiveClip=1: live_clip.h (Skate's pose also through GTA's anim, the owned clip)
    bool stallSampler = false; // StallSampler=1: stall_sampler.h (stack samples of stalled script frames)
    // Shooting on the board, GTA drive-by layout: GunButton (XInput mask,
    // default Back, on release without LB) takes a gun out or puts it away.
    // Gun out: hold AimButton (LB) to aim, FireButton (RB) fires, right stick
    // looks, d-pad left/right changes weapon; aiming leaves Skate the left
    // stick and A. Gun away: LB is Skate's; d-pad left/right steps the radio.
    bool boardAim = true;
    // Gun out: GTA animates the ped (its own aim, weapon and wheel tasks) and
    // Skate keeps the lower half. GunIK: 0 none, 1 arm IK (left hand onto the
    // grip), 2 also torso and head IK. GunProbe: log GTA's task tree and
    // aim/fire state while the gun is out.
    int gunIk = 1;
    float aimTwist = 80.0f; // AimTwist: most the aiming chest turns from the stance (degrees); shots still go to the reticle
    bool gunProbe = false;
    std::uint16_t aimButton = 0x0100, fireButton = 0x0200, gunButton = 0x0020;
    bool drawSkater = true;        // DrawSkater: the board mesh while the native board is unavailable
    bool hallOfMeat = true;        // HallOfMeat=1: Skate 3's bail scoring, broken-bone slow-mo, HoM HUD
    bool hallOfMeatMetrics = false; // HallOfMeatMetrics=1: every metric panel, not only those over threshold
    bool goofy = false;            // Stance=Goofy: Skate profile stance (default Regular: A is the normal push)
    std::string line;              // Line: showcase line files, ';'-separated (local; see rust line.rs)
    float bailTimeLimit = 0.0f;    // BailTimeLimit: seconds before a moving bail auto-resets; 0 none, -1 Skate's own
    float airTimeLimit = 30.0f;    // AirTimeLimit: seconds airborne before Skate returns you to a checkpoint; 0 none, -1 Skate's own
    float skitchStandoff = 0.3f;   // SkitchStandoff: metres behind a car's rear the skitch grab line sits (0 on the bodywork)
    bool lipRule = true;           // LipRule=1: stall on a ramp lip only riding up slowly; 0 retail grind admission
    int difficulty = 0;            // Difficulty=Easy|Normal|Hardcore|Motorized: Skate 3's physics_mode (index)
    int skaterTriangles = 3000;
    bool dynamicWorld = true;
    float dynamicRadius = 35.0f;
    bool pedHitboxes = true; // peds as their own ragdoll parts (PedHitboxes=0: the old box)
    // Ped launch (not physical): peds never slow the rider; the first touch
    // launches them at twice the closing speed, scaled, with lift.
    bool pedLaunch = true;
    float pedLaunchScale = 1.0f;
    float pedLaunchLift = 0.3f;
    float pedLaunchSpin = 1.0f; // PedLaunchSpin: gain on the launch's tumble about the struck point (0 none)
    // Voices (ped_speech.h): the skater's pain grunt on each hard hit in a
    // bail (BailPain: GTA damage reason for
    // PLAY_PAIN, sent with 0 damage like the post-fall grunt that is heard;
    // -1 off), a line when they are back up and one when a car only just
    // misses them; knocked-over peds the same. Speech lists are
    // comma-separated contexts, one picked at random among those the
    // character's voice has (Franklin, Michael and Trevor all have these).
    int bailPain = 13;
    float bailHitSpeed = 3.0f;    // BailHitSpeed: body velocity change (m/s) that counts as a hit
    float bailScreamSpeed = 9.0f; // BailScreamSpeed: falling this fast (m/s, ~4 m drop) screams
    std::string getUpSpeech = "GET_UP_FROM_FALL,CRASH_GENERIC,GENERIC_FRUSTRATED_HIGH,GENERIC_FRUSTRATED_MED,"
                              "GENERIC_CURSE_HIGH,GENERIC_SHOCKED_HIGH";
    std::string nearMissSpeech = "NEAR_MISS_VEHICLE"; // NearMissSpeech: a car passing within 1 m at 7 m/s
    int pedPain = 0;
    std::string pedGetUpSpeech = "GENERIC_CURSE_HIGH,GENERIC_CURSE_MED,GENERIC_INSULT_HIGH,GENERIC_INSULT_MED";
    // Skate impulses dent GTA vehicles (see DynamicWorld::SetVehicleDamage).
    bool vehicleDamage = true;
    // Radius is in SET_VEHICLE_DAMAGE's own units (scripts use tens to
    // hundreds; 0.5 dents nothing).
    float vehicleDamageScale = 0.5f, vehicleDamageMin = 200.0f, vehicleDamageRadius = 150.0f;
    // GTA delegation: swim when the skater's chest is under water (water this
    // deep over the feet; GTA's ped starts swimming at about the same depth),
    // climb over a barrier on X, X off the board.
    bool delegation = true;
    float swimDepth = 1.0f;
    bool gtaCharacter = true;  // your GTA character wears Skate's pose (else Skate's skater)
    std::string pedCache;      // local ped cache from tools/export-peds.ps1
    int characterTriangles = 8000;
    // RetailQuirk::BackwardsMan (Skate 3 backwards-man / speed glitch assist).
    bool backwardsMan = true;
    unsigned backwardsManChord = 0x00C0; // L3 + R3
    unsigned putAwayHoldMs = 600; // Y held this long while skating puts the board away (0 = off)
    bool backwardsManBackward = true;
    unsigned backwardsManRemountDelay = 6;
    // Seamless lifecycle (ledger P0-1): controller board action, guards,
    // background preparation. The menu's "Skate on / off" is the manual activation.
    lifecycle::Settings life;
};

std::wstring LogDir() {
    wchar_t buf[MAX_PATH]{};
    const DWORD n = GetEnvironmentVariableW(L"LOCALAPPDATA", buf, MAX_PATH);
    if (n == 0 || n >= MAX_PATH) return g_gameDir;
    std::wstring dir = std::wstring(buf) + L"\\SkateV\\";
    CreateDirectoryW(dir.c_str(), nullptr);
    return dir;
}

std::wstring GameDir() {
    wchar_t path[MAX_PATH]{};
    const DWORD n = GetModuleFileNameW(nullptr, path, MAX_PATH);
    if (n == 0 || n >= MAX_PATH) return L".\\";
    std::wstring s(path, n);
    return s.substr(0, s.find_last_of(L'\\') + 1);
}

void Log(const char* message) {
    OutputDebugStringA(message);
    OutputDebugStringA("\n");
    if (!g_log) g_log = _wfsopen((g_logDir + L"SkateVLegacy.log").c_str(), L"a", _SH_DENYNO);
    if (g_log) {
        SYSTEMTIME t{};
        GetLocalTime(&t);
        std::fprintf(g_log, "%02u:%02u:%02u.%03u %s\n", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds, message);
        std::fflush(g_log);
    }
}

template <class... A>
void Logf(const char* fmt, A... a) {
    util::Logf(Log, fmt, a...);
}

// ---- settings: one table of [SkateV] keys --------------------------------------
// INI text -> Config field, used by LoadConfig and by the menu items (Bound).
void Parse(const std::string& v, bool& f) { f = std::atof(v.c_str()) != 0.0; }
void Parse(const std::string& v, float& f) { f = static_cast<float>(std::atof(v.c_str())); }
void Parse(const std::string& v, int& f) { f = static_cast<int>(std::atof(v.c_str())); }
void Parse(const std::string& v, unsigned& f) { f = static_cast<unsigned>(std::strtoul(v.c_str(), nullptr, 0)); } // hex ok
void Parse(const std::string& v, std::uint16_t& f) { f = static_cast<std::uint16_t>(std::strtoul(v.c_str(), nullptr, 0)); }
void Parse(const std::string& v, std::string& f) { f = v; }

struct Key {
    const char* name;
    void (*store)(Config&, const std::string&);
};
template <auto Field>
void Store(Config& c, const std::string& v) { Parse(v, c.*Field); }
// "0" turns a speech list off; anything else is the list.
std::string Speech(const std::string& v) { return v == "0" ? "" : v; }

const char* const kDifficulties[] = {"Easy", "Normal", "Hardcore", "Motorized"};

const Key kKeys[] = {
    {"DataRoot", Store<&Config::dataRoot>},
    {"WorldCache", Store<&Config::worldCache>},
    {"PedZOffset", Store<&Config::pedZOffset>},
    {"CameraFovScale", Store<&Config::fovScale>},
    {"DrawBoard", Store<&Config::drawBoard>},
    {"ShowDebug", Store<&Config::showDebug>},
    {"VerboseLog", Store<&Config::verboseLog>},
    {"Runtime", Store<&Config::loadRuntime>},
    {"PosePed", Store<&Config::posePed>},
    {"GtaDelegation", Store<&Config::delegation>},
    {"SwimDepth", Store<&Config::swimDepth>},
    {"LiveClip", Store<&Config::liveClip>},
    {"StallSampler", Store<&Config::stallSampler>},
    {"BoardAim", Store<&Config::boardAim>},
    {"GunIK", Store<&Config::gunIk>},
    {"GunProbe", Store<&Config::gunProbe>},
    {"AimTwist", Store<&Config::aimTwist>},
    {"AimButton", Store<&Config::aimButton>},
    {"FireButton", Store<&Config::fireButton>},
    {"GunButton", Store<&Config::gunButton>},
    {"DrawSkater", Store<&Config::drawSkater>},
    {"HallOfMeat", Store<&Config::hallOfMeat>},
    {"HallOfMeatMetrics", Store<&Config::hallOfMeatMetrics>},
    {"Stance", [](Config& c, const std::string& v) { c.goofy = menu::SameText(v, "goofy") || v == "1"; }},
    {"BailTimeLimit", Store<&Config::bailTimeLimit>},
    {"AirTimeLimit", Store<&Config::airTimeLimit>},
    {"LipRule", Store<&Config::lipRule>},
    {"Difficulty", [](Config& c, const std::string& v) {
         c.difficulty = 0;
         for (int i = 0; i < 4; ++i)
             if (menu::SameText(v, kDifficulties[i])) c.difficulty = i;
     }},
    {"SkitchStandoff", Store<&Config::skitchStandoff>},
    {"SkaterTriangles", Store<&Config::skaterTriangles>},
    {"DynamicWorld", Store<&Config::dynamicWorld>},
    {"DynamicRadius", Store<&Config::dynamicRadius>},
    {"PedHitboxes", Store<&Config::pedHitboxes>},
    {"PedLaunch", Store<&Config::pedLaunch>},
    {"PedLaunchScale", Store<&Config::pedLaunchScale>},
    {"PedLaunchLift", Store<&Config::pedLaunchLift>},
    {"PedLaunchSpin", Store<&Config::pedLaunchSpin>},
    {"BailPain", Store<&Config::bailPain>},
    {"BailHitSpeed", Store<&Config::bailHitSpeed>},
    {"BailScreamSpeed", Store<&Config::bailScreamSpeed>},
    {"GetUpSpeech", [](Config& c, const std::string& v) { c.getUpSpeech = Speech(v); }},
    {"NearMissSpeech", [](Config& c, const std::string& v) { c.nearMissSpeech = Speech(v); }},
    {"PedPain", Store<&Config::pedPain>},
    {"PedGetUpSpeech", [](Config& c, const std::string& v) { c.pedGetUpSpeech = Speech(v); }},
    {"VehicleDamage", Store<&Config::vehicleDamage>},
    {"VehicleDamageScale", Store<&Config::vehicleDamageScale>},
    {"VehicleDamageMin", Store<&Config::vehicleDamageMin>},
    {"VehicleDamageRadius", Store<&Config::vehicleDamageRadius>},
    {"GtaCharacter", Store<&Config::gtaCharacter>},
    {"PedCache", Store<&Config::pedCache>},
    {"CharacterTriangles", Store<&Config::characterTriangles>},
    {"BackwardsMan", Store<&Config::backwardsMan>},
    {"BackwardsManChord", Store<&Config::backwardsManChord>},
    {"Line", Store<&Config::line>},
    {"BackwardsManDirection", [](Config& c, const std::string& v) { c.backwardsManBackward = v != "Forward"; }},
    {"BackwardsManRemountDelay", Store<&Config::backwardsManRemountDelay>},
    {"MenuButton", [](Config& c, const std::string& v) { Parse(v, c.life.menuButton); }},
    {"PutAwayHoldMs", Store<&Config::putAwayHoldMs>},
    {"BoardActionInMissions", [](Config& c, const std::string& v) { Parse(v, c.life.inMissions); }},
    {"BackgroundPrepare", [](Config& c, const std::string& v) { Parse(v, c.life.backgroundPrepare); }},
};

const Key* FindKey(const std::string& name) {
    for (const Key& k : kKeys)
        if (name == k.name) return &k;
    return nullptr;
}

Config LoadConfig() {
    const std::wstring ini = g_gameDir + L"SkateVLegacy.ini";
    if (GetFileAttributesW(ini.c_str()) == INVALID_FILE_ATTRIBUTES) {
        WritePrivateProfileStringW(L"SkateV", L"DataRoot", L"", ini.c_str());
        WritePrivateProfileStringW(L"SkateV", L"WorldCache", L"", ini.c_str());
        Log("SkateV Legacy: wrote SkateVLegacy.ini template; set DataRoot and WorldCache");
    }
    Config c; // an unset key keeps its default
    for (const Key& k : kKeys) {
        const std::string v = util::IniString(ini, util::Wide(k.name));
        if (!v.empty()) k.store(c, v);
    }
    return c;
}

Config g_config;

// '/' opens the settings menu (menu.h). Everything else that used a hotkey is a
// menu item; the items that act on a frame of the main loop set these flags.
std::atomic<bool> g_menuToggle{false};
std::atomic<bool> g_quirkRequested{false};
std::atomic<bool> g_soundListToggle{false};
std::atomic<bool> g_lineRequested{false};
std::atomic<bool> g_lineNext{false};  // next line in Line='s ';' list

void Keyboard(DWORD key, WORD, BYTE, BOOL, BOOL, BOOL wasDownBefore, BOOL isUpNow) {
    if (key == VK_OEM_2 && !wasDownBefore && !isUpNow) g_menuToggle.store(true);
}

// ---- GTA presentation ------------------------------------------------------
void Text(const char* s, float x, float y, float scale, int r, int g, int b, bool centre) {
    gta::Text(s, x, y, scale, r, g, b, 255, centre ? gta::Align::Centre : gta::Align::Left);
}

// The board while the native board object is unavailable: the runtime's board
// triangles (Ped presentation: the mesh is the board alone), flat, both sides.
void DrawBoardMesh() {
    static std::vector<SvColorTri> mesh(20000);
    const std::uint32_t n = g_runtime.SkaterMesh(mesh.data(), static_cast<std::uint32_t>(mesh.size()));
    for (std::uint32_t i = 0; i < n; ++i) {
        const SvColorTri& t = mesh[i];
        if (t.flags & kSvTriCutout) continue;
        const int r = t.rgba[0], g = t.rgba[1], b = t.rgba[2];
        Call<void>(gta::DRAW_POLY, t.a.x, t.a.y, t.a.z, t.b.x, t.b.y, t.b.z, t.c.x, t.c.y, t.c.z, r, g, b, 255);
        Call<void>(gta::DRAW_POLY, t.c.x, t.c.y, t.c.z, t.b.x, t.b.y, t.b.z, t.a.x, t.a.y, t.a.z, r, g, b, 255);
    }
}

std::string PrettyTrick(const char* raw) {
    // Formatting only: the label itself is Skate's scorer output.
    std::string s(raw);
    if (s.rfind("ID_TRICK_", 0) == 0) s = s.substr(9);
    for (char& ch : s) if (ch == '_') ch = ' ';
    return s;
}

void DrawHud(const SvOutput& o, const SvScoreState& s, bool debug) {
    char line[160];
    if (s.trick_utf8[0]) {
        const std::string trick = PrettyTrick(s.trick_utf8);
        Text(trick.c_str(), 0.5f, 0.08f, 0.6f, 255, 255, 255, true);
    }
    if (s.sequence_active || s.sequence_score > 0.0f) {
        sprintf_s(line, "%.0f  x%.2f", s.sequence_score, s.multiplier);
        Text(line, 0.5f, 0.13f, 0.5f, 255, 210, 60, true);
    }
    sprintf_s(line, "LINE %.0f   TOTAL %.0f", s.line_score, s.completed_lines);
    Text(line, 0.5f, 0.02f, 0.4f, 200, 200, 200, true);
    // Recent tricks, newest first (Skate's scorer labels).
    const std::string history = g_runtime.TrickHistory();
    std::vector<std::string> items;
    for (std::size_t at = 0; at < history.size();) {
        const std::size_t end = history.find('\n', at);
        items.push_back(history.substr(at, end == std::string::npos ? std::string::npos : end - at));
        if (end == std::string::npos) break;
        at = end + 1;
    }
    float y = 0.30f;
    for (auto it = items.rbegin(); it != items.rend(); ++it) {
        Text(PrettyTrick(it->c_str()).c_str(), 0.98f, y, 0.32f, 230, 230, 230, false);
        y += 0.03f;
    }
    if (debug) {
        const float speed = std::sqrt(o.velocity.x * o.velocity.x + o.velocity.y * o.velocity.y + o.velocity.z * o.velocity.z);
        sprintf_s(line, "SkateV %s  %.1f m/s  tick %llu%s%s%s", o.state_utf8, speed,
                  static_cast<unsigned long long>(o.tick), (s.flags & SV_SCORE_SWITCH) ? "  switch" : "",
                  (s.flags & SV_SCORE_FAKIE) ? "  fakie" : "", (s.flags & SV_SCORE_CLEAN) ? "  clean" : "");
        Text(line, 0.01f, 0.95f, 0.3f, 180, 255, 180, false);
    }
}

SvPad ReadPadRaw() {
    // Polled every frame in and out of skate mode (board action). XInput is
    // slow on empty slots, so the last connected slot is read each frame and
    // the others are scanned at most once a second.
    static DWORD s_slot = XUSER_MAX_COUNT;
    static DWORD s_nextScan = 0;
    SvPad pad{};
    const DWORD now = GetTickCount();
    const bool scan = s_slot >= XUSER_MAX_COUNT || static_cast<LONG>(now - s_nextScan) >= 0;
    for (DWORD i = 0; i < XUSER_MAX_COUNT; ++i) {
        if (!scan && i != s_slot) continue;
        XINPUT_STATE st{};
        if (XInputGetState(i, &st) != ERROR_SUCCESS) {
            if (i == s_slot) s_slot = XUSER_MAX_COUNT;
            continue;
        }
        s_slot = i;
        pad.connected = 1;
        pad.packet = st.dwPacketNumber;
        pad.buttons = st.Gamepad.wButtons;
        pad.left_trigger = st.Gamepad.bLeftTrigger;
        pad.right_trigger = st.Gamepad.bRightTrigger;
        pad.left_x = st.Gamepad.sThumbLX;
        pad.left_y = st.Gamepad.sThumbLY;
        pad.right_x = st.Gamepad.sThumbRX;
        pad.right_y = st.Gamepad.sThumbRY;
        break;
    }
    if (scan) s_nextScan = now + 1000;
    return pad;
}

// Skate and the lifecycle see a neutral pad while the settings menu is open.
SvPad ReadPad() {
    if (!menu::IsOpen()) return ReadPadRaw();
    SvPad idle{};
    idle.connected = 1;
    return idle;
}

// ---- frame timing ----------------------------------------------------------
// Wall time between script frames (game-thread frame pacing) and the time
// SkateV's own per-frame work takes, logged every 10 s in or out of skate mode,
// so a hitch can be attributed to the game or to this mod.
class FrameMonitor {
public:
    FrameMonitor() {
        QueryPerformanceFrequency(&freq_);
        QueryPerformanceCounter(&last_);
        window_ = last_;
    }
    double Now() const {
        LARGE_INTEGER t;
        QueryPerformanceCounter(&t);
        return static_cast<double>(t.QuadPart) * 1000.0 / static_cast<double>(freq_.QuadPart);
    }
    // Call once per script frame, right after WAIT(0) returns.
    void Frame(bool active) {
        LARGE_INTEGER now;
        QueryPerformanceCounter(&now);
        const double ms = static_cast<double>(now.QuadPart - last_.QuadPart) * 1000.0 / static_cast<double>(freq_.QuadPart);
        last_ = now;
        ++frames_;
        if (ms > maxFrame_) maxFrame_ = ms;
        if (ms > 50.0) {
            ++hitches_;
            if (util::g_verbose) Logf("hitch %.1f ms (skating %d): SkateV script work %.2f ms", ms, active ? 1 : 0, lastWork_);
        }
        activeFrames_ += active ? 1 : 0;
        const double windowMs = static_cast<double>(now.QuadPart - window_.QuadPart) * 1000.0 / static_cast<double>(freq_.QuadPart);
        if (windowMs >= 10000.0) {
            if (util::g_verbose) Logf("frames %.1fs: %u frames (%.1f fps), worst %.1f ms, %u over 50 ms, SkateV work avg %.3f ms max %.3f ms, skating %u/%u",
                 windowMs / 1000.0, frames_, frames_ * 1000.0 / windowMs, maxFrame_, hitches_,
                 frames_ ? workTotal_ / frames_ : 0.0, workMax_, activeFrames_, frames_);
            frames_ = hitches_ = activeFrames_ = 0;
            maxFrame_ = workTotal_ = workMax_ = 0.0;
            window_ = now;
        }
    }
    void Work(double ms) {
        lastWork_ = ms;
        workTotal_ += ms;
        if (ms > workMax_) workMax_ = ms;
    }

private:
    LARGE_INTEGER freq_{}, last_{}, window_{};
    unsigned frames_ = 0, hitches_ = 0, activeFrames_ = 0;
    double maxFrame_ = 0.0, workTotal_ = 0.0, workMax_ = 0.0, lastWork_ = 0.0;
};

// ---- session ---------------------------------------------------------------
// One explicit lifecycle state machine (lifecycle.h) for the controller board
// action, the menu's manual activation and every forced exit.
using LifeState = lifecycle::State;

// Hand-offs between Skate and GTA (take out, put away, swim, climb) blend the
// camera and the body over retail Skate's camera shot transition time
// (camera_shots ground_blends / blendshots TransitionTime 0.5 s).
constexpr int kHandOffMs = 500;

// WEAPON_SKATEBOARD (skatev DLC, tools/build-board-weapon.py): the board in
// GTA's weapon wheel, a hammer clone whose hammer model is hidden.
constexpr Hash kBoardWeapon = 0x38A8F85C;

struct Session {
    LifeState state = LifeState::Unprepared;
    Ped ped = 0;
    Cam cam = 0;
    // Skate's camera while GTA's camera eases in after a hand-off.
    Cam fadingCam = 0;
    DWORD fadingCamUntil = 0;
    // The pose hook fades Skate's last pose into GTA's (put away/delegation)
    // and is removed once that fade is over; on entry it fades in once.
    bool poseFadingOut = false;
    DWORD poseFadeOutSince = 0;
    bool poseFadedIn = false;
    // GTA's facing for the skater while skating (the ped entity's heading:
    // minimap arrow, markers): turned toward the stance-free facing.
    float facing = 0.0f;
    bool boardBackward = false; // rolling toward the board's tail axis
    // Where the posed body faces (PoseFacing; NAN until a pose is seen).
    float poseHeading = NAN;
    // Board aiming (BoardAim): GTA's aim/fire on the ped's upper body.
    bool armed = false;              // gun out (GunButton)
    bool gunChordUsed = false;       // LB went down during this GunButton hold (board chord)
    DWORD gunHeldSince = 0;          // GunButton went down (weapon wheel on a long hold)
    Hash wheelPick = 0;              // the wheel's last highlighted weapon
    bool wheelOpen = false;          // GTA's weapon wheel held open from GunButton
    DWORD wheelClosedAt = 0;
    bool aiming = false;
    DWORD nextShot = 0;
    DWORD reloadDone = 0; // board reload in progress until (0 none, direct fire only)
    // GTA's own fire: 0 not seen yet, 1 GTA's shots (INPUT_ATTACK), -1 GTA did
    // not fire in time, so shots go straight at the target (direct fire).
    int nativeFire = 0;
    bool fireWas = false, fireSeen = false;
    DWORD fireSince = 0;
    int fireClip0 = 0;
    int reloadPulse = 0;        // frames left of INPUT_RELOAD
    Hash wheelLit0 = 0;         // GTA's first non-empty wheel highlight
    DWORD wheelOpenAt = 0;
    bool wheelForced = false;   // GTA did not open its own wheel: forced up
    int wheelSlice = -1;        // the stick-slice fallback's slice
    Hash wheelLastLit = 0;      // its last non-empty highlight
    bool wheelLitMoved = false; // GTA's own highlight left where it started
    DWORD gunProbeAt = 0;       // GunProbe: last task sweep
    Hash aimWeapon = 0x1B06D571;     // last allowed weapon (WEAPON_PISTOL until one is used)
    std::uint16_t aimPrevButtons = 0;
    int aimCycle = 0;                // pending weapon change: +1 next, -1 previous
    int aimCycleWait = 0;            // frames left for GTA's switch to land
    int aimCycleSteps = 0;           // switches tried so far
    Hash aimCycleFrom = 0;
    DWORD aimLog = 0;
    // WEAPON_SKATEBOARD (BoardWeaponTick): selected last frame, last grant check.
    bool boardWeaponWas = false;
    DWORD boardGrantAt = 0;
    bool boardWeaponMissingLogged = false;
    // Crimes (ReportShootings): last shots-fired report, last scan, peds already reported.
    DWORD shotsReportedAt = 0;
    DWORD crimeScanAt = 0;
    std::vector<int> crimeVictims;
    // Radio banner: the audible track shown last (0 none), when to show the
    // station alone if no track turns up, last poll.
    int radioTrack = 0;
    DWORD radioDue = 0;
    DWORD radioPoll = 0;
    DynamicWorld world;
    DWORD messageUntil = 0;
    std::string message;
    SvOutput last{};
    bool wipingOut = false; // last output was a Wipeout state (bail voice)
    // Bail impacts: the skater's velocity from Skate's positions (60 Hz ticks).
    SvVec3 bailPos{}, bailVel{};
    std::uint64_t bailTick = 0;
    bool bailVelValid = false;
    bool fallScream = false; // the falling scream played this bail
    DWORD lineAt = 0;        // get-up line due
    DWORD nearMissAt = 0, lastNearMiss = 0; // near-miss line due / last said
    DWORD lastGrunt = 0;
    // BailHits: pelvis, chest and head positions at the last three runtime ticks.
    SvVec3 hitPos[3][3]{};
    std::uint64_t hitTick[3]{};
    int hitSamples = 0;
    // Lifecycle bookkeeping.
    lifecycle::BoardAction board;
    DWORD loadedSince = 0;     // player loaded and stable since (Unprepared)
    DWORD lastTrack = 0;       // last sv_lifecycle_track
    DWORD stateSince = 0;      // entered the current state
    DWORD suppressEnterUntil = 0; // GTA's INPUT_ENTER held off after leaving
    DWORD yHeldSince = 0;         // Y pressed while skating (hold to put the board away)
    bool yWasDown = true;
    bool trackingStarted = false; // preserves opt-out until the first board action
    // GTA ped state saved on entry and restored on leaving.
    bool savedCanRagdoll = true;
    bool savedVisible = true;
    bool savedCollisionDisabled = false;
    DWORD lastPedAudit = 0;
    std::uint32_t pedAuthorityCorrections = 0;
    // Where the script last put the ped: a ped found elsewhere was moved by something
    // else (Menyoo, a teleport mod), and Skate must follow it, not put it back.
    float placed[3]{};
    bool placedValid = false;
    bool keepPlace = false;     // Leave: the ped stays where it is
    bool teleportWait = false;  // waiting for the collision at a teleport target
    // GTA delegation (swim / climb): which action, since when, settled since.
    enum class Delegation { None, Swim, Climb } delegation = Delegation::None;
    DWORD delegatedAt = 0;
    DWORD settledSince = 0;
    lifecycle::DoubleTap climbTap;
};

// Most the ped entity turns per second while skating. With the pose hook the
// body is posed in world space against the entity's live matrix, so a sudden
// entity turn shows one frame turned by the jump (a 180 degree flip when
// Skate's heading flipped); a bounded turn keeps that one-frame error to a few
// degrees. Presentation choice, not Skate tuning.
constexpr float kFacingTurnDegPerSec = 360.0f;

float GtaHeading(float x, float y) {
    const float h = std::atan2(-x, y) * 57.29578f;
    return h < 0.0f ? h + 360.0f : h;
}

// The skater's facing for GTA, independent of stance: on the board, the
// board's long axis pointed the way it rolls (riding switch or fakie still
// points forward; the side flips only when the motion is within 60 degrees of
// the other end, so slides do not flicker); on foot, the biped's facing
// (Skate's published heading, as the put-away hand-off uses). None while
// wiping out (the facing holds).
bool SkaterFacing(Session& s, const SvOutput& o, float& out) {
    const std::string_view st(o.state_utf8);
    if (st.rfind("Wipeout", 0) == 0) return false;
    if (st.rfind("Biped", 0) == 0) {
        if (!std::isfinite(s.poseHeading)) return false;
        out = s.poseHeading;
        return true;
    }
    // The board's long axis: -Y of its rotation.
    posemath::M m{};
    posemath::ToRows({o.board_rotation.x, o.board_rotation.y, o.board_rotation.z, o.board_rotation.w}, m);
    const float len = std::hypot(m.r[1][0], m.r[1][1]);
    if (!(len > 1e-3f)) return false; // board on end: no horizontal axis
    const float ax = -m.r[1][0] / len, ay = -m.r[1][1] / len;
    const float speed = std::hypot(o.velocity.x, o.velocity.y);
    const float along = o.velocity.x * ax + o.velocity.y * ay;
    if (along > 0.5f * speed && along > 0.0f) s.boardBackward = false;
    else if (-along > 0.5f * speed && along < 0.0f) s.boardBackward = true;
    out = s.boardBackward ? GtaHeading(-ax, -ay) : GtaHeading(ax, ay);
    return true;
}

// The way the posed body faces: both feet, foot bone to toe, in GTA world
// space. Skate's published skater heading can be half a turn off the visible
// body standing after entry, which turned the ped round on the put-away.
// `feet`: left foot, left toe, right foot, right toe bone indices.
bool PoseFacing(const float* pose, const int feet[4], float& out) {
    float x = 0.0f, y = 0.0f;
    for (int i = 0; i < 4; i += 2) {
        if (feet[i] < 0 || feet[i + 1] < 0) return false;
        x += pose[16 * feet[i + 1] + 12] - pose[16 * feet[i] + 12];
        y += pose[16 * feet[i + 1] + 13] - pose[16 * feet[i] + 13];
    }
    if (!(std::hypot(x, y) > 0.02f)) return false; // feet pointing down (flips, bails)
    out = GtaHeading(x, y);
    return true;
}

// Turns `current` toward `target` by at most `maxStep` degrees.
float TurnToward(float current, float target, float maxStep) {
    float d = std::fmod(target - current + 540.0f, 360.0f) - 180.0f;
    if (d > maxStep) d = maxStep;
    if (d < -maxStep) d = -maxStep;
    const float h = std::fmod(current + d + 360.0f, 360.0f);
    return h;
}

void SetState(Session& s, LifeState next, const char* why) {
    if (s.state == next) return;
    Logf("SkateV Legacy: lifecycle %s -> %s (%s)", lifecycle::Name(s.state), lifecycle::Name(next), why);
    s.state = next;
    s.stateSince = GetTickCount();
}

bool SkateOwnsPlayer(const Session& s) {
    return s.state == LifeState::Entering || s.state == LifeState::Skating || s.state == LifeState::Leaving;
}

void Notify(Session& s, const std::string& text, DWORD ms = 4000) {
    s.message = text;
    s.messageUntil = GetTickCount() + ms;
    Logf("SkateV Legacy: %s", text.c_str());
}

// While Skate poses the real ped, GTA's IK (feet to ground, arm/head/torso
// look-at) and gesture/ambient animation would rewrite limbs after our pose.
void SetGtaPosing(Ped ped, bool enabled) {
    const int on = enabled ? 1 : 0;
    Call<void>(gta::SET_PED_CAN_ARM_IK, ped, on);
    Call<void>(gta::SET_PED_CAN_HEAD_IK, ped, on);
    Call<void>(gta::SET_PED_CAN_LEG_IK, ped, on);
    Call<void>(gta::SET_PED_CAN_TORSO_IK, ped, on);
    Call<void>(gta::SET_PED_LEG_IK_MODE, ped, enabled ? 2 : 0);
    Call<void>(gta::SET_PED_CAN_PLAY_GESTURE_ANIMS, ped, on);
    Call<void>(gta::SET_PED_CAN_PLAY_AMBIENT_ANIMS, ped, on);
    Call<void>(gta::SET_PED_CAN_PLAY_AMBIENT_BASE_ANIMS, ped, on);
}

// The held ped (SkateGTA-B4's ride hold, under which its owned clip plays):
// frozen, collision and gravity off, never completely disabled (the ragdoll
// bounds would follow GTA's pose, not Skate's). With the clip playing, GTA's
// pose is Skate's, so the bounds follow the visible body. Released: GTA's own.
void ApplyHold(Ped ped, bool hold, bool collisionOn, bool ragdollAfter) {
    Call<void>(gta::FREEZE_ENTITY_POSITION, ped, hold ? 1 : 0);
    Call<void>(gta::SET_ENTITY_COLLISION, ped, collisionOn ? 1 : 0, 0);
    if (!hold) Call<void>(gta::SET_ENTITY_COMPLETELY_DISABLE_COLLISION, ped, collisionOn ? 1 : 0, 0);
    Call<void>(gta::SET_ENTITY_HAS_GRAVITY, ped, hold ? 0 : 1);
    // Keep GTA damage/perception authority: Skate suppresses duplicate physical
    // motion, but does not grant invincibility or change police/NPC awareness.
    Call<void>(gta::SET_PED_CAN_RAGDOLL, ped, hold ? 0 : (ragdollAfter ? 1 : 0));
}

void HoldPed(Session& s, bool hold) {
    const Ped ped = s.ped;
    if (hold) {
        // Restore GTA's pre-existing ragdoll and collision settings on leaving.
        s.savedCanRagdoll = Call<BOOL>(gta::CAN_PED_RAGDOLL, ped) != 0;
        s.savedVisible = Call<BOOL>(gta::IS_ENTITY_VISIBLE, ped) != 0;
        s.savedCollisionDisabled = Call<BOOL>(gta::GET_ENTITY_COLLISION_DISABLED, ped) != 0;
    }
    ApplyHold(ped, hold, !hold && !s.savedCollisionDisabled, s.savedCanRagdoll);
    Logf("SkateV Legacy: ped collision %s (GTA reports disabled=%d)", hold ? "removed" : "restored",
         static_cast<int>(Call<BOOL>(gta::GET_ENTITY_COLLISION_DISABLED, ped)));
}

// Pose/task changes may recreate GTA's physical representation after entry.
// Enforce the duplicate-physics boundary after all active-frame presentation
// work, without calling HoldPed (which would overwrite the restore snapshot).
void EnforceHeldPed(Session& s) {
    if (!SkateOwnsPlayer(s) || !s.ped || !Call<BOOL>(gta::DOES_ENTITY_EXIST, s.ped)) return;
    const bool disabledBefore = Call<BOOL>(gta::GET_ENTITY_COLLISION_DISABLED, s.ped) != 0;
    const bool ragdollBefore = Call<BOOL>(gta::CAN_PED_RAGDOLL, s.ped) != 0;
    const bool drift = !disabledBefore || ragdollBefore;
    if (drift) ++s.pedAuthorityCorrections;
    ApplyHold(s.ped, true, false, s.savedCanRagdoll);
    const DWORD now = GetTickCount();
    if ((drift || util::g_verbose) && now - s.lastPedAudit >= (drift ? 1000u : 5000u)) {
        s.lastPedAudit = now;
        Logf("SkateV Legacy: ped authority state=%s collisionDisabled=%d->%d canRagdoll=%d->%d corrections=%u",
             lifecycle::Name(s.state), static_cast<int>(disabledBefore),
             static_cast<int>(Call<BOOL>(gta::GET_ENTITY_COLLISION_DISABLED, s.ped)),
             static_cast<int>(ragdollBefore), static_cast<int>(Call<BOOL>(gta::CAN_PED_RAGDOLL, s.ped)),
             s.pedAuthorityCorrections);
    }
}

// A hard hit: the pelvis, chest or head changing velocity by more than
// BailHitSpeed between two runtime ticks (the skater's root glides through a
// ragdoll fall and sees almost no hits), in a bail only: outside one, the
// animated pose alone (an ollie pop, a push, mounting or stepping off) moved
// the bones past the limit and grunted all the time. Plays the character's
// pain grunt (BailPain).
void ImpactGrunts(Session& s, const Config& cfg, const float* pose, const int bones[3], std::uint64_t tick) {
    if (cfg.bailPain < 0) {
        s.hitSamples = 0;
        return;
    }
    if (s.hitSamples && tick <= s.hitTick[0]) return;
    for (int k = 2; k > 0; --k) {
        s.hitTick[k] = s.hitTick[k - 1];
        for (int b = 0; b < 3; ++b) s.hitPos[k][b] = s.hitPos[k - 1][b];
    }
    s.hitTick[0] = tick;
    for (int b = 0; b < 3; ++b)
        if (bones[b] >= 0) s.hitPos[0][b] = {pose[16 * bones[b] + 12], pose[16 * bones[b] + 13], pose[16 * bones[b] + 14]};
    if (++s.hitSamples < 3) return;
    const float dt0 = static_cast<float>(s.hitTick[0] - s.hitTick[1]) / 60.0f;
    const float dt1 = static_cast<float>(s.hitTick[1] - s.hitTick[2]) / 60.0f;
    if (dt0 > 0.1f || dt1 > 0.1f) return;
    float worst = 0.0f;
    for (int b = 0; b < 3; ++b) {
        if (bones[b] < 0) continue;
        const SvVec3 &p0 = s.hitPos[0][b], &p1 = s.hitPos[1][b], &p2 = s.hitPos[2][b];
        const float x = (p0.x - p1.x) / dt0 - (p1.x - p2.x) / dt1, y = (p0.y - p1.y) / dt0 - (p1.y - p2.y) / dt1,
                    z = (p0.z - p1.z) / dt0 - (p1.z - p2.z) / dt1;
        worst = std::max(worst, std::sqrt(x * x + y * y + z * z));
    }
    const DWORD now = GetTickCount();
    // A new grunt more often than this cut the last one off before it played.
    if (!s.wipingOut || worst <= cfg.bailHitSpeed || now - s.lastGrunt <= 700) return;
    s.lastGrunt = now;
    pedspeech::Pain(s.ped, cfg.bailPain, 0.0f);
    static unsigned s_logs = 0;
    if (s_logs++ < 200) Logf("SkateV Legacy: bail hit %.1f m/s: pain %d", worst, cfg.bailPain);
}

// A car only just missing the skater: within 1 m of the board at 7 m/s or
// more relative speed. The line follows once it has gone by, unless it hit
// after all (a bail grunt); at most every 15 s.
void NearMiss(Session& s, const Config& cfg, const std::vector<SvDynamicBody>& bodies) {
    const DWORD now = GetTickCount();
    if (cfg.nearMissSpeech.empty() || s.wipingOut) {
        s.nearMissAt = 0;
        return;
    }
    if (s.nearMissAt) {
        if (now - s.lastGrunt < 1000) {
            s.nearMissAt = 0;
        } else if (now >= s.nearMissAt) {
            s.nearMissAt = 0, s.lastNearMiss = now;
            const std::string said = pedspeech::Say(s.ped, cfg.nearMissSpeech);
            Logf("SkateV Legacy: near miss: %s", said.empty() ? "no listed context in this voice" : said.c_str());
        }
        return;
    }
    if (now - s.lastNearMiss < 15000) return;
    const SvVec3 p = s.last.board_position, v = s.last.velocity;
    for (const SvDynamicBody& b : bodies) {
        if ((b.fallback.tag >> 28) & 7) continue; // vehicles only
        const SvVec3 d{p.x - b.fallback.center.x, p.y - b.fallback.center.y, p.z - b.fallback.center.z};
        const SvVec3 axes[3] = {b.right, b.forward, b.up};
        const float half[3] = {b.fallback.half_extents.x, b.fallback.half_extents.y, b.fallback.half_extents.z};
        float outside = 0.0f;
        for (int k = 0; k < 3; ++k) {
            const SvVec3& a = axes[k];
            const float len = std::sqrt(a.x * a.x + a.y * a.y + a.z * a.z);
            if (len < 1e-6f) continue;
            const float e = std::fabs((d.x * a.x + d.y * a.y + d.z * a.z) / len) - half[k];
            if (e > 0.0f) outside += e * e;
        }
        const float rx = b.linear_velocity.x - v.x, ry = b.linear_velocity.y - v.y, rz = b.linear_velocity.z - v.z;
        if (outside < 1.0f && rx * rx + ry * ry + rz * rz > 49.0f) {
            s.nearMissAt = now + 600;
            return;
        }
    }
}

// Hands the player back to GTA. `voluntary`: Y held or the menu; the GTA
// ped takes Skate's current place and facing, whether riding or on foot.
// Otherwise (menu, death, runtime error, network) the ped is put where the
// skater last was.
void Leave(Session& s, const Config& cfg, const char* why, bool voluntary) {
    if (!SkateOwnsPlayer(s)) return;
    const bool wasSkating = s.state == LifeState::Skating;
    SetState(s, LifeState::Leaving, why);
    s.wipingOut = false, s.lineAt = 0; // no get-up line on the next entry
    s.placedValid = false;
    s.armed = false;
    posehook::FadeOut(kHandOffMs);
    if (s.ped) liveclip::Stop(s.ped);
    s.poseFadingOut = true;
    s.poseFadeOutSince = GetTickCount();
    boardnative::Stop(); // remove the instance; keep loaded model/type resident
    pedcollision::Stop();
    if (s.ped && Call<BOOL>(gta::DOES_ENTITY_EXIST, s.ped)) SetGtaPosing(s.ped, true);
    // Final Skate pose before Skate stops stepping.
    SvOutput o{};
    if (g_runtime.Output(o) && o.tick != 0) s.last = o;
    g_runtime.Deactivate();
    // GTA's camera takes over where Skate's camera looks, easing in; Skate's
    // camera stays until the ease is over.
    float camPitch = 0.0f, camHeading = NAN;
    if (s.cam && Call<BOOL>(gta::DOES_CAM_EXIST, s.cam)) {
        const Vector3 rot = Call<Vector3>(gta::GET_FINAL_RENDERED_CAM_ROT, 2);
        if (std::isfinite(rot.x) && std::isfinite(rot.z)) camPitch = rot.x, camHeading = rot.z;
        Call<void>(gta::RENDER_SCRIPT_CAMS, 0, 1, kHandOffMs, 1, 0, 0);
        if (s.fadingCam && Call<BOOL>(gta::DOES_CAM_EXIST, s.fadingCam)) Call<void>(gta::DESTROY_CAM, s.fadingCam, 0);
        s.fadingCam = s.cam;
        s.fadingCamUntil = GetTickCount() + kHandOffMs;
    }
    s.cam = 0;
    if (s.ped && Call<BOOL>(gta::DOES_ENTITY_EXIST, s.ped)) {
        const SvVec3 p = s.last.skater_position;
        const bool finite = std::isfinite(p.x) && std::isfinite(p.y) && std::isfinite(p.z);
        if (wasSkating && finite && !s.keepPlace) {
            // Skate's root is at ground contact (measured: biped root - ground
            // = 0.00 m); the GTA ped origin sits PedZOffset above it.
            Call<void>(gta::SET_ENTITY_COORDS_NO_OFFSET, s.ped, p.x, p.y, p.z + cfg.pedZOffset, 0, 0, 0);
            // On foot the feet give the facing; on the board they point
            // sideways, so the board's rolling direction (s.facing) does.
            const bool biped = std::string_view(s.last.state_utf8).rfind("Biped", 0) == 0;
            const float h = biped ? s.poseHeading : s.facing;
            if (voluntary && std::isfinite(h)) Call<void>(gta::SET_ENTITY_HEADING, s.ped, h);
        }
        HoldPed(s, false);
        if (!Call<BOOL>(gta::IS_ENTITY_VISIBLE, s.ped)) Call<void>(gta::SET_ENTITY_VISIBLE, s.ped, s.savedVisible ? 1 : 0, 0);
        // GTA's camera looks where Skate's camera looked (no cut behind the player).
        if (std::isfinite(camHeading)) {
            float relative = camHeading - Call<float>(gta::GET_ENTITY_HEADING, s.ped);
            while (relative > 180.0f) relative -= 360.0f;
            while (relative < -180.0f) relative += 360.0f;
            Call<void>(gta::SET_GAMEPLAY_CAM_RELATIVE_HEADING, relative);
            Call<void>(gta::SET_GAMEPLAY_CAM_RELATIVE_PITCH, camPitch, 1.0f);
        } else {
            Call<void>(gta::SET_GAMEPLAY_CAM_RELATIVE_HEADING, 0.0f);
            Call<void>(gta::SET_GAMEPLAY_CAM_RELATIVE_PITCH, 0.0f, 1.0f);
        }
    }
    Call<void>(gta::CLEAR_FOCUS);
    s.suppressEnterUntil = GetTickCount() + 750;
    s.lastTrack = 0; // resume tracking now
    SetState(s, LifeState::Ready, voluntary ? "back to GTA on foot" : why);
    if (!voluntary) Notify(s, std::string("skate mode OFF (") + why + ")", 2500);
    else Logf("SkateV Legacy: left skate mode at %.2f %.2f %.2f heading %.1f (feet %.1f, published %.1f; Skate state %s)",
              s.last.skater_position.x, s.last.skater_position.y, s.last.skater_position.z,
              Call<float>(gta::GET_ENTITY_HEADING, s.ped), s.poseHeading, s.last.skater_heading_degrees, s.last.state_utf8);
}

void SendCharacter(const Session& s, const Config& cfg);
void TryEnter(Session& s, const Config& cfg, bool strict);

// ---- GTA delegation ----------------------------------------------------------
// Skate keeps every movement it has (riding, walking, jumps, BackwardsMan).
// GTA performs only what Skate has no movement for: swimming once the skater's
// chest is under water, and GTA's climb over a barrier on X, X. Skate resumes
// off the board, board in hand, when GTA reports the player back on foot.

// Water depth over the skater's feet (GTA's own water surface), or < 0.
float WaterDepth(const SvVec3& feet) {
    float surface = 0.0f;
    if (!Call<BOOL>(gta::GET_WATER_HEIGHT_NO_WAVES, feet.x, feet.y, feet.z + 3.0f, &surface)) return -1.0f;
    return surface - feet.z;
}

// A barrier GTA can climb: blocked straight ahead at waist height, clear at
// head-and-a-bit (GTA's climb then decides the exact vault/climb).
// `heading`: the posed body's facing (PoseFacing; the published heading
// pointed behind the skater, so the probe looked the wrong way).
bool BarrierAhead(const SvOutput& o, float heading, Ped ignore) {
    if (!std::isfinite(heading)) return false;
    const float h = heading * 0.01745329f;
    const float fx = -std::sin(h), fy = std::cos(h);
    auto blocked = [&](float up) {
        const SvVec3 p = o.skater_position;
        const int test = Call<int>(gta::START_EXPENSIVE_SYNCHRONOUS_SHAPE_TEST_LOS_PROBE, p.x, p.y, p.z + up,
                                   p.x + fx * 1.2f, p.y + fy * 1.2f, p.z + up, 1 | 16, ignore, 7);
        BOOL hit = 0;
        Vector3 end{}, normal{};
        Entity entity = 0;
        return Call<int>(gta::GET_SHAPE_TEST_RESULT, test, &hit, &end, &normal, &entity) == 2 && hit;
    };
    return blocked(0.7f) && !blocked(2.4f);
}

void Delegate(Session& s, const Config& cfg, Session::Delegation what) {
    const SvVec3 v = s.last.velocity;
    const Ped ped = s.ped;
    Leave(s, cfg, what == Session::Delegation::Swim ? "swimming (GTA)" : "climbing (GTA)", true);
    if (!ped || !Call<BOOL>(gta::DOES_ENTITY_EXIST, ped)) return;
    // Skate's momentum carries into GTA's movement.
    if (std::isfinite(v.x) && std::isfinite(v.y) && std::isfinite(v.z)) Call<void>(gta::SET_ENTITY_VELOCITY, ped, v.x, v.y, v.z);
    if (what == Session::Delegation::Climb) Call<void>(gta::TASK_CLIMB, ped, 1);
    s.delegation = what;
    s.delegatedAt = GetTickCount();
    s.settledSince = 0;
    SetState(s, LifeState::Delegated, what == Session::Delegation::Swim ? "GTA swims" : "GTA climbs");
}

// Skating: hand over to GTA for an action Skate has no movement for.
bool CheckDelegation(Session& s, const Config& cfg, const SvPad& pad) {
    if (!cfg.delegation || s.last.tick == 0) return false;
    const std::string_view state(s.last.state_utf8);
    const DWORD now = GetTickCount();
    const bool onFoot = state.rfind("Biped", 0) == 0;
    const bool tap = s.climbTap.Update((pad.buttons & 0x4000) != 0, now); // X
    if (WaterDepth(s.last.skater_position) >= cfg.swimDepth) {
        Delegate(s, cfg, Session::Delegation::Swim);
        return true;
    }
    if (tap && onFoot && BarrierAhead(s.last, s.poseHeading, s.ped)) {
        Delegate(s, cfg, Session::Delegation::Climb);
        return true;
    }
    return false;
}

// Ground contact under the player (2 in above ground, like the mashup host).
SvVec3 GroundUnder(Ped ped) {
    const Vector3 pos = Call<Vector3>(gta::GET_ENTITY_COORDS, ped, 1);
    float ground = pos.z - 1.0f;
    Call<BOOL>(gta::GET_GROUND_Z_FOR_3D_COORD, pos.x, pos.y, pos.z + 0.5f, &ground, 0, 0);
    return {pos.x, pos.y, ground};
}

// Script-toggled map states (alternate interiors, mission states) GTA has
// active near GTA point `at`: sent to the runtime when the set changes, and
// always with `force` (before an activation), so their collision is there.
void SendMapStates(const SvVec3& at, bool force) {
    std::vector<std::uint32_t> active;
    std::string names;
    if (!mapstates::Refresh(at.x, at.y, force, active, names)) return;
    if (g_runtime.MapStatesSet(active.data(), static_cast<std::uint32_t>(active.size())))
        Logf("SkateV Legacy: map states active near (%.0f, %.0f): %s", at.x, at.y, names.empty() ? "none" : names.c_str());
}

// Lifecycle tracking: the player's position while skate mode is inactive
// (on foot or driving). The first call prepares the Skate session there.
void Track(Session& s, bool force) {
    const DWORD now = GetTickCount();
    if (!force && now - s.lastTrack < 250) return;
    s.lastTrack = now;
    const Ped ped = Call<Ped>(gta::PLAYER_PED_ID);
    if (!Call<BOOL>(gta::DOES_ENTITY_EXIST, ped)) return;
    s.trackingStarted = true;
    SvLifecycleTrack t{};
    t.size = sizeof(t);
    t.flags = Call<BOOL>(gta::IS_PED_IN_ANY_VEHICLE, ped, 0) ? 1u : 0u;
    t.position = GroundUnder(ped);
    t.heading_degrees = Call<float>(gta::GET_ENTITY_HEADING, ped);
    SendMapStates(t.position, false);
    g_runtime.LifecycleTrack(t);
}

// The Skate session exists and the collision working set covers the player.
bool AreaReady(const char*& why) {
    SvLifecycleState l{};
    if (!g_runtime.LifecycleState(l)) {
        why = "no lifecycle state";
        return false;
    }
    if (!l.prepared) {
        why = l.preparing ? "preparing Skate" : "Skate not prepared yet";
        return false;
    }
    const Vector3 pos = Call<Vector3>(gta::GET_ENTITY_COORDS, Call<Ped>(gta::PLAYER_PED_ID), 1);
    const float dx = pos.x - l.collision_center.x, dy = pos.y - l.collision_center.y;
    if (dx * dx + dy * dy > 60.0f * 60.0f) {
        why = "loading this area";
        return false;
    }
    why = nullptr;
    return true;
}

// Board action (or the menu): start Skate here. `strict` = controller (all GTA
// guards); the menu skips the transient ones.
void TryEnter(Session& s, const Config& cfg, bool strict) {
    const Ped ped = Call<Ped>(gta::PLAYER_PED_ID);
    {
        SvOutput o{};
        g_runtime.Output(o);
        if (o.status == SV_STATUS_ERROR) {
            Notify(s, "Skate runtime error: " + g_runtime.StatusText(), 6000);
            SetState(s, LifeState::Ready, "runtime error");
            return;
        }
    }
    if (const char* why = lifecycle::Blocked(ped, strict, cfg.life)) {
        Notify(s, std::string("can't skate now: ") + why, 2500);
        SetState(s, LifeState::Ready, why);
        return;
    }
    if (const char* why = nullptr; !AreaReady(why)) {
        Track(s, true); // also starts preparation when BackgroundPrepare=0
        SetState(s, LifeState::Waiting, why);
        return;
    }
    SvOutput o{};
    g_runtime.Output(o);
    if (o.status != SV_STATUS_READY) {
        Notify(s, "Skate runtime not ready: " + g_runtime.StatusText());
        SetState(s, LifeState::Ready, "runtime not ready");
        return;
    }
    SvSpawn spawn{};
    spawn.size = sizeof(spawn);
    spawn.position = GroundUnder(ped);
    spawn.position.z += 0.05f;
    spawn.heading_degrees = Call<float>(gta::GET_ENTITY_HEADING, ped);
    s.facing = spawn.heading_degrees;
    s.poseHeading = NAN;
    s.boardBackward = false;
    spawn.aspect_ratio = Call<float>(gta::GET_ASPECT_RATIO, 0);
    // Off the board with the board in hand (the player's Y mounts).
    SendMapStates(spawn.position, true);
    if (!g_runtime.LifecycleEnter(spawn)) {
        Notify(s, "activation rejected: " + g_runtime.StatusText());
        SetState(s, LifeState::Ready, "activation rejected");
        return;
    }
    Call<void>(gta::CLEAR_PED_TASKS_IMMEDIATELY, ped);
    // The board weapon's hammer stand-in goes with the hand-over (Skate holds the board).
    if (Call<Hash>(gta::GET_SELECTED_PED_WEAPON, ped) == kBoardWeapon)
        Call<void>(gta::SET_CURRENT_PED_WEAPON, ped, 0xA2719263, 1);
    s.ped = ped;
    HoldPed(s, true);
    // The runtime solves Skate's pose on the character's skeleton.
    SendCharacter(s, g_config);
    SetState(s, LifeState::Entering, strict ? "board action" : "menu");
    Logf("SkateV Legacy: entering off the board at %.2f %.2f %.2f heading %.1f", spawn.position.x, spawn.position.y, spawn.position.z, spawn.heading_degrees);
}

// Tells the runtime which GTA character wears Skate's pose: the player's own
// model and the outfit it has on right now.
void SendCharacter(const Session& s, const Config& cfg) {
    if (!cfg.gtaCharacter || cfg.pedCache.empty()) {
        g_runtime.SetCharacter(nullptr);
        return;
    }
    SvCharacter c{};
    c.size = sizeof(c);
    c.model_hash = Call<Hash>(gta::GET_ENTITY_MODEL, s.ped);
    c.cache_root_utf8 = cfg.pedCache.c_str();
    for (int slot = 0; slot < 12; ++slot) {
        const int d = Call<int>(gta::GET_PED_DRAWABLE_VARIATION, s.ped, slot);
        const int t = Call<int>(gta::GET_PED_TEXTURE_VARIATION, s.ped, slot);
        c.drawable[slot] = static_cast<std::uint16_t>(d < 0 ? 0 : d);
        c.texture[slot] = static_cast<std::uint8_t>(t < 0 ? 0 : t);
    }
    c.triangle_budget = static_cast<std::uint32_t>(cfg.characterTriangles > 0 ? cfg.characterTriangles : 0);
    g_runtime.SetCharacter(&c);
    Logf("SkateV Legacy: character model %08x outfit sent", c.model_hash);
}

void BeginActive(Session& s, const Config& cfg) {
    if (cfg.drawBoard) boardnative::Start(g_gameDir + L"SkateVLegacy.ini",
        std::filesystem::u8path(cfg.dataRoot).wstring(), Log);
    if (cfg.posePed) pedcollision::Start(g_gameDir + L"SkateVLegacy.ini",
        std::filesystem::u8path(cfg.dataRoot).wstring(), Log);
    if (s.fadingCam && Call<BOOL>(gta::DOES_CAM_EXIST, s.fadingCam)) Call<void>(gta::DESTROY_CAM, s.fadingCam, 0);
    s.fadingCam = 0;
    s.cam = Call<Cam>(gta::CREATE_CAM, "DEFAULT_SCRIPTED_CAMERA", 1);
    {
        const Vector3 at = Call<Vector3>(gta::GET_FINAL_RENDERED_CAM_COORD);
        const Vector3 rot = Call<Vector3>(gta::GET_FINAL_RENDERED_CAM_ROT, 2);
        Call<void>(gta::SET_CAM_COORD, s.cam, at.x, at.y, at.z);
        Call<void>(gta::SET_CAM_ROT, s.cam, rot.x, rot.y, rot.z, 2);
    }
    Call<void>(gta::SET_CAM_ACTIVE, s.cam, 1);
    Call<void>(gta::RENDER_SCRIPT_CAMS, 1, 1, kHandOffMs, 1, 0, 0);
    s.poseFadedIn = false;
    s.poseFadingOut = false;
    s.armed = s.aiming = false;
    s.nativeFire = 0, s.fireWas = false, s.wheelOpen = false;
    liveclip::Suspend(s.ped, false);
    posehook::SetGun(false, 1);
    Call<void>(gta::SET_CURRENT_PED_WEAPON, s.ped, 0xA2719263, 1); // gun away (GunButton takes it out)
    SvLifecycleState l{};
    if (g_runtime.LifecycleState(l)) {
        Logf("SkateV Legacy: skate mode ON after %lu ms (runtime activation %.0f ms, session builds %u)",
             GetTickCount() - s.stateSince, l.last_activate_ms, l.session_builds);
    }
    SetState(s, LifeState::Skating, "runtime active");
}
// Board aiming. Sniper rifles and the homing launcher take over the camera,
// so they are not offered; unarmed has nothing to aim.
constexpr int kAimBlendMs = 200;
bool AimAllowed(Hash w) {
    if (!w || w == 0xA2719263 /*UNARMED*/ || w == 0x63AB0442 /*HOMINGLAUNCHER*/ || w == kBoardWeapon) return false;
    return Call<Hash>(gta::GET_WEAPONTYPE_GROUP, w) != 0xB7BBD827; // GROUP_SNIPER
}

// Off the board: every character owns WEAPON_SKATEBOARD (granted, never
// equipped), its hammer stand-in never shows, and picking it in the wheel
// takes the board out like the board chord (roadmap Phase 2). True on the
// frame it becomes the selected weapon.
bool BoardWeaponTick(Session& s, Ped player) {
    const DWORD now = GetTickCount();
    if (now - s.boardGrantAt >= 2000) {
        s.boardGrantAt = now;
        if (!Call<BOOL>(gta::IS_WEAPON_VALID, kBoardWeapon)) {
            if (!s.boardWeaponMissingLogged) Log("SkateV Legacy: WEAPON_SKATEBOARD not loaded (skatev pack without weapon meta)");
            s.boardWeaponMissingLogged = true;
        } else if (!Call<BOOL>(gta::HAS_PED_GOT_WEAPON, player, kBoardWeapon, 0)) {
            Call<void>(gta::GIVE_WEAPON_TO_PED, player, kBoardWeapon, 1, 0, 0);
            Logf("SkateV Legacy: board weapon granted=%d",
                 static_cast<int>(Call<BOOL>(gta::HAS_PED_GOT_WEAPON, player, kBoardWeapon, 0)));
        }
    }
    const bool selected = Call<Hash>(gta::GET_SELECTED_PED_WEAPON, player) == kBoardWeapon;
    if (selected) {
        const Entity standIn = Call<Entity>(gta::GET_CURRENT_PED_WEAPON_ENTITY_INDEX, player, 0);
        if (standIn) Call<void>(gta::SET_ENTITY_VISIBLE, standIn, 0, 0);
    }
    const bool picked = selected && !s.boardWeaponWas;
    s.boardWeaponWas = selected;
    return picked;
}

void StopAim(Session& s, bool restoreCam) {
    if (!s.aiming) return;
    s.aiming = false;
    s.aimCycle = 0;
    s.fireWas = false;
    if (restoreCam && s.cam) Call<void>(gta::RENDER_SCRIPT_CAMS, 1, 1, kAimBlendMs, 1, 0, 0);
    Log("SkateV Legacy: board aim off");
}

// Called with the pad about to go to Skate. While aiming, only the left
// stick and A (push) reach Skate; GTA's own aim/attack controls run on the
// ped and the pose hook hands it the upper body.
// Skate mode takes GTA's player controls one by one, never with
// DISABLE_ALL_CONTROL_ACTIONS: GTA reports replay unavailable on any frame
// that calls it and stops a recording (re-enabling the replay controls
// afterwards does not help). Kept:
// pause, the replay controls, the character wheel (the controller's record
// menu) unless LB is held (LB + d-pad is Skate's session marker), and while
// aiming the look, aim, attack and reload controls.
// GTA's weapon wheel is open (BoardControls): its controls stay GTA's.
bool g_weaponWheel = false;

void DisableSkateControls(std::uint16_t buttons, bool aiming) {
    if (menu::IsOpen()) return; // the open menu disables every control but pause
    for (int c = 0; c < 360; ++c) {
        // INPUT_WEAPON_WHEEL_UD/LR/NEXT/PREV, SELECT_NEXT/PREV_WEAPON, SELECT_WEAPON
        // (and the look axes 1, 2: the pad's right stick, in case the wheel reads it there)
        if (g_weaponWheel && ((c >= 12 && c <= 17) || c == 37 || c == 1 || c == 2)) continue;
        if (c == 199 || c == 200 || c == 170 || c == 288 || c == 289 || (c >= 296 && c <= 328) || c == 349) continue;
        if (c == 19 && !(buttons & 0x0100)) continue;
        if (aiming && (c == 1 || c == 2 || c == 24 || c == 25 || c == 45 || (c >= 270 && c <= 273) || (c >= 290 && c <= 295))) continue;
        Call<void>(gta::DISABLE_CONTROL_ACTION, 0, c, 1);
    }
}

// Radio while skating (GTA's mobile radio): +1 next station, -1 previous,
// with "off" between the last and the first.
void StepRadio(Session& s, int dir) {
    const int n = Call<int>(gta::GET_NUM_UNLOCKED_RADIO_STATIONS);
    if (n <= 0) return;
    int cur = Call<BOOL>(gta::IS_MOBILE_PHONE_RADIO_ACTIVE) ? Call<int>(gta::GET_PLAYER_RADIO_STATION_INDEX) : 255;
    if (cur < 0 || cur >= n) cur = n; // off
    const int next = (cur + dir + n + 1) % (n + 1);
    if (next == n) {
        Call<void>(gta::SET_MOBILE_PHONE_RADIO_STATE, 0);
        Call<void>(gta::SET_MOBILE_RADIO_ENABLED_DURING_GAMEPLAY, 0);
        Call<void>(gta::SET_AUDIO_FLAG, "MobileRadioInGame", 0);
        s.radioDue = 0;
        if (!hudoverlay::ShowRadio("Radio Off", "", "", "OFF")) Notify(s, "Radio off", 1500);
        return;
    }
    Call<void>(gta::SET_AUDIO_FLAG, "MobileRadioInGame", 1);
    Call<void>(gta::SET_MOBILE_RADIO_ENABLED_DURING_GAMEPLAY, 1);
    Call<void>(gta::SET_MOBILE_PHONE_RADIO_STATE, 1);
    Call<void>(gta::SET_RADIO_TO_STATION_INDEX, next);
    // RadioTick shows the banner once the station's track is known.
    s.radioTrack = -1;
    s.radioDue = GetTickCount() + 1200;
    const char* id = Call<const char*>(gta::GET_RADIO_STATION_NAME, next);
    Logf("SkateV Legacy: radio station %d/%d %s", next, n, id ? id : "?");
}

// Label text, or "" when the label does not exist.
std::string GameText(const char* label) {
    if (!label || !*label || !Call<BOOL>(gta::DOES_TEXT_LABEL_EXIST, label)) return {};
    const char* text = Call<const char*>(gta::GET_FILENAME_FOR_AUDIO_CONVERSATION, label);
    return text ? text : "";
}

// While skating with the radio on: the TRAX banner shows the song, artist
// and station whenever the audible track changes (GTA's radio wheel labels:
// `<track>S` song, `<track>A` artist), or the station alone 1.2 s after a
// switch when no track is known (talk, adverts).
// The song/artist labels live in GTA's TRACKID text block (x64b.rpf
// american_rel.rpf/trackid.gxt2), not in the global text; GTA loads it only
// for its own radio UI. Kept in a free additional text slot (highest first,
// so mission text is never evicted); true once it is in.
bool TrackTextLoaded() {
    static int slot = -1;
    if (slot >= 0 && Call<BOOL>(gta::HAS_THIS_ADDITIONAL_TEXT_LOADED, "TRACKID", slot)) return true;
    for (int i = 19; i >= 0; --i)
        if (Call<BOOL>(gta::HAS_THIS_ADDITIONAL_TEXT_LOADED, "TRACKID", i)) return slot = i, true;
    if (slot < 0 || Call<BOOL>(gta::HAS_ADDITIONAL_TEXT_LOADED, slot)) {
        slot = -1;
        for (int i = 19; i >= 10 && slot < 0; --i)
            if (!Call<BOOL>(gta::HAS_ADDITIONAL_TEXT_LOADED, i)) slot = i;
        if (slot < 0) return false;
        Call<void>(gta::REQUEST_ADDITIONAL_TEXT, "TRACKID", slot);
        Logf("SkateV Legacy: radio track names requested (TRACKID, text slot %d)", slot);
    }
    return false;
}

void RadioTick(Session& s) {
    if (!Call<BOOL>(gta::IS_MOBILE_PHONE_RADIO_ACTIVE)) return;
    const DWORD now = GetTickCount();
    if (now - s.radioPoll < 250) return;
    s.radioPoll = now;
    if (!TrackTextLoaded() && !(s.radioDue && static_cast<int>(now - s.radioDue) >= 0)) return;
    const int track = Call<int>(gta::GET_AUDIBLE_MUSIC_TRACK_TEXT_ID);
    char label[24];
    std::snprintf(label, sizeof label, "%dS", track);
    const std::string song = track > 1 ? GameText(label) : std::string();
    std::snprintf(label, sizeof label, "%dA", track);
    const std::string artist = track > 1 ? GameText(label) : std::string();
    const bool known = !song.empty();
    const bool due = s.radioDue && static_cast<int>(now - s.radioDue) >= 0;
    if (!(known && track != s.radioTrack) && !due) return;
    s.radioDue = 0;
    s.radioTrack = known ? track : 0;
    const char* id = Call<const char*>(gta::GET_PLAYER_RADIO_STATION_NAME);
    std::string station = GameText(id);
    if (station.empty()) station = id ? id : "Radio";
    // Station as the heading, then song, then artist.
    const bool shown = known ? hudoverlay::ShowRadio(station.c_str(), song.c_str(), artist.c_str(), id ? id : "")
                             : hudoverlay::ShowRadio(station.c_str(), "", "", id ? id : "");
    if (!shown) Notify(s, known ? song + " - " + artist : station, 2500);
    Logf("SkateV Legacy: radio banner track=%d station=%s song=%s artist=%s", track, id ? id : "?", song.c_str(),
         artist.c_str());
}

// While the gun is out GTA's arm IK (the left hand onto the grip) and, with
// GunIK=2, torso and head IK run on the ped again (SetGtaPosing turned them
// off for Skate's pose); legs, gestures and ambient animation stay off.
void GunIk(Ped ped, bool on) {
    const int mode = on ? g_config.gunIk : 0;
    Call<void>(gta::SET_PED_CAN_ARM_IK, ped, mode >= 1 ? 1 : 0);
    Call<void>(gta::SET_PED_CAN_TORSO_IK, ped, mode >= 2 ? 1 : 0);
    Call<void>(gta::SET_PED_CAN_HEAD_IK, ped, mode >= 2 ? 1 : 0);
}

constexpr int kGunBlendMs = 250;

// Gun out: GTA sees an armed player ped. The clip that held the ped's task slot
// stops, so GTA's own player tasks run (aim, shoot, reload, the weapon wheel)
// and animate the upper body. Skate's own pose stays until the player aims;
// then the pose hook takes GTA's upper body and keeps Skate's hips and legs
// (gun_pose.h, BoardControls).
void GunOut(Session& s, Hash w) {
    Call<void>(gta::SET_CURRENT_PED_WEAPON, s.ped, w, 1);
    s.aimWeapon = w;
    s.armed = true;
    s.nextShot = s.reloadDone = 0;
    liveclip::Suspend(s.ped, true);
    Call<void>(gta::CLEAR_PED_TASKS, s.ped);
    Logf("SkateV Legacy: gun out (weapon 0x%08X)", static_cast<unsigned>(w));
}

void Holster(Session& s) {
    StopAim(s, true);
    s.armed = false;
    Call<void>(gta::SET_CURRENT_PED_WEAPON, s.ped, 0xA2719263, 1); // WEAPON_UNARMED
    posehook::SetGun(false, kGunBlendMs);
    liveclip::Suspend(s.ped, false); // Skate's clip takes the task slot back
    GunIk(s.ped, false);
}

// The gun to take out: the selected weapon when the board can use it, else the
// last one; false when the ped has none.
bool TakeOutGun(Session& s) {
    Hash w = Call<Hash>(gta::GET_SELECTED_PED_WEAPON, s.ped);
    if (!AimAllowed(w)) w = s.aimWeapon;
    if (!AimAllowed(w) || !Call<BOOL>(gta::HAS_PED_GOT_WEAPON, s.ped, w, 0)) return false;
    GunOut(s, w);
    return true;
}

// GunProbe: what GTA runs on the armed ped, once a second (one run of the
// game should say which of GTA's own systems work on the board).
void GunProbe(Session& s, const char* state) {
    const DWORD now = GetTickCount();
    if (now - s.aimLog < 1000) return;
    s.aimLog = now;
    const Hash w = Call<Hash>(gta::GET_SELECTED_PED_WEAPON, s.ped);
    int clip = 0;
    Call<BOOL>(gta::GET_AMMO_IN_CLIP, s.ped, w, &clip);
    Logf("SkateV Legacy: board gun weapon=0x%08X clip %d total %d aiming=%d (GTA free-aiming %d) shooting %d reloading %d nativeFire %d "
         "heading %.0f camera %.0f falling %d wheel %.8X state=%s",
         static_cast<unsigned>(w), clip, Call<int>(gta::GET_AMMO_IN_PED_WEAPON, s.ped, w), static_cast<int>(s.aiming),
         static_cast<int>(Call<BOOL>(gta::IS_PLAYER_FREE_AIMING, Call<Player>(gta::PLAYER_ID))),
         static_cast<int>(Call<BOOL>(gta::IS_PED_SHOOTING, s.ped)), static_cast<int>(Call<BOOL>(gta::IS_PED_RELOADING, s.ped)),
         s.nativeFire, Call<float>(gta::GET_ENTITY_HEADING, s.ped), Call<Vector3>(gta::GET_GAMEPLAY_CAM_ROT, 2).z,
         static_cast<int>(Call<BOOL>(gta::IS_PED_FALLING, s.ped)),
         static_cast<unsigned>(Call<Hash>(gta::HUD_GET_WEAPON_WHEEL_CURRENTLY_HIGHLIGHTED)), state);
    if (now - s.gunProbeAt >= 3000) {
        s.gunProbeAt = now;
        char tasks[600];
        int len = std::snprintf(tasks, sizeof tasks, "SkateV Legacy: board gun GTA tasks active:");
        for (int t = 0; t < 560 && len < static_cast<int>(sizeof tasks) - 8; ++t)
            if (Call<BOOL>(gta::GET_IS_TASK_ACTIVE, s.ped, t)) len += std::snprintf(tasks + len, sizeof tasks - len, " %d", t);
        Log(tasks);
    }
}

// GTA's camera takes over looking from where Skate's camera was (aim, wheel).
void GameplayCam(Session& s) {
    if (s.cam) {
        const Vector3 rot = Call<Vector3>(gta::GET_CAM_ROT, s.cam, 2);
        Call<void>(gta::SET_GAMEPLAY_CAM_RELATIVE_HEADING, rot.z - Call<float>(gta::GET_ENTITY_HEADING, s.ped));
        Call<void>(gta::SET_GAMEPLAY_CAM_RELATIVE_PITCH, rot.x, 1.0f);
    }
    Call<void>(gta::RENDER_SCRIPT_CAMS, 0, 1, kAimBlendMs, 1, 0, 0);
}

// Called with the pad about to go to Skate. Gun away: only the radio (d-pad
// left/right) is taken. Gun out: GTA runs the ped's player tasks, so its own
// aim, attack, reload and weapon wheel act on the pad as on foot: LB/RB stand in
// for INPUT_AIM / INPUT_ATTACK (the triggers stay Skate's grabs), the d-pad
// changes weapon, B reloads; while aiming only the left stick and A (push)
// reach Skate. The pose hook gives the upper body to GTA (gun_pose.h).
void BoardControls(Session& s, const Config& cfg, SvPad& pad) {
    const std::uint16_t buttons = pad.buttons;
    const std::uint16_t pressed = buttons & ~s.aimPrevButtons;
    const std::uint16_t released = s.aimPrevButtons & ~buttons;
    s.aimPrevButtons = buttons;
    RadioTick(s);
    if (!cfg.boardAim || !cfg.posePed) return;

    // GunButton held opens GTA's weapon wheel with the gun out; the right
    // stick picks and letting go selects. The hold then
    // does not put the gun away.
    const DWORD tick = GetTickCount();
    if (pressed & cfg.gunButton) s.gunHeldSince = tick;
    if ((buttons & cfg.gunButton) && !(buttons & 0x0100) && tick - s.gunHeldSince >= 300 &&
        (s.armed || s.wheelOpen || TakeOutGun(s))) {
        if (!s.wheelOpen) {
            s.wheelOpen = true;
            s.wheelPick = 0;
            s.wheelLit0 = s.wheelLastLit = 0;
            s.wheelLitMoved = s.wheelForced = false;
            s.wheelSlice = -1;
            s.wheelOpenAt = tick;
            char slots[200];
            int len = 0;
            for (int i = 0; i < 8; ++i) {
                const Hash h = Call<Hash>(gta::HUD_GET_WEAPON_WHEEL_TOP_SLOT, i);
                len += std::snprintf(slots + len, sizeof(slots) - len, " %d:%08X/%08X", i, static_cast<unsigned>(h),
                                     static_cast<unsigned>(h ? Call<Hash>(gta::GET_WEAPONTYPE_GROUP, h) : 0));
            }
            Logf("SkateV Legacy: weapon wheel slots (weapon/group):%s", slots);
            StopAim(s, true);
            // GTA's own camera, as when aiming: its wheel may not open under a
            // script camera.
            GameplayCam(s);
            Log("SkateV Legacy: weapon wheel open");
        }
        s.gunChordUsed = true;
    } else if (s.wheelOpen) {
        s.wheelOpen = false;
        s.wheelClosedAt = tick;
        Call<void>(gta::HUD_FORCE_WEAPON_WHEEL, 0);
        if (s.cam && !s.aiming) Call<void>(gta::RENDER_SCRIPT_CAMS, 1, 1, kAimBlendMs, 1, 0, 0);
        // GTA's own highlight is what the wheel picks; where it never moved
        // (no GTA task running on the ped), the stick's slice below.
        if (s.wheelLitMoved && s.wheelLastLit) s.wheelPick = s.wheelLastLit;
        if (s.wheelPick && s.wheelPick != Call<Hash>(gta::GET_SELECTED_PED_WEAPON, s.ped))
            Call<void>(gta::SET_CURRENT_PED_WEAPON, s.ped, s.wheelPick, 1);
        Logf("SkateV Legacy: weapon wheel closed on 0x%08X (%s, GTA's own highlight %s)", static_cast<unsigned>(s.wheelPick),
             s.wheelForced ? "forced up" : "opened by GTA", s.wheelLitMoved ? "moved" : "never moved");
    }
    g_weaponWheel = s.wheelOpen;
    if (s.wheelOpen) {
        // INPUT_SELECT_WEAPON held (GTA's wheel, highlight and selection). Holding
        // it alone may not show the wheel on the held ped; if GTA has not opened
        // its own in 400 ms (no highlight), it is forced up.
        Call<void>(gta::ENABLE_CONTROL_ACTION, 0, 37, 1);
        Call<BOOL>(gta::SET_CONTROL_VALUE_NEXT_FRAME, 0, 37, 1.0f);
        const Hash lit = Call<Hash>(gta::HUD_GET_WEAPON_WHEEL_CURRENTLY_HIGHLIGHTED);
        if (lit) {
            if (!s.wheelLit0) s.wheelLit0 = lit;
            else if (lit != s.wheelLit0) s.wheelLitMoved = true;
            s.wheelLastLit = lit;
        }
        if (!s.wheelLit0 && tick - s.wheelOpenAt >= 400) s.wheelForced = true;
        if (s.wheelForced) {
            Call<void>(gta::HUD_FORCE_WEAPON_WHEEL, 1);
            Call<void>(gta::SHOW_HUD_COMPONENT_THIS_FRAME, 19); // HUD_WEAPON_WHEEL
        }
        static DWORD s_wheelLog = 0;
        static Hash s_lastLit = 0;
        if (lit != s_lastLit || tick - s_wheelLog >= 500) {
            // GTA's clock against the wall clock: below 1 the world is in slow motion.
            static DWORD s_gameAt = 0, s_wallAt = 0;
            const DWORD game = static_cast<DWORD>(Call<int>(gta::GET_GAME_TIMER));
            if (s_wallAt && tick - s_wallAt > 0 && tick - s_wallAt < 2000)
                Logf("SkateV Legacy: weapon wheel clock: game %.2fx wall, frame time %.4f s, time step %.4f s", static_cast<float>(game - s_gameAt) / static_cast<float>(tick - s_wallAt),
                     Call<float>(gta::GET_FRAME_TIME), Call<float>(gta::TIMESTEP));
            s_gameAt = game, s_wallAt = tick;
            s_wheelLog = tick, s_lastLit = lit;
            Logf("SkateV Legacy: weapon wheel highlight 0x%08X forced %d, select pressed %d (control %.2f, disabled %.2f), stick (%.2f %.2f), wheel LR/UD %.2f/%.2f (disabled %.2f/%.2f), look %.2f/%.2f",
                 static_cast<unsigned>(lit), static_cast<int>(s.wheelForced), static_cast<int>(Call<BOOL>(gta::IS_CONTROL_PRESSED, 0, 37)),
                 Call<float>(gta::GET_CONTROL_NORMAL, 0, 37), Call<float>(gta::GET_DISABLED_CONTROL_NORMAL, 0, 37), pad.right_x / 32767.0f, pad.right_y / 32767.0f,
                 Call<float>(gta::GET_CONTROL_NORMAL, 0, 13), Call<float>(gta::GET_CONTROL_NORMAL, 0, 12),
                 Call<float>(gta::GET_DISABLED_CONTROL_NORMAL, 0, 13), Call<float>(gta::GET_DISABLED_CONTROL_NORMAL, 0, 12),
                 Call<float>(gta::GET_CONTROL_NORMAL, 0, 1), Call<float>(gta::GET_CONTROL_NORMAL, 0, 2));
        }
        // Without a GTA highlight that moves (no GTA task running on the ped),
        // the stick's direction names the wheel slice (clockwise
        // from the top: pistol, SMG/MG, rifle, sniper, melee, shotgun, heavy,
        // thrown) and the slice's weapon is the one GTA shows there.
        const float sx = pad.right_x / 32767.0f, sy = pad.right_y / 32767.0f;
        if (!s.wheelLitMoved && sx * sx + sy * sy > 0.25f) {
            static const Hash kSlice[8][2] = {{0x18D5FA97, 0x18D5FA97}, {0xC6E9A5C5, 0x451B04BC}, {0x39D5C192, 0x39D5C192},
                                              {0xB7BBD827, 0xB7BBD827}, {0xD49321D4, 0xA00FC1E4}, {0x33431399, 0x33431399},
                                              {0xA27A4F9F, 0xA27A4F9F}, {0x5C4C5883, 0x5C4C5883}};
            const float deg = std::atan2(sx, sy) * 57.29578f; // 0 = up, clockwise
            const int slice = static_cast<int>(std::lround((deg < 0 ? deg + 360.0f : deg) / 45.0f)) % 8;
            Hash pick = 0;
            for (int i = 0; i < 8 && !pick; ++i) {
                const Hash h = Call<Hash>(gta::HUD_GET_WEAPON_WHEEL_TOP_SLOT, i);
                const Hash g = h ? Call<Hash>(gta::GET_WEAPONTYPE_GROUP, h) : 0;
                if (h && (g == kSlice[slice][0] || g == kSlice[slice][1])) pick = h;
            }
            if (slice == 4 && !pick) pick = 0xA2719263; // WEAPON_UNARMED
            if (pick && pick != s.wheelPick) Logf("SkateV Legacy: weapon wheel stick slice %d -> 0x%08X", slice, static_cast<unsigned>(pick));
            if (pick) s.wheelPick = pick, s.wheelSlice = slice;
        }
        if (!s.wheelLitMoved && s.wheelSlice >= 0) {
            // GTA's highlight stays put here, so the pick is named on screen.
            static const char* const kName[8] = {"Pistols", "SMGs and machine guns", "Rifles", "Snipers", "Unarmed", "Shotguns", "Heavy", "Thrown"};
            Text((std::string("Weapon: ") + kName[s.wheelSlice]).c_str(), 0.5f, 0.78f, 0.5f, 255, 255, 255, true);
        }
        pad.right_x = pad.right_y = 0;
    } else if (s.armed && s.wheelClosedAt && tick - s.wheelClosedAt >= 150) {
        // What the wheel picked: unarmed (or the board itself) puts the gun
        // away; a weapon the board cannot use (sniper, homing launcher) goes
        // back to the last gun.
        s.wheelClosedAt = 0;
        const Hash picked = Call<Hash>(gta::GET_SELECTED_PED_WEAPON, s.ped);
        if (picked == 0xA2719263 || picked == kBoardWeapon) { // WEAPON_UNARMED
            Holster(s);
            Log("SkateV Legacy: gun away (weapon wheel)");
        } else if (!AimAllowed(picked)) {
            Call<void>(gta::SET_CURRENT_PED_WEAPON, s.ped, s.aimWeapon, 1);
            Notify(s, "That weapon takes the camera; kept your last gun", 2000);
        } else {
            s.aimWeapon = picked;
            Logf("SkateV Legacy: weapon wheel picked 0x%08X", static_cast<unsigned>(picked));
        }
    }

    // GunButton on release, unless LB was part of the hold (board chord).
    if (buttons & cfg.gunButton && buttons & 0x0100) s.gunChordUsed = true;
    if ((released & cfg.gunButton) && !(buttons & cfg.gunButton)) {
        if (!s.gunChordUsed) {
            if (s.armed) {
                Holster(s);
                Log("SkateV Legacy: gun away");
            } else if (!TakeOutGun(s)) {
                Notify(s, "No gun to use on the board (sniper rifles and the homing launcher take the camera)", 2500);
            }
        }
        s.gunChordUsed = false;
    }

    if (!s.armed) {
        if (pressed & 0x0004) StepRadio(s, 1);  // d-pad left: next station (GTA's car layout)
        if (pressed & 0x0008) StepRadio(s, -1); // d-pad right: previous
        return;
    }

    const std::string_view state(s.last.state_utf8);
    // GTA's own weapon icon and ammo count, and its reticle while aiming.
    Call<void>(gta::SHOW_HUD_COMPONENT_THIS_FRAME, 2); // HUD_WEAPON_ICON
    Call<void>(gta::DISPLAY_AMMO_THIS_FRAME, 1);
    // LB + d-pad up/down stays Skate's session marker (place / hold to return)
    // with the gun out: the d-pad takes the chord from the aim.
    const bool marker = (buttons & cfg.aimButton) && (buttons & 0x0003);
    const bool held = (buttons & cfg.aimButton) == cfg.aimButton && !marker;
    if (!s.aiming) {
        if (held && state.rfind("Wipeout", 0) != 0 && state.rfind("Biped", 0) != 0) {
            s.aiming = true;
            GameplayCam(s);
            Log("SkateV Legacy: board aim on");
        }
    } else if (!held || state.rfind("Wipeout", 0) == 0) {
        StopAim(s, true);
    }

    // GTA animates the upper body only while aiming (and not bailing); the rest
    // of the time the body is Skate's, the gun in its hand.
    posehook::SetGun(s.aiming && state.rfind("Wipeout", 0) != 0, kGunBlendMs);
    const Hash w = Call<Hash>(gta::GET_SELECTED_PED_WEAPON, s.ped);
    if (AimAllowed(w)) s.aimWeapon = w;
    // D-pad right/left: GTA's next/previous weapon, skipping the excluded ones.
    if (pressed & 0x0008) s.aimCycle = 1, s.aimCycleWait = 0, s.aimCycleSteps = 0;
    if (pressed & 0x0004) s.aimCycle = -1, s.aimCycleWait = 0, s.aimCycleSteps = 0;
    if (s.aimCycle) {
        const int action = s.aimCycle > 0 ? 16 : 17; // INPUT_SELECT_NEXT_WEAPON / PREV
        if (s.aimCycleWait > 0 && w == s.aimCycleFrom) {
            --s.aimCycleWait;
        } else if (s.aimCycleWait > 0 && AimAllowed(w)) {
            s.aimCycle = 0; // landed on an allowed weapon
        } else if (s.aimCycleSteps < 16) {
            Call<void>(gta::ENABLE_CONTROL_ACTION, 0, action, 1);
            Call<BOOL>(gta::SET_CONTROL_VALUE_NEXT_FRAME, 0, action, 1.0f);
            s.aimCycleFrom = w;
            s.aimCycleWait = 30;
            ++s.aimCycleSteps;
        } else {
            s.aimCycle = 0;
        }
    }
    pad.buttons &= static_cast<std::uint16_t>(~(cfg.fireButton | 0x000C | (marker ? 0 : cfg.aimButton)));
    if (cfg.gunProbe) GunProbe(s, s.last.state_utf8);
    if (!s.aiming) return;

    Call<void>(gta::SHOW_HUD_COMPONENT_THIS_FRAME, 14); // HUD_RETICLE
    // GTA's own aim: the ped's player gun task reads INPUT_AIM (LB stands in)
    // and aims along the gameplay camera.
    Call<BOOL>(gta::SET_CONTROL_VALUE_NEXT_FRAME, 0, 25, 1.0f);
    const DWORD now = GetTickCount();
    int clip = 0;
    Call<BOOL>(gta::GET_AMMO_IN_CLIP, s.ped, w, &clip);
    const int clipMax = Call<int>(gta::GET_MAX_AMMO_IN_CLIP, s.ped, w, 1);
    const int total = Call<int>(gta::GET_AMMO_IN_PED_WEAPON, s.ped, w);

    // Fire: INPUT_ATTACK while FireButton is held. GTA's shots are proven by
    // a shot or a falling clip; if none comes in 350 ms the shots go straight at
    // the reticle (direct fire) for the rest of the session.
    const bool fireHeld = (buttons & cfg.fireButton) == cfg.fireButton;
    if (fireHeld) {
        Call<BOOL>(gta::SET_CONTROL_VALUE_NEXT_FRAME, 0, 24, 1.0f);
        if (!s.fireWas) s.fireSince = now, s.fireClip0 = clip, s.fireSeen = false;
        if (s.nativeFire != -1 && (Call<BOOL>(gta::IS_PED_SHOOTING, s.ped) || clip < s.fireClip0)) {
            s.fireSeen = true;
            if (s.nativeFire != 1) Log("SkateV Legacy: board fire: GTA fires the shots");
            s.nativeFire = 1;
        } else if (s.nativeFire == 0 && !s.fireSeen && clip > 0 && now - s.fireSince > 350) {
            s.nativeFire = -1;
            Log("SkateV Legacy: board fire: GTA did not fire in 350 ms; direct shots from now on");
        }
    }
    s.fireWas = fireHeld;
    // Reload: B, or GTA's own empty-clip reload while firing.
    if (pressed & 0x2000) s.reloadPulse = 3;
    if (s.reloadPulse > 0) {
        Call<BOOL>(gta::SET_CONTROL_VALUE_NEXT_FRAME, 0, 45, 1.0f); // INPUT_RELOAD
        --s.reloadPulse;
    }

    if (s.nativeFire == -1) {
        // Direct fire. GTA's reload never runs either then: an empty clip, or B
        // with a part-used one, reloads (GTA's reload is asked for, for the
        // sound), firing waits, then the clip is filled from the ped's own ammo
        // (the total includes the clip: none is made).
        if (!s.reloadDone && total > clip && clip < clipMax && (clip == 0 || (pressed & 0x2000))) {
            Call<BOOL>(gta::MAKE_PED_RELOAD, s.ped);
            s.reloadDone = now + 1500;
            Logf("SkateV Legacy: board reload (clip %d of %d, total %d)", clip, clipMax, total);
        } else if (s.reloadDone && static_cast<int>(now - s.reloadDone) >= 0) {
            s.reloadDone = 0;
            Call<BOOL>(gta::GET_AMMO_IN_CLIP, s.ped, w, &clip);
            if (clip < clipMax) Call<BOOL>(gta::SET_AMMO_IN_CLIP, s.ped, w, std::min(clipMax, total));
        }
        if (!s.reloadDone && clip > 0 && fireHeld && static_cast<int>(now - s.nextShot) >= 0) {
            // Target: what the reticle (screen centre) covers, from past the rider.
            const Vector3 cam = Call<Vector3>(gta::GET_GAMEPLAY_CAM_COORD);
            const Vector3 rot = Call<Vector3>(gta::GET_GAMEPLAY_CAM_ROT, 2);
            const float pr = rot.x * 0.01745329f, hr = rot.z * 0.01745329f;
            const float dx = -std::sin(hr) * std::cos(pr), dy = std::cos(hr) * std::cos(pr), dz = std::sin(pr);
            const Vector3 me = Call<Vector3>(gta::GET_ENTITY_COORDS, s.ped, 1);
            float along = (me.x - cam.x) * dx + (me.y - cam.y) * dy + (me.z - cam.z) * dz + 1.0f;
            if (along < 0.5f) along = 0.5f;
            float target[3] = {cam.x + dx * 300.0f, cam.y + dy * 300.0f, cam.z + dz * 300.0f};
            const int test = Call<int>(gta::START_EXPENSIVE_SYNCHRONOUS_SHAPE_TEST_LOS_PROBE, cam.x + dx * along,
                                       cam.y + dy * along, cam.z + dz * along, target[0], target[1], target[2],
                                       1 | 2 | 4 | 8 | 16 | 256, s.ped, 7);
            BOOL hit = 0;
            Vector3 end{}, normal{};
            Entity entity = 0;
            if (Call<int>(gta::GET_SHAPE_TEST_RESULT, test, &hit, &end, &normal, &entity) == 2 && hit)
                target[0] = end.x, target[1] = end.y, target[2] = end.z;
            Call<void>(gta::SET_PED_SHOOTS_AT_COORD, s.ped, target[0], target[1], target[2], 1);
            // Scripted shots from the held ped never reach GTA's crime system:
            // shots fired, once a second.
            if (now - s.shotsReportedAt >= 1000) s.shotsReportedAt = now, crime::Report(crime::kShotsFired);
            const float gap = Call<float>(gta::GET_WEAPON_TIME_BETWEEN_SHOTS, w);
            s.nextShot = now + static_cast<DWORD>(gap > 0.05f && gap < 5.0f ? gap * 1000.0f : 100.0f);
        }
    }
    pad.buttons &= 0x1000; // A: push
    pad.left_trigger = pad.right_trigger = 0;
    pad.right_x = pad.right_y = 0;
}

// Peds the skater has shot (GTA marks them damaged by the player ped) become
// assaults with a deadly weapon, officer shot for cops, once per ped. Four
// times a second while skating, from GTA's nearby-ped list.
void ReportShootings(Session& s) {
    const DWORD now = GetTickCount();
    if (now - s.crimeScanAt < 250) return;
    s.crimeScanAt = now;
    constexpr int kMax = 32;
    int peds[2 + kMax * 2] = {kMax}; // script array: count, then 8-byte slots
    const int n = Call<int>(gta::GET_PED_NEARBY_PEDS, s.ped, peds, -1);
    for (int i = 0; i < std::min(n, kMax); ++i) {
        const int ped = peds[2 + i * 2];
        if (!ped || std::find(s.crimeVictims.begin(), s.crimeVictims.end(), ped) != s.crimeVictims.end()) continue;
        if (!Call<BOOL>(gta::HAS_ENTITY_BEEN_DAMAGED_BY_ENTITY, ped, s.ped, 1)) continue;
        const bool cop = crime::IsCop(ped);
        crime::Report(cop ? crime::kShootCop : crime::kShootPed);
        if (s.crimeVictims.size() >= 64) s.crimeVictims.erase(s.crimeVictims.begin());
        s.crimeVictims.push_back(ped);
        Logf("SkateV Legacy: crime reported: %s %d damaged by the skater (wanted %d)", cop ? "officer" : "ped", ped,
             Call<int>(gta::GET_PLAYER_WANTED_LEVEL, Call<Player>(gta::PLAYER_ID)));
    }
}

void UpdateActive(Session& s, const Config& cfg, const SvPad& pad) {
    DisableSkateControls(pad.buttons, s.aiming);
    if (s.nativeFire != 1) ReportShootings(s); // GTA reports the crimes of its own shots
    {
        // Riding into another map state's reach (twice a second).
        static DWORD lastStates = 0;
        const DWORD now = GetTickCount();
        if (now - lastStates >= 500) {
            lastStates = now;
            const Vector3 at = Call<Vector3>(gta::GET_ENTITY_COORDS, Call<Ped>(gta::PLAYER_PED_ID), 1);
            SendMapStates({at.x, at.y, at.z}, false);
        }
    }
    Call<void>(gta::ENABLE_CONTROL_ACTION, 0, 199, 1); // INPUT_FRONTEND_PAUSE
    Call<void>(gta::ENABLE_CONTROL_ACTION, 0, 200, 1); // INPUT_FRONTEND_PAUSE_ALTERNATE
    Call<void>(gta::INVALIDATE_IDLE_CAM);
    Call<void>(gta::INVALIDATE_CINEMATIC_VEHICLE_IDLE_MODE);

    SvInput in{};
    in.size = sizeof(in);
    // GTA's own game time step, so slow motion slows Skate too. Not under the
    // weapon wheel's 0.1x: Skate steps at a fixed rate, so a tenth of the time
    // is a few steps a second, and the skater, the ped and the camera on it
    // skip against a smooth world. Wall time there.
    {
        static LARGE_INTEGER s_freq{}, s_last{};
        if (!s_freq.QuadPart) QueryPerformanceFrequency(&s_freq);
        LARGE_INTEGER now;
        QueryPerformanceCounter(&now);
        const float wall = s_last.QuadPart ? static_cast<float>(now.QuadPart - s_last.QuadPart) / static_cast<float>(s_freq.QuadPart) : 0.0f;
        s_last = now;
        const float real = Call<float>(gta::GET_FRAME_TIME);
        const float game = Call<float>(gta::TIMESTEP);
        in.dt_seconds = std::isfinite(game) && game >= 0.0f && game <= real * 1.01f ? game : real;
        if (s.wheelOpen && wall > 0.0f && wall < 0.25f) in.dt_seconds = wall;
    }
    in.aspect_ratio = Call<float>(gta::GET_ASPECT_RATIO, 0);
    in.pad = pad;
    BoardControls(s, cfg, in.pad);
    if (cfg.dynamicWorld) {
        // Moving GTA entities near the skater, for Skate's contact code.
        s.world.SetPedParts(cfg.pedHitboxes);
        s.world.SetPedLaunch(cfg.pedLaunch, cfg.pedLaunchScale, cfg.pedLaunchLift, cfg.pedLaunchSpin);
        s.world.SetPedVoices(cfg.pedPain, cfg.pedGetUpSpeech);
        s.world.SetVehicleDamage(cfg.vehicleDamage, cfg.vehicleDamageScale, cfg.vehicleDamageMin, cfg.vehicleDamageRadius);
        s.world.Gather(s.ped, s.last.board_position, cfg.dynamicRadius, boardnative::Entity());
        const auto& bodies = s.world.Bodies();
        g_runtime.SetDynamicBodies(bodies.data(), static_cast<std::uint32_t>(bodies.size()));
        NearMiss(s, cfg, bodies);
        if (cfg.pedHitboxes) {
            const auto& parts = s.world.PedParts();
            g_runtime.SetPedParts(parts.data(), static_cast<std::uint32_t>(parts.size()));
        }
    }
    g_runtime.Step(in);

    SvOutput o{};
    if (!g_runtime.Output(o) || o.tick == 0) {
        pedcollision::Tick(s.ped, nullptr, 0);
        return;
    }
    s.last = o;

    {
        // The skater's own voice, in GTA's pattern for a fall: the falling
        // scream on a long drop, pain on each hard hit (ImpactGrunts), then a
        // line once back up.
        const bool wipeout = std::string_view(o.state_utf8).rfind("Wipeout", 0) == 0;
        const DWORD now = GetTickCount();
        if (wipeout && !s.wipingOut) {
            s.bailPos = o.skater_position, s.bailTick = o.tick, s.bailVelValid = false, s.fallScream = false;
            s.lineAt = 0;
        } else if (wipeout && o.tick > s.bailTick) {
            const float dt = static_cast<float>(o.tick - s.bailTick) / 60.0f;
            const SvVec3 v{(o.skater_position.x - s.bailPos.x) / dt, (o.skater_position.y - s.bailPos.y) / dt,
                           (o.skater_position.z - s.bailPos.z) / dt};
            if (s.bailVelValid && dt < 0.1f) {
                if (!s.fallScream && v.z < -cfg.bailScreamSpeed) {
                    s.fallScream = true;
                    pedspeech::Pain(s.ped, 1 /*AUD_DAMAGE_REASON_FALLING*/, 0.0f);
                }
            }
            s.bailVel = v, s.bailVelValid = dt < 0.1f;
            s.bailPos = o.skater_position, s.bailTick = o.tick;
        } else if (!wipeout && s.wipingOut) {
            s.lineAt = cfg.getUpSpeech.empty() ? 0 : now + 1200;
        }
        if (s.lineAt && now >= s.lineAt) {
            s.lineAt = 0;
            const std::string said = pedspeech::Say(s.ped, cfg.getUpSpeech);
            Logf("SkateV Legacy: bail over (%s): %s", o.state_utf8, said.empty() ? "no listed context in this voice" : said.c_str());
        }
        s.wipingOut = wipeout;
    }

    const SvVec3 p = o.skater_position;
    // keepTasks, keepIK, doWarp (CitizenFX docs; the pinned native DB calls
    // them axis flags). keepTasks 0 removed every task each tick: the live
    // clip, a vanilla clip and the aim task never ran. As SkateGTA-B4.
    if (s.placedValid) {
        // Moved by something else (a teleport): leave Skate where the ped went, then
        // come back once the collision there is loaded (it was not built for the new
        // place, so Skate's skater would fall through the floor).
        const Vector3 at = Call<Vector3>(gta::GET_ENTITY_COORDS, s.ped, 1);
        const float dx = at.x - s.placed[0], dy = at.y - s.placed[1], dz = at.z - s.placed[2];
        if (dx * dx + dy * dy + dz * dz > 25.0f && std::isfinite(at.x + at.y + at.z)) {
            Logf("SkateV Legacy: ped moved %.0f m by something else (a teleport): Skate follows", std::sqrt(dx * dx + dy * dy + dz * dz));
            s.keepPlace = true;
            Leave(s, cfg, "teleported", false);
            s.keepPlace = false;
            s.teleportWait = true;
            TryEnter(s, cfg, false);
            return;
        }
    }
    Call<void>(gta::SET_ENTITY_COORDS_NO_OFFSET, s.ped, p.x, p.y, p.z + cfg.pedZOffset, 1, 1, 0);
    s.placed[0] = p.x, s.placed[1] = p.y, s.placed[2] = p.z + cfg.pedZOffset, s.placedValid = true;
    if (cfg.posePed) {
        // The written pose is world space, made relative to the entity's live
        // matrix, so the entity can face where the skater faces (else the
        // minimap and markers keep the entry heading). It turns at a bounded
        // rate: an instant 180 degree turn shows the body flipped for a frame.
        float target = s.facing;
        const float dt = in.dt_seconds > 0.0f && in.dt_seconds < 0.25f ? in.dt_seconds : 0.0f;
        if (s.aiming) {
            // The frozen ped cannot turn into its aim: face the camera, at most
            // AimTwist degrees from the stance (the legs are written in world
            // space, so only the torso turns; shots still go to the reticle).
            float aim = Call<Vector3>(gta::GET_GAMEPLAY_CAM_ROT, 2).z;
            if (std::isfinite(s.poseHeading)) {
                const float d = std::fmod(aim - s.poseHeading + 540.0f, 360.0f) - 180.0f;
                aim = s.poseHeading + std::max(-cfg.aimTwist, std::min(cfg.aimTwist, d));
            }
            s.facing = TurnToward(s.facing, std::fmod(aim + 360.0f, 360.0f), 4.0f * kFacingTurnDegPerSec * dt);
        } else if (SkaterFacing(s, o, target)) {
            s.facing = TurnToward(s.facing, target, kFacingTurnDegPerSec * dt);
        }
        Call<void>(gta::SET_ENTITY_HEADING, s.ped, s.facing);
    } else {
        // GTA's ped forward is opposite Skate's published skater heading.
        Call<void>(gta::SET_ENTITY_HEADING, s.ped, o.skater_heading_degrees + 180.0f);
    }
    if (cfg.posePed) {
        SetGtaPosing(s.ped, false);
        if (s.armed) GunIk(s.ped, s.aiming);
        // Target the live skeleton (re-resolved every second; it is stable
        // while the ped exists) and hand the hook the runtime's pose.
        static DWORD s_nextResolve = 0;
        static int s_bones = 0;
        static std::uintptr_t s_skeleton = 0, s_boneData = 0;
        static int s_hitBones[3] = {-1, -1, -1};
        static int s_feet[4] = {-1, -1, -1, -1};
        if (GetTickCount() >= s_nextResolve) {
            s_nextResolve = GetTickCount() + 1000;
            probe::SkeletonInfo sk;
            const char* why = "";
            if (probe::ResolvePedSkeleton(s.ped, sk, why)) {
                if (posehook::Install(reinterpret_cast<std::uintptr_t>(getScriptHandleBaseAddress(s.ped)), sk, Log) &&
                    !s.poseFadedIn) {
                    posehook::FadeIn(kHandOffMs);
                    s.poseFadedIn = true;
                }
                const bool same = sk.bones == s_boneData && sk.count == s_bones;
                s_bones = sk.count;
                s_skeleton = sk.skeleton;
                s_boneData = sk.bones;
                if (!same) { // bone indices only change with the skeleton data
                    s_hitBones[0] = probe::BoneIndexByTag(sk, 11816); // pelvis
                    s_hitBones[1] = probe::BoneIndexByTag(sk, 24818); // spine3
                    s_hitBones[2] = probe::BoneIndexByTag(sk, 31086); // head
                    s_feet[0] = probe::BoneIndexByTag(sk, 14201); // SKEL_L_Foot
                    s_feet[1] = probe::BoneIndexByTag(sk, 2108);  // SKEL_L_Toe0
                    s_feet[2] = probe::BoneIndexByTag(sk, 52301); // SKEL_R_Foot
                    s_feet[3] = probe::BoneIndexByTag(sk, 20781); // SKEL_R_Toe0
                }
            } else {
                s_bones = 0;
                s_boneData = 0;
            }
        }
        static std::vector<float> s_pose(512 * 16);
        const std::uint32_t n = g_runtime.CharacterPose(s_pose.data(), 512);
        const bool posed = n > 0 && static_cast<int>(n) == s_bones;
        if (posed) {
            posehook::PublishPose(s_pose.data(), static_cast<int>(n));
            ImpactGrunts(s, cfg, s_pose.data(), s_hitBones, o.tick);
            float h;
            if (PoseFacing(s_pose.data(), s_feet, h)) s.poseHeading = h;
        }
        pedcollision::Tick(s.ped, posed ? s_pose.data() : nullptr, n, g_runtime.SkitchVehicle());
        if (cfg.liveClip && s_skeleton) liveclip::Tick(s.ped, s_skeleton, Log);
    }

    if (o.camera_valid) {
        Call<void>(gta::SET_CAM_COORD, s.cam, o.camera_position.x, o.camera_position.y, o.camera_position.z);
        Call<void>(gta::SET_CAM_ROT, s.cam, o.camera_rotation.x, o.camera_rotation.y, o.camera_rotation.z, 2);
        float fov = o.camera_fov * cfg.fovScale;
        fov = fov < 20.0f ? 20.0f : (fov > 120.0f ? 120.0f : fov);
        Call<void>(gta::SET_CAM_FOV, s.cam, fov);
    } else {
        // Skate camera not published yet: plain chase view behind the skater.
        const float h = o.skater_heading_degrees * 0.01745329f;
        Call<void>(gta::SET_CAM_COORD, s.cam, p.x + std::sin(h) * 4.0f, p.y - std::cos(h) * 4.0f, p.z + 1.8f);
        Call<void>(gta::SET_CAM_ROT, s.cam, -12.0f, 0.0f, o.skater_heading_degrees, 2);
        Call<void>(gta::SET_CAM_FOV, s.cam, 60.0f);
    }

    if (cfg.dynamicWorld) {
        SvDynamicHit hits[16];
        const std::uint32_t n = g_runtime.DynamicHits(hits, 16);
        s.world.SetExchange(true);
        s.world.React(hits, n, o.velocity);
        s.world.SpeakGetUps(Log);
        s.world.KickDebris(o.board_position, o.velocity);
        s.world.OpenDoors(o.skater_position);
        // Contact exchange: Skate's solved impulses on GTA vehicles/props.
        SvDynamicImpulse impulses[64];
        const std::uint32_t k = g_runtime.DynamicImpulses(impulses, 64);
        s.world.ApplyImpulses(impulses, k, Log);
        static DWORD lastWorldLog = 0;
        const DWORD now = GetTickCount();
        if (util::g_verbose && now - lastWorldLog >= 10000) {
            lastWorldLog = now;
            const auto& w = s.world.Stats();
            Logf("SkateV Legacy: dynamic collision exact=%u missingTemplates=%u vehicles=%u peds=%u (as ragdoll parts %u, %zu parts) objects=%u doors=%u contacts=%u exchanged=%u (ped impulses %u)",
                 w.exactBounds, w.missingBounds, w.vehicles, w.peds, w.pedParts, s.world.PedParts().size(),
                 w.objects, w.doors, w.hits, w.exchanged, w.pedImpulses);
        }
    }

    SvBoardPose boardPose{};
    const bool nativeBoard = cfg.drawBoard && g_runtime.BoardPose(boardPose)
        && boardnative::Tick(true, boardPose);
    if (!cfg.drawBoard || boardPose.bone_count != 7) boardnative::Tick(false, SvBoardPose{});
    if (cfg.drawSkater && !nativeBoard) DrawBoardMesh();
    if (g_quirkRequested.exchange(false) && cfg.backwardsMan) g_runtime.TriggerQuirk(0);
    SvQuirkState quirk{};
    if (g_runtime.QuirkState(quirk) && quirk.assist_phase != 0) {
        Text(quirk.assist_phase == 6 ? "BACKWARDS MAN" : "backwards man...", 0.5f, 0.2f, 0.55f, 255, 120, 40, true);
    }
    if (cfg.showDebug) { // dev only; the player sees hud_overlay
        SvScoreState score{};
        g_runtime.Score(score);
        DrawHud(o, score, true);
    }
}

// One script frame of the lifecycle state machine (lifecycle.h). `f6`: the
// dev shortcut, mapped onto the same transitions (enter without the
// controller confirmation window; leave immediately, forced).
void LifecycleFrame(Session& s, const Config& cfg, bool f6) {
    const DWORD now = GetTickCount();
    const Ped player = Call<Ped>(gta::PLAYER_PED_ID);
    if (s.teleportWait && s.state != LifeState::Waiting) {
        // After a teleport: Skate took the ped back (it is held) or the wait ended (GTA's again).
        s.teleportWait = false;
        if (!SkateOwnsPlayer(s) && Call<BOOL>(gta::DOES_ENTITY_EXIST, player)) Call<void>(gta::FREEZE_ENTITY_POSITION, player, 0);
    }
    // Hand-off tails: Skate's camera goes once GTA's has eased in; the pose
    // hook once Skate's pose has faded into GTA's (or the fade cannot run).
    if (s.fadingCam && static_cast<int>(now - s.fadingCamUntil) >= 0) {
        if (Call<BOOL>(gta::DOES_CAM_EXIST, s.fadingCam)) {
            Call<void>(gta::SET_CAM_ACTIVE, s.fadingCam, 0);
            Call<void>(gta::DESTROY_CAM, s.fadingCam, 0);
        }
        s.fadingCam = 0;
    }
    if (s.poseFadingOut && !SkateOwnsPlayer(s) &&
        (posehook::FadeOutDone() || now - s.poseFadeOutSince > static_cast<DWORD>(kHandOffMs) * 4 ||
         !Call<BOOL>(gta::DOES_ENTITY_EXIST, s.ped))) {
        posehook::Uninstall(Log);
        s.poseFadingOut = false;
    }
    switch (s.state) {
    case LifeState::Unprepared: {
        const bool loaded = Call<BOOL>(gta::DOES_ENTITY_EXIST, player) &&
                            Call<BOOL>(gta::IS_PLAYER_PLAYING, Call<Player>(gta::PLAYER_ID)) &&
                            !Call<BOOL>(gta::GET_IS_LOADING_SCREEN_ACTIVE) && Call<BOOL>(gta::IS_SCREEN_FADED_IN) &&
                            !Call<BOOL>(gta::IS_PLAYER_SWITCH_IN_PROGRESS);
        if (!loaded) {
            s.loadedSince = 0;
            if (f6) Notify(s, "game still loading", 2000);
            break;
        }
        if (s.loadedSince == 0) s.loadedSince = now;
        if (now - s.loadedSince < 1000) break; // player stable for a second
        if (cfg.life.backgroundPrepare) {
            Track(s, true);
            SetState(s, LifeState::Preparing, "player loaded: preparing Skate in the background");
        } else {
            SetState(s, LifeState::Ready, "BackgroundPrepare=0: Skate prepares at the first board action");
        }
        break;
    }
    case LifeState::Preparing:
    case LifeState::Ready: {
        if (cfg.life.backgroundPrepare || s.trackingStarted) Track(s, false);
        if (s.state == LifeState::Preparing) {
            SvLifecycleState l{};
            if (g_runtime.LifecycleState(l) && l.prepared) {
                Logf("SkateV Legacy: Skate session prepared in the background in %.0f ms", l.last_prepare_ms);
                SetState(s, LifeState::Ready, "Skate prepared");
            }
        }
        const SvPad pad = ReadPad();
        // After leaving, the Y that dismounted must not become GTA's
        // INPUT_ENTER (vehicle entry) while it is still held.
        if (now < s.suppressEnterUntil) {
            Call<void>(gta::DISABLE_CONTROL_ACTION, 0, 23, 1); // INPUT_ENTER
            if (pad.buttons & 0x8000) s.suppressEnterUntil = now + 100;
        }
        const bool wheel = BoardWeaponTick(s, player);
        if (wheel) Log("SkateV Legacy: board picked in the weapon wheel");
        if (f6 || wheel) TryEnter(s, cfg, !f6);
        // Refused (vehicle, water, ...): no invisible weapon left in hand.
        if (wheel && s.state == LifeState::Ready) Call<void>(gta::SET_CURRENT_PED_WEAPON, player, 0xA2719263, 1);
        break;
    }
    case LifeState::Waiting: {
        // Board action accepted but the session / this area is still being
        // built: GTA keeps the player meanwhile; a second press or the menu cancels.
        Track(s, false);
        if (s.teleportWait) {
            // After a teleport: GTA streams collision here; the player stays put until it has.
            const Vector3 here = Call<Vector3>(gta::GET_ENTITY_COORDS, player, 1);
            Call<void>(gta::REQUEST_COLLISION_AT_COORD, here.x, here.y, here.z);
            Call<void>(gta::FREEZE_ENTITY_POSITION, player, 1);
        }
        BoardWeaponTick(s, player); // keeps the stand-in hidden while the area builds
        if (f6) {
            SetState(s, LifeState::Ready, "cancelled by the player");
            break;
        }
        if (const char* why = lifecycle::Blocked(player, false, cfg.life)) {
            SetState(s, LifeState::Ready, why);
            break;
        }
        // A first take-out waits for the runtime's preparation, which waits for GTA to stream the area
        // (a hard disk: past 45 s, 2026-10-09); the shorter limit is for an area that never loads.
        SvLifecycleState life{};
        const bool preparing = g_runtime.LifecycleState(life) && !life.prepared;
        if (now - s.stateSince > (preparing ? 180000u : 45000u)) {
            Notify(s, "Skate could not load this area in time", 3000);
            SetState(s, LifeState::Ready, "waiting timed out");
            break;
        }
        const char* why = nullptr;
        if (AreaReady(why)) {
            if (!lifecycle::Blocked(player, true, cfg.life)) TryEnter(s, cfg, true);
        } else if (now - s.stateSince > 300) {
            Text((std::string("SK8V: ") + why + "...").c_str(), 0.5f, 0.88f, 0.4f, 255, 255, 255, true);
        }
        break;
    }
    case LifeState::Entering: {
        if (player != s.ped || !Call<BOOL>(gta::DOES_ENTITY_EXIST, s.ped)) {
            Leave(s, cfg, "player character changed or removed", false);
            break;
        }
        DisableSkateControls(ReadPad().buttons, false);
        SvOutput o{};
        g_runtime.Output(o);
        const bool dead = Call<BOOL>(gta::IS_ENTITY_DEAD, s.ped, 0);
        if (o.status == SV_STATUS_ERROR) {
            Leave(s, cfg, "runtime error", false);
            Notify(s, "Skate runtime error: " + g_runtime.StatusText(), 8000);
        } else if (dead || Call<BOOL>(gta::NETWORK_IS_SESSION_STARTED)) {
            Leave(s, cfg, dead ? "player died" : "network session", false);
        } else if (f6) {
            Leave(s, cfg, "menu", false);
        } else if (o.status == SV_STATUS_ACTIVE) {
            BeginActive(s, cfg);
        } else if (now - s.stateSince > 60000) {
            Leave(s, cfg, "activation timed out", false);
        } else if (now - s.stateSince > 300) {
            Text(("SK8V: " + g_runtime.StatusText()).c_str(), 0.5f, 0.45f, 0.5f, 255, 255, 255, true);
        }
        break;
    }
    case LifeState::Skating: {
        const SvPad pad = ReadPad(); // once per frame
        // Hold Y to put the board away. The press still reaches Skate, so
        // it steps off first, then puts the board away while Y stays down.
        // Only a press made while skating counts; a Y held at entry does not.
        const bool yDown = (pad.buttons & 0x8000) != 0;
        if (yDown && !s.yWasDown) s.yHeldSince = now;
        if (!yDown) s.yHeldSince = 0;
        s.yWasDown = yDown;
        const bool heldAway = cfg.putAwayHoldMs && s.yHeldSince && now - s.yHeldSince >= cfg.putAwayHoldMs;
        if (player != s.ped || !Call<BOOL>(gta::DOES_ENTITY_EXIST, s.ped)) {
            Leave(s, cfg, "player character changed or removed", false);
            break;
        }
        SvOutput o{};
        g_runtime.Output(o);
        const bool dead = Call<BOOL>(gta::IS_ENTITY_DEAD, s.ped, 0);
        if (o.status == SV_STATUS_ERROR) {
            Leave(s, cfg, "runtime error", false);
            Notify(s, "Skate runtime error: " + g_runtime.StatusText(), 8000);
        } else if (dead || Call<BOOL>(gta::NETWORK_IS_SESSION_STARTED)) {
            Leave(s, cfg, dead ? "player died" : "network session", false);
        } else if (f6 || heldAway) {
            s.yHeldSince = 0, s.yWasDown = true;
            Leave(s, cfg, heldAway ? "board put away (Y held)" : "menu", true);
        } else if (!CheckDelegation(s, cfg, pad)) {
            UpdateActive(s, cfg, pad);
            // Y mounts/dismounts inside Skate. Holding Y or the menu puts the
            // board away and returns input/camera authority to GTA.
        }
        break;
    }
    case LifeState::Leaving:
        SetState(s, LifeState::Ready, "left");
        break;
    case LifeState::Delegated: {
        // GTA owns the player until it is back on foot; the menu,
        // a vehicle, death or a long action keep GTA in charge for good.
        Track(s, false);
        if (f6) {
            SetState(s, LifeState::Ready, "delegation ended by the player");
            break;
        }
        if (player != s.ped || !Call<BOOL>(gta::DOES_ENTITY_EXIST, player) || Call<BOOL>(gta::IS_ENTITY_DEAD, player, 0) ||
            Call<BOOL>(gta::IS_PED_IN_ANY_VEHICLE, player, 0) || now - s.delegatedAt > 30000) {
            SetState(s, LifeState::Ready, "delegation ended");
            break;
        }
        bool busy = Call<BOOL>(gta::IS_PED_FALLING, player) || Call<BOOL>(gta::IS_PED_JUMPING, player) ||
                    Call<BOOL>(gta::IS_PED_IN_PARACHUTE_FREE_FALL, player);
        if (s.delegation == Session::Delegation::Swim) {
            busy = busy || Call<BOOL>(gta::IS_PED_SWIMMING, player) ||
                   (Call<BOOL>(gta::IS_ENTITY_IN_WATER, player) && Call<float>(gta::GET_ENTITY_SUBMERGED_LEVEL, player) > 0.25f);
        } else {
            constexpr Hash climbTask = util::Joaat("SCRIPT_TASK_CLIMB");
            busy = busy || Call<BOOL>(gta::IS_PED_CLIMBING, player) || Call<BOOL>(gta::IS_PED_VAULTING, player) ||
                   Call<int>(gta::GET_SCRIPT_TASK_STATUS, player, climbTask) != 7;
        }
        if (busy) {
            s.settledSince = 0;
            break;
        }
        if (!s.settledSince) s.settledSince = now;
        if (now - s.settledSince >= 250 && !lifecycle::Blocked(player, true, cfg.life)) {
            s.delegation = Session::Delegation::None;
            TryEnter(s, cfg, true);
        }
        break;
    }
    }
}

// ---- settings menu ---------------------------------------------------------
// Every item is a [SkateV] INI key (menu.h): written when changed, applied live
// here unless the item is marked restart (read once at startup). Live items
// store through the same key table as LoadConfig (Bound).
bool g_hallOfMeatOn = true; // the Hall of Meat setting as the runtime holds it

unsigned RawPadButtons() { return ReadPadRaw().buttons; }

// Live item: the new INI text goes into g_config through the key table, then `then`.
menu::Apply Bound(const char* key, std::function<void()> then = {}) {
    const Key* k = FindKey(key);
    return [k, then](const std::string& v) {
        if (k) k->store(g_config, v);
        if (then) then();
    };
}

void ApplyQuirk() {
    const Config& c = g_config;
    SvQuirkConfig q{};
    q.size = sizeof(q);
    q.enabled = c.backwardsMan ? 1u : 0u;
    q.chord = c.backwardsManChord;
    q.remount_delay = c.backwardsManRemountDelay;
    q.model = 2; // Retail, the only model the runtime has
    q.backward = c.backwardsManBackward ? 1u : 0u;
    g_runtime.ConfigureQuirk(q);
}

std::function<std::string(float)> Fixed(const char* format) {
    return [format](float v) {
        char buf[48];
        std::snprintf(buf, sizeof(buf), format, v);
        return std::string(buf);
    };
}

// 0 = none, -1 = Skate 3's own (BailTimeLimit / AirTimeLimit).
std::string LimitText(float v) {
    return v < 0.0f ? "Skate's own" : v == 0.0f ? "None" : Fixed("%g s")(v);
}

std::vector<menu::Page> MenuPages(Session& s) {
    using namespace menu;
    const Config D{};
    std::vector<Page> pages;

    pages.push_back({"Skating", {
        Action("Skate on / off", [] { g_toggleRequested.store(true); }),
        Choice("Stance", "Stance", {"Regular", "Goofy"}, {"Regular", "Goofy"}, {}, true),
        Choice("Difficulty", "Difficulty", {"Easy", "Normal", "Hardcore", "Motorized"},
               {"Easy", "Normal", "Hardcore", "Motorized (RB motor, no bad-landing bails)"},
               Bound("Difficulty", [] { g_runtime.SetDifficulty(g_config.difficulty); })),
        Flag("Ramp lip rule", "LipRule", D.lipRule, Bound("LipRule", [] { g_runtime.SetLipRule(g_config.lipRule); })),
        Number("Air time limit", "AirTimeLimit", D.airTimeLimit, -1, 120, 5, LimitText,
               Bound("AirTimeLimit", [] { g_runtime.SetAirLimit(g_config.airTimeLimit); })),
        Number("Bail time limit", "BailTimeLimit", D.bailTimeLimit, -1, 60, 1, LimitText,
               Bound("BailTimeLimit", [] { g_runtime.SetBailLimit(g_config.bailTimeLimit); })),
        Number("Skitch standoff", "SkitchStandoff", D.skitchStandoff, 0, 1, 0.05f, Fixed("%.2f m"),
               Bound("SkitchStandoff", [] { g_runtime.SetSkitchStandoff(g_config.skitchStandoff); })),
        Flag("Swim and climb with GTA", "GtaDelegation", D.delegation, Bound("GtaDelegation")),
        Number("Swim depth", "SwimDepth", D.swimDepth, 0.3f, 2, 0.1f, Fixed("%.1f m"), Bound("SwimDepth")),
        Number("Hold Y to put the board away", "PutAwayHoldMs", static_cast<float>(D.putAwayHoldMs), 0, 2000, 100,
               [](float v) { return v == 0.0f ? std::string("Off") : Fixed("%.0f ms")(v); }, Bound("PutAwayHoldMs")),
        Flag("Shooting on the board", "BoardAim", D.boardAim, Bound("BoardAim")),
        Number("Camera FOV scale", "CameraFovScale", D.fovScale, 0.5f, 1.5f, 0.05f, Fixed("x%.2f"), Bound("CameraFovScale")),
    }});

    pages.push_back({"Hall of Meat and records", {
        Flag("Hall of Meat", "HallOfMeat", D.hallOfMeat, [&s](const std::string& v) {
            const bool on = std::atof(v.c_str()) != 0.0;
            if (g_runtime.IsLoaded() && g_runtime.SetHallOfMeat(on)) {
                g_hallOfMeatOn = g_config.hallOfMeat = on;
            } else {
                Notify(s, "Hall of Meat toggle needs the current SkateVRuntime.dll", 2500);
            }
        }),
        Flag("Show every metric panel", "HallOfMeatMetrics", D.hallOfMeatMetrics, {}, true),
        Flag("Broken-bone x-ray", "HallOfMeatXray", true, {}, true),
        Choice("Skater grunts on hits", "BailPain", {"13", "33", "16", "-1"}, {"Post-fall grunt (13)", "Pain (33)", "Small grunt (16)", "Off"}, Bound("BailPain")),
        Choice("Skater get-up line", "GetUpSpeech", {D.getUpSpeech, "0"}, {"On", "Off"}, Bound("GetUpSpeech")),
        Choice("Skater near-miss line", "NearMissSpeech", {D.nearMissSpeech, "0"}, {"On", "Off"}, Bound("NearMissSpeech")),
        Action("Show records board", [] { recordsui::NextPage(); }),
    }});

    pages.push_back({"Peds and vehicles", {
        Flag("Nearby entities collide", "DynamicWorld", D.dynamicWorld, Bound("DynamicWorld")),
        Number("Collision radius", "DynamicRadius", D.dynamicRadius, 10, 80, 5, Fixed("%.0f m"), Bound("DynamicRadius")),
        Flag("Peds as body parts", "PedHitboxes", D.pedHitboxes, Bound("PedHitboxes")),
        Flag("Launch peds", "PedLaunch", D.pedLaunch, Bound("PedLaunch")),
        Number("Launch strength", "PedLaunchScale", D.pedLaunchScale, 0, 3, 0.1f, Fixed("x%.1f"), Bound("PedLaunchScale")),
        Number("Launch lift", "PedLaunchLift", D.pedLaunchLift, 0, 1, 0.1f, Fixed("%.1f"), Bound("PedLaunchLift")),
        Number("Launch tumble", "PedLaunchSpin", D.pedLaunchSpin, 0, 3, 0.1f, Fixed("x%.1f"), Bound("PedLaunchSpin")),
        Choice("Struck ped get-up line", "PedGetUpSpeech", {D.pedGetUpSpeech, "0"}, {"On", "Off"}, Bound("PedGetUpSpeech")),
        Flag("Dent vehicles", "VehicleDamage", D.vehicleDamage, Bound("VehicleDamage")),
        Number("Dent strength", "VehicleDamageScale", D.vehicleDamageScale, 0, 2, 0.1f, Fixed("x%.1f"), Bound("VehicleDamageScale")),
    }});

    pages.push_back({"BackwardsMan", {
        Flag("BackwardsMan assist", "BackwardsMan", D.backwardsMan, Bound("BackwardsMan", ApplyQuirk)),
        Choice("Direction", "BackwardsManDirection", {"Backward", "Forward"}, {"Backward", "Forward"},
               Bound("BackwardsManDirection", ApplyQuirk)),
        Number("Remount delay", "BackwardsManRemountDelay", static_cast<float>(D.backwardsManRemountDelay), 0, 30, 1, Fixed("%.0f"),
               Bound("BackwardsManRemountDelay", ApplyQuirk)),
        Action("Trigger now", [&s] {
            if (s.state == LifeState::Skating && g_config.backwardsMan) g_quirkRequested.store(true);
            else Notify(s, "Get on the board first (BackwardsMan must be on)", 2000);
        }),
    }});

    pages.push_back({"Audio", {
        Number("Skate volume", "AudioGtaGainDb", 1.4f, -30, 24, 1, Fixed("%+.1f dB"),
               [](const std::string& v) { gtaaudio::SetGainDb(static_cast<float>(std::atof(v.c_str()))); }),
        Number("Master gain", "AudioMasterGain", 1, 0, 2, 0.1f, Fixed("x%.1f"), {}, true),
        Choice("Output", "AudioOutput", {"Gta", "Off"}, {"GTA mixer", "Off"}, {}, true),
    }});

    pages.push_back({"Controller and HUD", {
        Chord("Open menu (with /)", "MenuButton", D.life.menuButton, true, Bound("MenuButton")),
        Chord("Gun out / away", "GunButton", D.gunButton, false, Bound("GunButton")),
        Chord("Aim", "AimButton", D.aimButton, false, Bound("AimButton")),
        Chord("Fire", "FireButton", D.fireButton, false, Bound("FireButton")),
        Chord("BackwardsMan", "BackwardsManChord", D.backwardsManChord, false, Bound("BackwardsManChord", ApplyQuirk)),
        Flag("Skate during missions", "BoardActionInMissions", D.life.inMissions, Bound("BoardActionInMissions")),
        Flag("Original Skate HUD", "Hud", true, {}, true),
    }});

    pages.push_back({"Dev", {
        Action("Play showcase line", [] { g_lineRequested.store(true); }),
        Action("Next showcase line", [] { g_lineNext.store(true); }),
        Action("Sound list overlay", [] { g_soundListToggle.store(true); }),
        Number("Gun IK (0 off, 1 arms, 2 arms+torso+head)", "GunIK", static_cast<float>(D.gunIk), 0, 2, 1, Fixed("%.0f"), Bound("GunIK")),
        Number("Aim twist limit", "AimTwist", D.aimTwist, 20, 180, 10, Fixed("%.0f deg"), Bound("AimTwist")),
        Flag("Log GTA's gun state", "GunProbe", D.gunProbe, Bound("GunProbe")),
        Flag("Debug text HUD", "ShowDebug", D.showDebug, Bound("ShowDebug")),
        Flag("Verbose log", "VerboseLog", D.verboseLog, Bound("VerboseLog", [] { util::g_verbose = g_config.verboseLog; g_runtime.SetVerboseLog(g_config.verboseLog); })),
        Number("Ped Z offset", "PedZOffset", D.pedZOffset, 0, 2, 0.05f, Fixed("%.2f m"), Bound("PedZOffset")),
        Number("Bail hit speed", "BailHitSpeed", D.bailHitSpeed, 0, 20, 0.5f, Fixed("%.1f m/s"), Bound("BailHitSpeed")),
        Number("Bail scream speed", "BailScreamSpeed", D.bailScreamSpeed, 0, 40, 1, Fixed("%.0f m/s"), Bound("BailScreamSpeed")),
        Number("Struck ped pain reason", "PedPain", static_cast<float>(D.pedPain), -1, 20, 1, Fixed("%.0f"), Bound("PedPain")),
        Number("Vehicle dent minimum", "VehicleDamageMin", D.vehicleDamageMin, 0, 2000, 50, Fixed("%.0f N s"), Bound("VehicleDamageMin")),
        Number("Vehicle dent radius", "VehicleDamageRadius", D.vehicleDamageRadius, 0, 500, 10, Fixed("%.0f"), Bound("VehicleDamageRadius")),
        Flag("Prepare Skate at load", "BackgroundPrepare", D.life.backgroundPrepare, {}, true),
        Flag("Load Skate runtime", "Runtime", D.loadRuntime, {}, true),
        Flag("Native board", "BoardNative", true, {}, true),
        Flag("Draw the board", "DrawBoard", D.drawBoard, {}, true),
        Flag("Pose the GTA ped", "PosePed", D.posePed, {}, true),
        Flag("Ped pose collision", "PedPoseCollision", false, {}, true),
        Flag("Owned live clip", "LiveClip", D.liveClip, {}, true),
        Choice("Audio placement", "AudioGtaPlacement", {"Position", "Tracker"}, {"Emitter positions", "Player ped"}, {}, true),
        Flag("Editor audio tone", "AudioEditorTone", false, {}, true),
        Flag("Stall sampler", "StallSampler", D.stallSampler, {}, true),
    }});
    return pages;
}

void ScriptMain() {
    Log("SkateV Legacy: ScriptMain start");
    {
        // Other script mods loaded alongside, for attributing hitches.
        std::string asis;
        WIN32_FIND_DATAW fd{};
        const HANDLE h = FindFirstFileW((g_gameDir + L"*.asi").c_str(), &fd);
        if (h != INVALID_HANDLE_VALUE) {
            do {
                asis += (asis.empty() ? "" : ", ") + util::Narrow(fd.cFileName);
            } while (FindNextFileW(h, &fd));
            FindClose(h);
        }
        Logf("SkateV Legacy: ASI files in game folder: %s", asis.c_str());
    }
    const GameVersion version = ReadHostVersion();
    Logf("SkateV Legacy: host GTA5.exe %u.%u.%u.%u (required %u.%u.%u.%u)", version.major, version.minor,
         version.build, version.revision, kRequiredVersion.major, kRequiredVersion.minor, kRequiredVersion.build,
         kRequiredVersion.revision);
    if (!IsSupportedLegacy(version)) {
        // Unsupported build: stay inert rather than call natives with wrong assumptions.
        Log("SkateV Legacy: unsupported game version; skate mode disabled");
        for (;;) WAIT(1000);
    }

    g_config = LoadConfig();
    const Config& cfg = g_config;
    util::g_verbose = cfg.verboseLog;
    std::string error;
    if (!cfg.loadRuntime) {
        error = "Runtime=0 in SkateVLegacy.ini";
        Log("SkateV Legacy: Runtime=0, Skate runtime not loaded (frame-timing log only)");
    } else if (SetEnvironmentVariableA("SKATEV_PED_LAUNCH", cfg.pedLaunch ? "1" : "0"),
               !g_runtime.Load((g_gameDir + L"SkateVRuntime.dll").c_str(), cfg.dataRoot, cfg.worldCache,
                        util::Narrow(g_logDir + L"SkateVRuntime.log"),
                        static_cast<std::uint32_t>(cfg.skaterTriangles > 0 ? cfg.skaterTriangles : 0),
                        1u /* Ped presentation */ | (cfg.hallOfMeat ? 2u : 0u) |
                            (cfg.hallOfMeatMetrics ? 4u : 0u) | (cfg.goofy ? 8u : 0u),
                        error)) {
        Logf("SkateV Legacy: runtime unavailable: %s", error.c_str());
    } else {
        Logf("SkateV Legacy: runtime ABI %u loaded (host ABI %u); data '%s' cache '%s'", g_runtime.ApiVersion(),
             SKATEV_ABI_VERSION, cfg.dataRoot.c_str(), cfg.worldCache.c_str());
        Logf("SkateV Legacy: %zu exact prop and vehicle collision templates", DynamicWorld::LoadPropTemplates(cfg.worldCache));
        Logf("SkateV Legacy: %zu script-toggled map states with collision", mapstates::Load(cfg.worldCache));
        ApplyQuirk();
        Logf("SkateV Legacy: BackwardsMan %s chord %04x", cfg.backwardsMan ? "on" : "off", cfg.backwardsManChord);
        hudoverlay::Start(g_gameDir + L"SkateVLegacy.ini", g_runtime.Handle(), Log);
        recordsui::Start(g_runtime.Handle(), Log);
        Logf("SkateV Legacy: Hall of Meat %s (%s)", cfg.hallOfMeat ? "on" : "off",
             g_runtime.SetHallOfMeat(cfg.hallOfMeat) ? "runtime toggle present" : "runtime toggle absent");
        Logf("SkateV Legacy: bail time limit %.1f s (%s; %s)", cfg.bailTimeLimit,
             cfg.bailTimeLimit < 0.0f ? "Skate's own" : cfg.bailTimeLimit == 0.0f ? "none" : "INI",
             g_runtime.SetBailLimit(cfg.bailTimeLimit) ? "runtime option present" : "runtime option absent");
        Logf("SkateV Legacy: air time limit %.1f s (%s; %s)", cfg.airTimeLimit,
             cfg.airTimeLimit < 0.0f ? "Skate's own" : cfg.airTimeLimit == 0.0f ? "none" : "INI",
             g_runtime.SetAirLimit(cfg.airTimeLimit) ? "runtime option present" : "runtime option absent");
        Logf("SkateV Legacy: skitch standoff %.2f m (%s)", cfg.skitchStandoff,
             g_runtime.SetSkitchStandoff(cfg.skitchStandoff) ? "runtime option present" : "runtime option absent");
        physlevel::Start(Log, [](std::uintptr_t table, std::uintptr_t lo, std::uintptr_t hi) { g_runtime.SetPhysicsLevel(table, lo, hi); });
        Logf("SkateV Legacy: ramp lips %s (%s)", cfg.lipRule ? "SkateV rule" : "retail",
             g_runtime.SetLipRule(cfg.lipRule) ? "runtime option present" : "runtime option absent");
        Logf("SkateV Legacy: difficulty %s (%s)", kDifficulties[cfg.difficulty],
             g_runtime.SetDifficulty(cfg.difficulty) ? "runtime option present" : "runtime option absent");
        g_runtime.SetVerboseLog(cfg.verboseLog);
    }

    // Skate audio inside GTA's audio engine (P0-3b); runtime may be absent.
    gtaaudio::Start(g_gameDir + L"SkateVLegacy.ini", GetModuleHandleW(L"SkateVRuntime.dll"), g_runtime.Handle(), Log);
    Logf("SkateV Legacy: menu button %04x (missions %s), background prepare %s", cfg.life.menuButton,
         cfg.life.inMissions ? "allowed" : "blocked", cfg.life.backgroundPrepare ? "on" : "off");
    Session s;
    g_hallOfMeatOn = cfg.hallOfMeat;
    menu::Start(g_gameDir + L"SkateVLegacy.ini", MenuPages(s), RawPadButtons, Log);
    FrameMonitor monitor;
    stall::SetLog(Log);
    // Line= may list several files separated by ';': the Dev menu replays the
    // current one or moves to the next.
    std::vector<std::string> lines;
    for (size_t at = 0; at <= cfg.line.size();) {
        const size_t end = std::min(cfg.line.find(';', at), cfg.line.size());
        if (end > at) lines.push_back(cfg.line.substr(at, end - at));
        at = end + 1;
    }
    size_t lineIndex = 0;
    for (;;) {
        const double workStart = monitor.Now();
        const bool f6 = g_toggleRequested.exchange(false);
        {
            static bool showList = false;
            static DWORD nextFetch = 0;
            static std::string list;
            if (g_soundListToggle.exchange(false)) showList = !showList;
            if (showList && g_runtime.IsLoaded()) {
                if (GetTickCount() >= nextFetch) {
                    nextFetch = GetTickCount() + 250;
                    list = g_runtime.AudioDebugText();
                }
                float y = 0.06f;
                std::size_t start = 0;
                for (int line = 0; line < 18 && start < list.size(); ++line) {
                    const std::size_t end = list.find('\n', start);
                    const std::string text = list.substr(start, end == std::string::npos ? std::string::npos : end - start);
                    Text(text.c_str(), 0.02f, y, 0.3f, 255, 255, 160, false);
                    y += 0.022f;
                    if (end == std::string::npos) break;
                    start = end + 1;
                }
            }
        }
        if (g_lineNext.exchange(false) && !lines.empty()) {
            lineIndex = (lineIndex + 1) % lines.size();
            const std::string& l = lines[lineIndex];
            const size_t slash = l.find_last_of("/\\");
            Notify(s, "Line " + std::to_string(lineIndex + 1) + "/" + std::to_string(lines.size()) + ": " +
                          (slash == std::string::npos ? l : l.substr(slash + 1)), 2000);
        }
        if (g_lineRequested.exchange(false) && g_runtime.IsLoaded()) {
            if (lines.empty()) {
                Notify(s, "No showcase line: add Line= to SkateVLegacy.ini", 2500);
            } else if (s.state != LifeState::Skating) {
                Notify(s, "Get on the board first, then play the line", 2000);
            } else if (g_runtime.PlayLine(lines[lineIndex])) {
                Logf("SkateV Legacy: line %s requested", lines[lineIndex].c_str());
                Notify(s, "Line: any input takes over", 1500);
            }
        }
        if (g_runtime.IsLoaded()) {
            LifecycleFrame(s, cfg, f6);
            if (s.aiming && s.state != LifeState::Skating) StopAim(s, false);
            if (s.wheelOpen && s.state != LifeState::Skating) {
                s.wheelOpen = g_weaponWheel = false;
                Call<void>(gta::HUD_FORCE_WEAPON_WHEEL, 0);
            }
            EnforceHeldPed(s);
        } else if (f6) {
            Notify(s, "runtime unavailable: " + error);
        }

        if (!s.message.empty() && GetTickCount() < s.messageUntil) {
            Text(s.message.c_str(), 0.5f, 0.88f, 0.4f, 255, 255, 255, true);
        }
        hudoverlay::Tick(s.state == LifeState::Skating);
        if (s.board.Pressed(RawPadButtons(), cfg.life.menuButton)) g_menuToggle.store(true);
        if (g_menuToggle.exchange(false)) {
            if (recordsui::Showing()) recordsui::NextPage();  // '/' also closes the records board
            else menu::Toggle();
        }
        menu::Tick();
        recordsui::Tick(s.state == LifeState::Skating,
                        g_hallOfMeatOn && std::string_view(s.last.state_utf8).rfind("Wipeout", 0) == 0);
        if (s.state != LifeState::Skating) boardnative::Tick(false, SvBoardPose{});
        gtaaudio::Tick(s.state == LifeState::Skating);
        physlevel::Tick();
        monitor.Work(monitor.Now() - workStart);
        WAIT(0);
        if (cfg.stallSampler) stall::Beat();
        monitor.Frame(s.state == LifeState::Skating);
    }
}
}

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID reserved) {
    if (reason == DLL_PROCESS_ATTACH) {
        g_gameDir = GameDir();
        g_logDir = LogDir();
        // One session per log: the previous launch's logs become *.prev.log, older ones go.
        for (const wchar_t* name : {L"SkateVLegacy", L"SkateVRuntime", L"SkateVRuntime-crash"})
            MoveFileExW((g_logDir + name + L".log").c_str(), (g_logDir + name + L".prev.log").c_str(),
                        MOVEFILE_REPLACE_EXISTING);
        DisableThreadLibraryCalls(module);
        packloader::Install(g_gameDir, Log);
        keyboardHandlerRegister(Keyboard);
        scriptRegister(module, ScriptMain);
    } else if (reason == DLL_PROCESS_DETACH) {
        keyboardHandlerUnregister(Keyboard);
        scriptUnregister(module);
        hudoverlay::Shutdown(reserved != nullptr);
        // reserved != nullptr: the process is exiting; let the OS reclaim.
        // Otherwise release the handle; the runtime DLL itself stays mapped
        // because its worker thread exits asynchronously.
        if (reserved == nullptr) {
            stall::Stop();
            gtaaudio::Stop(); // feeder thread pulls from the runtime handle
            g_runtime.Release();
            Log("SkateV Legacy: detached");
        }
        if (g_log) { std::fclose(g_log); g_log = nullptr; }
    }
    return TRUE;
}

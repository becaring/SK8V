#include "board_shop.h"
#include "board_parts.h"
#include "host_util.h"
#include "menu.h"
#include "natives.h"
#include "runtime_bridge.h"
#include <windows.h>
#include <cmath>
#include <cstring>
#include <cwchar>
#include <filesystem>
#include <cstdlib>
#include <cstdio>
#include <sstream>
#include <vector>

namespace boardshop {
namespace {
using namespace gta;

// GTA's controls: G (INPUT_DETONATE, D-pad left) opens the boards; E stays
// the store's own clothing counter. The menu reads the frontend group.
constexpr int kOpen = 47;
constexpr int kUp = 172, kDown = 173, kLeft = 174, kRight = 175, kAccept = 201, kCancel = 202, kReset = 203;

RuntimeBridge* g_rt = nullptr;
void (*g_log)(const char*) = nullptr;
void (*g_wheels)(float) = nullptr;
bool g_ready = false, g_open = false;
std::uint32_t g_model = 0;
int g_tierShown = -1;
bool g_partView = false;  // Tab: the offer's part alone, or the whole board

void Log(const std::string& s) {
    if (g_log) g_log(s.c_str());
}

// Binco and the Discount Store (0), Suburban (1); Ponsonbys (2) sells no boards.
int TierHere(int ped) {
    const int interior = Call<int>(GET_INTERIOR_FROM_ENTITY, ped);
    if (!interior) return -1;
    Vector3 at{};
    std::uint32_t hash = 0;
    Call<void>(GET_INTERIOR_LOCATION_AND_NAMEHASH, interior, &at, &hash);
    if (hash == util::Joaat("v_clotheslo")) return 0;
    if (hash == util::Joaat("v_clothesmid")) return 1;
    if (hash == util::Joaat("v_clotheshi")) return 2;
    return -1;
}

// Michael, Franklin and Trevor by model (each keeps their own boards and cash).
const char* Who(std::uint32_t model) {
    if (model == util::Joaat("player_zero")) return "michael";
    if (model == util::Joaat("player_one")) return "franklin";
    if (model == util::Joaat("player_two")) return "trevor";
    return nullptr;
}

std::uint32_t CashStat(std::uint32_t model) {
    if (model == util::Joaat("player_zero")) return util::Joaat("SP0_TOTAL_CASH");
    if (model == util::Joaat("player_one")) return util::Joaat("SP1_TOTAL_CASH");
    if (model == util::Joaat("player_two")) return util::Joaat("SP2_TOTAL_CASH");
    return 0;
}

std::vector<std::string> Split(const std::string& text, char by) {
    std::vector<std::string> out;
    std::string item;
    std::istringstream in(text);
    while (std::getline(in, item, by))
        if (!item.empty()) out.push_back(item);
    return out;
}

// The boards on show in the store the player is in (found once per store and
// kept by the runtime; tried again every 1.5 s until a place is found).
struct OnShow {
    float at[16];
    std::vector<std::string> models;
};
std::vector<OnShow> g_onShow;
std::uint32_t g_showStore = 0;  // the interior they are for
DWORD g_showRetry = 0;
bool g_showMissed = false;  // "no place yet" logged for this store

std::uint32_t __cdecl Probe(void* ctx, const float* from, const float* to, float* hit) {
    const int test = Call<int>(START_EXPENSIVE_SYNCHRONOUS_SHAPE_TEST_LOS_PROBE, from[0], from[1], from[2], to[0], to[1], to[2],
                               1 | 16, static_cast<int>(reinterpret_cast<std::intptr_t>(ctx)), 7);
    BOOL any = 0;
    Vector3 end{}, normal{};
    Entity entity = 0;
    if (Call<int>(GET_SHAPE_TEST_RESULT, test, &any, &end, &normal, &entity) != 2 || !any) return 0;
    hit[0] = end.x, hit[1] = end.y, hit[2] = end.z;
    return 1;
}

// The store's clerk standing near (s_f_y_shop_low/mid, the mask shop's).
int Clerk(int ped) {
    static int peds[256];
    const int n = worldGetAllPeds(peds, 256);
    const Vector3 me = Call<Vector3>(GET_ENTITY_COORDS, ped, 0);
    int best = 0;
    float bestD = 20.0f * 20.0f;
    for (int i = 0; i < n; ++i) {
        const int p = peds[i];
        if (p == ped || Call<BOOL>(IS_ENTITY_DEAD, p, 0)) continue;
        const std::uint32_t m = Call<std::uint32_t>(GET_ENTITY_MODEL, p);
        if (m != util::Joaat("s_f_y_shop_low") && m != util::Joaat("s_f_y_shop_mid") && m != util::Joaat("s_m_y_shop_mask")) continue;
        const Vector3 c = Call<Vector3>(GET_ENTITY_COORDS, p, 0);
        const float d = (c.x - me.x) * (c.x - me.x) + (c.y - me.y) * (c.y - me.y) + (c.z - me.z) * (c.z - me.z);
        if (d < bestD) bestD = d, best = p;
    }
    return best;
}

void ClearShow() {
    if (!g_onShow.empty()) boardparts::ClearDisplay();
    g_onShow.clear();
    g_showStore = 0;
}

// Boards on show: placed once, then kept posed at rest every frame.
void ShowBoards(int ped, int tier) {
    const int interior = Call<int>(GET_INTERIOR_FROM_ENTITY, ped);
    Vector3 place{};
    std::uint32_t hash = 0;
    Call<void>(GET_INTERIOR_LOCATION_AND_NAMEHASH, interior, &place, &hash);
    const std::uint32_t key = static_cast<std::uint32_t>(interior);
    if (key != g_showStore) ClearShow(), g_showStore = key, g_showRetry = 0, g_showMissed = false;
    if (g_onShow.empty() && static_cast<LONG>(GetTickCount() - g_showRetry) >= 0) {
        g_showRetry = GetTickCount() + 1500;
        const Vector3 me = Call<Vector3>(GET_ENTITY_COORDS, ped, 0);
        const float at[3] = {place.x, place.y, place.z}, eye[3] = {me.x, me.y, me.z + 0.3f};
        float clerk[6];
        const int c = Clerk(ped);
        if (c) {
            const Vector3 p = Call<Vector3>(GET_ENTITY_COORDS, c, 0), f = Call<Vector3>(GET_ENTITY_FORWARD_VECTOR, c);
            clerk[0] = p.x, clerk[1] = p.y, clerk[2] = p.z, clerk[3] = f.x, clerk[4] = f.y, clerk[5] = f.z;
        }
        const unsigned day = static_cast<unsigned>(Call<int>(GET_CLOCK_MONTH) * 31 + Call<int>(GET_CLOCK_DAY_OF_MONTH));
        float out[8 * 16];
        std::vector<std::string> models;
        const int n = g_rt->ShopDisplay(tier, at, eye, c ? clerk : nullptr, day, Probe,
                                        reinterpret_cast<void*>(static_cast<std::intptr_t>(ped)), out, 8, models);
        for (int i = 0; i < n; ++i) {
            OnShow b;
            std::memcpy(b.at, out + i * 16, sizeof(b.at));
            b.models = Split(models[static_cast<std::size_t>(i)], ',');
            g_onShow.push_back(b);
        }
        if (n) Log("board shop: " + std::to_string(n) + " boards on show (" + (c ? "a clerk near: behind the till" : "on a wall") + ")");
        else if (!g_showMissed) g_showMissed = true, Log("board shop: no place for boards on show found yet (tried again every 1.5 s)");
    }
    for (std::size_t i = 0; i < g_onShow.size(); ++i) {
        SvBoardPose pose{};
        if (g_rt->ShopRestPose(g_onShow[i].at, pose)) boardparts::Display(static_cast<int>(i), g_onShow[i].models, pose);
    }
}

// Looking at the boards on show from up to 3.5 m (the camera within 25
// degrees of them): E opens the shop there, as at the store's other goods.
// Looking elsewhere (the counter, the clerk) E stays the store's own.
bool LookingAtShow(int ped) {
    if (g_onShow.empty()) return false;
    float c[3] = {0, 0, 0};
    for (const OnShow& b : g_onShow)
        for (int i = 0; i < 3; ++i) c[i] += b.at[12 + i] / static_cast<float>(g_onShow.size());
    const Vector3 me = Call<Vector3>(GET_ENTITY_COORDS, ped, 0);
    const float dx = c[0] - me.x, dy = c[1] - me.y;
    if (dx * dx + dy * dy > 3.5f * 3.5f) return false;
    const Vector3 cam = Call<Vector3>(GET_GAMEPLAY_CAM_COORD);
    const Vector3 rot = Call<Vector3>(GET_GAMEPLAY_CAM_ROT, 2);
    const float yaw = rot.z * 0.017453292f, pitch = rot.x * 0.017453292f;
    const float f[3] = {-std::sin(yaw) * std::cos(pitch), std::cos(yaw) * std::cos(pitch), std::sin(pitch)};
    float to[3] = {c[0] - cam.x, c[1] - cam.y, c[2] - cam.z};
    const float len = std::sqrt(to[0] * to[0] + to[1] * to[1] + to[2] * to[2]);
    if (len < 0.1f) return true;
    return (f[0] * to[0] + f[1] * to[1] + f[2] * to[2]) / len > std::cos(25.0f * 0.017453292f);
}

// A line at the bottom right: GTA's help box stays the store's own (its
// clothing counter prompt), which a help text of ours would replace.
void Hint(const char* text) {
    gta::Text(text, 0.0f, 0.93f, 0.36f, 255, 255, 255, 230, gta::Align::Right, 0.985f, true);
}

// GTA's own help box (top left), as the store's other goods prompt with it:
// "~INPUT_CONTEXT~" draws the key the player has for E.
void HelpBox(const char* text) {
    Call<void>(BEGIN_TEXT_COMMAND_DISPLAY_HELP, "STRING");
    Call<void>(ADD_TEXT_COMPONENT_SUBSTRING_PLAYER_NAME, text);
    Call<void>(END_TEXT_COMMAND_DISPLAY_HELP, 0, 0, 1, -1);
}

void Sound(const char* name, const char* set = "HUD_FRONTEND_DEFAULT_SOUNDSET") {
    Call<void>(PLAY_SOUND_FRONTEND, -1, name, set, 1);
}

void Ride() {
    boardparts::SetLoadout(Split(g_rt->ShopText(1), ','));
    if (g_wheels) {
        const std::string h = g_rt->ShopText(4);
        g_wheels(h.empty() ? -1.0f : static_cast<float>(std::atof(h.c_str())));
    }
}

void Close() {
    g_rt->ShopClose();
    g_open = false;
    boardparts::Preview({}, SvBoardPose{});
}

// The preview board's axes (rows: board X, Y, Z) in the camera's level frame
// (right, forward, up: right-handed, so the matrix stays a rotation that
// SET_ENTITY_QUATERNION takes): first the deck tilted 35 degrees toward the
// camera; turned all ways round with the mouse while the left button is held
// (it never turns by itself).
float g_orient[3][3];

void ResetOrient() {
    const float t = 0.61f, c = std::cos(t), s = std::sin(t);
    const float o[3][3] = {{1.0f, 0.0f, 0.0f}, {0.0f, c, s}, {0.0f, -s, c}};
    std::memcpy(g_orient, o, sizeof(o));
}

// Turns every row by `angle` about camera axis `axis` (0 right, 2 up).
void Turn(int axis, float angle) {
    const int a = axis == 0 ? 1 : 0, b = axis == 0 ? 2 : 1;
    const float c = std::cos(angle), s = std::sin(angle);
    for (auto& row : g_orient) {
        const float u = row[a], v = row[b];
        row[a] = c * u - s * v;
        row[b] = s * u + c * v;
    }
}

float g_zoom = 1.6f; // the preview's distance from the camera (mouse wheel)

// The side the offer is about faces the player: the grip for a griptape, the
// graphic (and trucks and wheels) for the rest. Turning by hand stays until
// the cursor reaches an offer about the other side.
void FaceOffer() {
    static std::string s_face;
    const std::string face = g_rt->ShopText(6);
    if (face.empty() || face == s_face) return;
    s_face = face;
    ResetOrient();  // the top toward the camera
    if (face == "bottom")
        for (int i = 0; i < 3; ++i) g_orient[0][i] = -g_orient[0][i], g_orient[2][i] = -g_orient[2][i];  // half a turn about its length
}

void ShowPreview() {
    FaceOffer();
    if (Call<BOOL>(IS_DISABLED_CONTROL_PRESSED, 0, 24)) {
        Turn(2, Call<float>(GET_DISABLED_CONTROL_NORMAL, 0, 1) * 4.0f);
        Turn(0, Call<float>(GET_DISABLED_CONTROL_NORMAL, 0, 2) * 4.0f);
    }
    // Keyboard: A/D turn it round, W/S tip it; a controller's right stick does both.
    const float dt = Call<float>(GET_FRAME_TIME), step = 2.2f * (dt > 0.1f ? 0.1f : dt);
    auto down = [](int vk) { return (GetAsyncKeyState(vk) & 0x8000) != 0; };
    if (down('A')) Turn(2, -step);
    if (down('D')) Turn(2, step);
    if (down('W')) Turn(0, -step);
    if (down('S')) Turn(0, step);
    if (!Call<BOOL>(IS_USING_KEYBOARD_AND_MOUSE, 0)) {
        Turn(2, Call<float>(GET_DISABLED_CONTROL_NORMAL, 0, 220) * step * 1.5f); // INPUT_SCRIPT_RIGHT_AXIS_X
        Turn(0, Call<float>(GET_DISABLED_CONTROL_NORMAL, 0, 221) * step * 1.5f);
    }
    // Mouse wheel (or the triggers): nearer or farther.
    if (Call<BOOL>(IS_DISABLED_CONTROL_JUST_PRESSED, 0, 241)) g_zoom -= 0.15f;  // INPUT_CURSOR_SCROLL_UP
    if (Call<BOOL>(IS_DISABLED_CONTROL_JUST_PRESSED, 0, 242)) g_zoom += 0.15f;  // INPUT_CURSOR_SCROLL_DOWN
    // The triggers only on a controller: on the keyboard INPUT_SCRIPT_RT is the
    // left mouse button, which turns the board (it zoomed out while held).
    if (!Call<BOOL>(IS_USING_KEYBOARD_AND_MOUSE, 0))
        g_zoom += (Call<float>(GET_DISABLED_CONTROL_NORMAL, 0, 229) - Call<float>(GET_DISABLED_CONTROL_NORMAL, 0, 228)) * dt; // RT / LT
    g_zoom = g_zoom < 0.7f ? 0.7f : g_zoom > 3.0f ? 3.0f : g_zoom;
    const Vector3 cam = Call<Vector3>(GET_GAMEPLAY_CAM_COORD);
    const Vector3 rot = Call<Vector3>(GET_GAMEPLAY_CAM_ROT, 2);
    const float yaw = rot.z * 0.017453292f, pitch = rot.x * 0.017453292f;
    const float fx = -std::sin(yaw) * std::cos(pitch), fy = std::cos(yaw) * std::cos(pitch), fz = std::sin(pitch);
    const float rx = std::cos(yaw), ry = std::sin(yaw);
    const float right[3] = {rx, ry, 0.0f}, forward[3] = {-std::sin(yaw), std::cos(yaw), 0.0f}, up[3] = {0.0f, 0.0f, 1.0f};
    float at[16];
    for (int r = 0; r < 3; ++r) {
        for (int i = 0; i < 3; ++i) at[r * 4 + i] = g_orient[r][0] * right[i] + g_orient[r][1] * forward[i] + g_orient[r][2] * up[i];
        at[r * 4 + 3] = 0.0f;
    }
    // g_zoom m ahead (1.6 at first) and a little right of the camera, a little below its line.
    const float side = 0.5f * g_zoom / 1.6f;
    at[12] = cam.x + fx * g_zoom + rx * side;
    at[13] = cam.y + fy * g_zoom + ry * side;
    at[14] = cam.z + fz * g_zoom - 0.2f * g_zoom / 1.6f;
    at[15] = 1.0f;
    SvBoardPose pose{};
    if (g_rt->ShopRestPose(at, pose)) boardparts::Preview(Split(g_rt->ShopText(g_partView ? 5 : 2), ','), pose);
}

void Menu(int ped) {
    // Every control but pause, one by one: DISABLE_ALL_CONTROL_ACTIONS would
    // stop a Rockstar Editor recording (as SK8V's own control handling notes).
    for (int c = 0; c < 360; ++c)
        if (c != 199 && c != 200) Call<void>(DISABLE_CONTROL_ACTION, 0, c, 1);
    Call<void>(INVALIDATE_IDLE_CAM);
    struct Key { int control, input; const char* sound; };
    constexpr Key keys[] = {{kUp, 0, "NAV_UP_DOWN"},       {kDown, 1, "NAV_UP_DOWN"},   {kLeft, 2, "NAV_LEFT_RIGHT"},
                            {kRight, 3, "NAV_LEFT_RIGHT"}, {kAccept, 4, nullptr},       {kCancel, 5, "BACK"},
                            {kReset, 6, "SELECT"}};
    const std::uint32_t model = Call<std::uint32_t>(GET_ENTITY_MODEL, ped);
    const std::uint32_t stat = CashStat(model);
    // Tab (INPUT_FRONTEND_Y, the pad's Y): the whole board or the part alone.
    if (Call<BOOL>(IS_DISABLED_CONTROL_JUST_PRESSED, 2, 204)) {
        g_partView = !g_partView;
        Sound("NAV_LEFT_RIGHT");
    }
    for (const Key& k : keys) {
        if (!Call<BOOL>(IS_DISABLED_CONTROL_JUST_PRESSED, 2, k.control)) continue;
        int cash = 0;
        if (stat) Call<BOOL>(STAT_GET_INT, stat, &cash, -1);
        const SvShopResult r = g_rt->ShopInput(k.input, stat ? cash : 0);
        if (k.sound) Sound(k.sound);
        switch (r.kind) {
        case 1: Close(); return;
        case 2:
            Call<BOOL>(STAT_SET_INT, stat, cash - static_cast<int>(r.price), true);
            Sound("PURCHASE", "HUD_LIQUOR_STORE_SOUNDSET");
            Ride();
            break;
        case 3: case 4: Sound("SELECT"); Ride(); break;
        case 5: Sound("ERROR"); break;
        default: break;
        }
        break;
    }
    const auto lines = Split(g_rt->ShopText(0), '\n');
    if (lines.size() < 2) {
        Close();
        return;
    }
    const auto head = Split(lines[0], '\t');
    std::vector<menu::PanelRow> rows;
    for (std::size_t i = 2; i < lines.size(); ++i) {
        const auto cells = Split(lines[i], '\t');
        menu::PanelRow row;
        row.label = cells.size() > 0 ? cells[0] : "";
        row.value = cells.size() > 1 ? cells[1] : "";
        row.selected = cells.size() > 2 && cells[2] == "1";
        rows.push_back(row);
    }
    menu::DrawPanel("Boards", head.empty() ? "" : head[0], head.size() > 1 ? head[1] : "", rows,
                    lines[1] + (g_partView ? "  Tab: whole board" : "  Tab: the part alone") + "  Turn: hold left mouse + move, or W A S D  Zoom: mouse wheel");
    ShowPreview();
}

} // namespace

namespace {
// The board in hand (GTA mode): WEAPON_SKATEBOARD, SK8V's hammer clone whose
// hammer is hidden, carries the bought board, placed on the hidden hammer.
// CarryOffset / CarryRotation (INI, the menu's Board shop page) tune it.
constexpr Hash kBoardWeapon = 0x38A8F85C;
float g_carryOffset[3] = {0.0f, 0.0f, 0.3f};   // m, in the hammer's frame (its length is +Z)
float g_carryRotation[3] = {0.0f, 0.0f, 0.0f}; // degrees about the hammer's X, Y, Z
std::wstring g_iniPath;

void ReadCarry() {
    if (g_iniPath.empty()) return;
    auto one = [](const wchar_t* key, float& out) {
        wchar_t text[32]{};
        GetPrivateProfileStringW(L"SkateV", key, L"", text, 32, g_iniPath.c_str());
        float v;
        if (swscanf_s(text, L"%f", &v) == 1) out = v;
    };
    one(L"CarryX", g_carryOffset[0]);
    one(L"CarryY", g_carryOffset[1]);
    one(L"CarryZ", g_carryOffset[2]);
    one(L"CarryTurnX", g_carryRotation[0]);
    one(L"CarryTurnY", g_carryRotation[1]);
    one(L"CarryTurnZ", g_carryRotation[2]);
}

// Rotates the 3x3 `m` (columns) about local axis `axis` by `deg`.
void Spin(float (&m)[3][3], int axis, float deg) {
    const float r = deg * 0.017453292f, c = std::cos(r), s = std::sin(r);
    const int a = (axis + 1) % 3, b = (axis + 2) % 3;
    for (int i = 0; i < 3; ++i) {
        const float u = m[a][i], v = m[b][i];
        m[a][i] = c * u + s * v;
        m[b][i] = -s * u + c * v;
    }
}

void Carry(int ped) {
    static int s_frame = 0;
    const bool held = Call<Hash>(GET_SELECTED_PED_WEAPON, ped) == kBoardWeapon;
    const int standIn = held ? Call<int>(GET_CURRENT_PED_WEAPON_ENTITY_INDEX, ped, 0) : 0;
    if (!standIn) {
        boardparts::Hand({}, SvBoardPose{});
        return;
    }
    static ULONGLONG s_readAt = 0;
    if (GetTickCount64() >= s_readAt) ReadCarry(), s_readAt = GetTickCount64() + 1000;
    // The hammer's frame from three of its points.
    const Vector3 o = Call<Vector3>(GET_OFFSET_FROM_ENTITY_IN_WORLD_COORDS, standIn, 0.0f, 0.0f, 0.0f);
    const Vector3 ex = Call<Vector3>(GET_OFFSET_FROM_ENTITY_IN_WORLD_COORDS, standIn, 1.0f, 0.0f, 0.0f);
    const Vector3 ey = Call<Vector3>(GET_OFFSET_FROM_ENTITY_IN_WORLD_COORDS, standIn, 0.0f, 1.0f, 0.0f);
    const Vector3 ez = Call<Vector3>(GET_OFFSET_FROM_ENTITY_IN_WORLD_COORDS, standIn, 0.0f, 0.0f, 1.0f);
    const float hx[3] = {ex.x - o.x, ex.y - o.y, ex.z - o.z}, hy[3] = {ey.x - o.x, ey.y - o.y, ey.z - o.z},
                hz[3] = {ez.x - o.x, ez.y - o.y, ez.z - o.z};
    // The board's axes in the hammer's frame: its length (Y) along the
    // handle (Z), its deck (Z) along the hammer's X; then the tuning turn.
    float local[3][3] = {{0, 1, 0}, {0, 0, 1}, {1, 0, 0}};  // rows: board X, Y, Z
    for (int axis = 0; axis < 3; ++axis) Spin(local, axis, g_carryRotation[axis]);
    float at[16]{};
    for (int r = 0; r < 3; ++r)
        for (int i = 0; i < 3; ++i) at[r * 4 + i] = local[r][0] * hx[i] + local[r][1] * hy[i] + local[r][2] * hz[i];
    at[12] = o.x + g_carryOffset[0] * hx[0] + g_carryOffset[1] * hy[0] + g_carryOffset[2] * hz[0];
    at[13] = o.y + g_carryOffset[0] * hx[1] + g_carryOffset[1] * hy[1] + g_carryOffset[2] * hz[1];
    at[14] = o.z + g_carryOffset[0] * hx[2] + g_carryOffset[1] * hy[2] + g_carryOffset[2] * hz[2];
    at[15] = 1.0f;
    SvBoardPose pose{};
    if (g_rt->ShopRestPose(at, pose)) boardparts::Hand(Split(g_rt->ShopText(1), ','), pose);
    // Blows struck with it wear the board: people and vehicles within 3 m that
    // the player damaged, every 6 frames.
    if (++s_frame % 6) return;
    const Vector3 me = Call<Vector3>(GET_ENTITY_COORDS, ped, 0);
    static int s_near[64];
    for (int pass = 0; pass < 2; ++pass) {
        const int n = pass == 0 ? worldGetAllPeds(s_near, 64) : worldGetAllVehicles(s_near, 64);
        for (int i = 0; i < n; ++i) {
            const int e = s_near[i];
            if (e == ped) continue;
            const Vector3 p = Call<Vector3>(GET_ENTITY_COORDS, e, 0);
            if ((p.x - me.x) * (p.x - me.x) + (p.y - me.y) * (p.y - me.y) > 9.0f) continue;
            if (Call<BOOL>(HAS_ENTITY_BEEN_DAMAGED_BY_ENTITY, e, ped, 1)) {
                Call<void>(CLEAR_ENTITY_LAST_DAMAGE_ENTITY, e);
                if (g_rt->ShopRide("Hit", 0.0f, 0.0f)) Ride();
            }
        }
    }
}
} // namespace

bool Start(RuntimeBridge& runtime, const std::string& dataRoot, const std::string& gameDir, void (*log)(const char*),
           void (*wheels)(float hardness)) {
    g_rt = &runtime;
    g_log = log;
    g_wheels = wheels;
    g_model = 0;
    if (!runtime.HasShop()) {
        Log("board shop: the runtime has none");
        return g_ready = false;
    }
    g_ready = runtime.ShopStart(dataRoot, gameDir);
    g_iniPath = std::filesystem::path(gameDir).wstring() + L"\\SkateVLegacy.ini";
    Log("board shop: " + runtime.ShopText(3));
    return g_ready;
}

void Tick(bool busy) {
    if (!g_ready) return;
    const int ped = Call<int>(PLAYER_PED_ID);
    const std::uint32_t model = Call<std::uint32_t>(GET_ENTITY_MODEL, ped);
    if (model != g_model) {
        g_model = model;
        if (g_open) Close();
        char hex[16];
        std::snprintf(hex, sizeof(hex), "%08x", model);
        const char* who = Who(model);
        g_rt->ShopCharacter(who ? who : hex);
        Ride();
    }
    if (!busy) Carry(ped);
    else boardparts::Hand({}, SvBoardPose{});
    const int tier = TierHere(ped);
    if (tier != g_tierShown) {
        g_tierShown = tier;
        if (tier >= 0) Log("board shop: in a clothing store (tier " + std::to_string(tier) + (tier < 2 ? ", G opens the boards)" : ", sells no boards)"));
    }
    const bool can = !busy && (tier == 0 || tier == 1);
    // The boards on show stay while the player is in the store (on the
    // board too); they go when leaving it.
    if (tier == 0 || tier == 1) ShowBoards(ped, tier);
    else ClearShow();
    if (g_open) {
        if (!can) {
            Close();
            return;
        }
        Menu(ped);
        return;
    }
    if (!can || menu::IsOpen() || Call<BOOL>(IS_PED_IN_ANY_VEHICLE, ped, false) ||
        Call<BOOL>(IS_PLAYER_WANTED_LEVEL_GREATER, Call<int>(PLAYER_ID), 0))
        return;
    const bool atShow = LookingAtShow(ped);
    if (atShow) Call<void>(DISABLE_CONTROL_ACTION, 0, 51, 1);  // INPUT_CONTEXT: ours while looking at the boards
    // Looking at the boards: GTA's help box, as at the clothes. Elsewhere in
    // the store the box stays the store's own, and a line names G.
    if (atShow) HelpBox("Press ~INPUT_CONTEXT~ to browse Skate 3 boards.");
    else Hint("G / D-pad left:  Skate 3 boards (complete or build your own)");
    // Skate mode disables GTA's controls; the key is read either way.
    const bool e = atShow && Call<BOOL>(IS_DISABLED_CONTROL_JUST_PRESSED, 0, 51);
    if ((e || Call<BOOL>(IS_CONTROL_JUST_PRESSED, 0, kOpen) || Call<BOOL>(IS_DISABLED_CONTROL_JUST_PRESSED, 0, kOpen)) &&
        g_rt->ShopOpen(tier)) {
        g_open = true;
        ResetOrient();
        Sound("SELECT");
        Log("board shop: opened");
    }
}

void Riding(const char* state, float speed) {
    if (!g_ready) return;
    if (g_rt->ShopRide(state, speed, Call<float>(GET_FRAME_TIME))) {
        Log("board shop: a part of the board wore through: its worn finish shows now");
        Ride();
    }
}

bool IsOpen() {
    return g_open;
}


bool CrewBoard(unsigned slot, const SvBoardPose& pose) {
    if (!g_rt || slot >= 4) return false;
    static std::vector<std::string> s_models[4];
    static bool s_asked[4] = {};
    if (!s_asked[slot]) s_models[slot] = Split(g_rt->ShopText(100 + static_cast<int>(slot)), ','), s_asked[slot] = true;
    return boardparts::Crew(static_cast<int>(slot), s_models[slot], pose);
}

void ClearCrewBoard(unsigned slot) {
    if (slot < 4) boardparts::Crew(static_cast<int>(slot), {}, SvBoardPose{});
}

} // namespace boardshop

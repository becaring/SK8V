#include "board_parts.h"
#include "board_native.h"
#include "host_util.h"
#include "natives.h"
#include "pose_math.h"
#include "streaming.h"
#include <windows.h>
#include <cstdio>
#include <filesystem>
#include <map>

namespace boardparts {
namespace {
using namespace gta;

struct Model {
    std::uint32_t ydr = ~0u, ytyp = ~0u;
    std::string ydrPath, ytypPath;
    bool typed = false, failed = false, loaded = false;
    ULONGLONG requested = 0;
};
struct Shown {
    std::string name;
    int object = 0;
    boardnative::PoseCache cache;
};

void (*g_log)(const char*) = nullptr;
std::filesystem::path g_root;
bool g_started = false;
std::map<std::string, Model> g_models;  // registrations persist for the process
// A set of part objects making one board: the ridden one and the store's preview.
struct Set {
    std::vector<std::string> models;
    std::vector<Shown> shown;
    bool announced = false;
    int interior = 0;            // the room the objects are kept in (the player's)
    std::uint32_t room = 0;
    ULONGLONG waitingSince = 0;  // not all parts loaded yet (for the log)
    bool notRigidLogged = false;
};
Set g_ride, g_preview;
Set g_display[8];
Set g_crew[4];
Set g_hand;

bool ShownElsewhere(const std::string& name, const Set& except) {
    auto in = [&](const Set& set) {
        if (&set == &except) return false;
        for (const Shown& s : set.shown)
            if (s.name == name) return true;
        return false;
    };
    if (in(g_ride) || in(g_preview)) return true;
    for (const Set& set : g_display)
        if (in(set)) return true;
    for (const Set& set : g_crew)
        if (in(set)) return true;
    return in(g_hand);
}

void Log(const char* s) {
    if (g_log) g_log(s);
}

void Logf(const char* format, const char* a, const char* b = "") {
    char line[320];
    std::snprintf(line, sizeof(line), format, a, b);
    Log(line);
}

// Registers the part's loose YDR/YTYP once and asks for its model; true when loaded.
// GTA's loose-file registration and type load, guarded: in a long store visit
// (hundreds of parts looked at) the registrar wrote through a null pointer
// inside GTA and Script Hook V stopped the whole script (10-11). A fault
// here fails that one part instead.
bool SafeRegister(streaming::Register reg, std::uint32_t* out, const char* path, const char* name) {
    __try {
        reg(out, path, true, name, false, false);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

bool SafeTypeLoad(streaming::TypeLoad load, const char* name) {
    __try {
        load(name);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

// New part models registered this game session; past this the native board
// shows (GTA's streaming has room for a few hundred loose files).
constexpr int kMaxRegistered = 240;
int g_registered = 0;

bool Ready(const std::string& name) {
    Model& m = g_models[name];
    if (m.failed) return false;
    const std::uint32_t hash = util::Joaat(name);
    if (!m.typed) {
        if (!streaming::Ready()) return false;
        if (Call<bool>(IS_MODEL_IN_CDIMAGE, hash)) {
            Logf("board parts: %s: model name already occupied", name.c_str());
            m.failed = true;
            return false;
        }
        m.ydrPath = streaming::NarrowAnsi(g_root / (name + ".ydr"));
        m.ytypPath = streaming::NarrowAnsi(g_root / (name + ".ytyp"));
        if (m.ydrPath.empty() || m.ytypPath.empty() || !std::filesystem::is_regular_file(g_root / (name + ".ydr"))) {
            Logf("board parts: %s: no model file", name.c_str());
            m.failed = true;
            return false;
        }
        if (g_registered >= kMaxRegistered) {
            static bool s_said = false;
            if (!s_said) s_said = true, Logf("board parts: %s: %s part models this session, no more (restart GTA for others)", name.c_str(), std::to_string(g_registered).c_str());
            m.failed = true;
            return false;
        }
        const auto reg = streaming::Registrar();
        const std::string ydrName = name + ".ydr", ytypName = name + ".ytyp";
        if (!SafeRegister(reg, &m.ydr, m.ydrPath.c_str(), ydrName.c_str()) ||
            !SafeRegister(reg, &m.ytyp, m.ytypPath.c_str(), ytypName.c_str())) {
            Logf("board parts: %s: GTA faulted registering it (%s registered)", name.c_str(), std::to_string(g_registered).c_str());
            m.failed = true;
            g_registered = kMaxRegistered;  // GTA's streaming is unwell: no more this session
            return false;
        }
        ++g_registered;
        if (m.ydr == ~0u || m.ytyp == ~0u) {
            Logf("board parts: %s: registration failed", name.c_str());
            m.failed = true;
            return false;
        }
        if (!SafeTypeLoad(streaming::TypeLoader(), ytypName.c_str())) {
            Logf("board parts: %s: GTA faulted loading its type", name.c_str());
            m.failed = true;
            g_registered = kMaxRegistered;
            return false;
        }
        m.typed = true;
        m.requested = GetTickCount64();
    }
    if (Call<bool>(HAS_MODEL_LOADED, hash)) {
        m.loaded = true;
        return true;
    }
    // Let go earlier (RemoveAll) and wanted again: the wait starts now.
    if (m.loaded) m.loaded = false, m.requested = GetTickCount64();
    if (Call<bool>(IS_MODEL_VALID, hash)) Call<void>(REQUEST_MODEL, hash);
    if (GetTickCount64() - m.requested > 15000) {
        Logf("board parts: %s: model load timed out", name.c_str());
        m.failed = true;
    }
    return false;
}

void Remove(Shown& s) {
    if (s.object && Call<bool>(DOES_ENTITY_EXIST, s.object)) Call<void>(DELETE_OBJECT, &s.object);
    s.object = 0;
}

bool ShownElsewhere(const std::string& name, const Set& except);

// Removes the set's objects; a model no other set shows is let go, so
// browsing the store does not keep every part it showed in memory.
void RemoveAll(Set& set) {
    for (Shown& s : set.shown) Remove(s);
    for (const Shown& s : set.shown)
        if (!ShownElsewhere(s.name, set)) Call<void>(SET_MODEL_AS_NO_LONGER_NEEDED, util::Joaat(s.name));
    set.shown.clear();
}

int Create(const std::string& name, const SvBoardPose& pose) {
    const int object = Call<int>(CREATE_OBJECT_NO_OFFSET, util::Joaat(name), pose.entity[12], pose.entity[13],
                                 pose.entity[14], false, false, false, 0);
    if (!object) return 0;
    // Presentation only, as the native board.
    Call<void>(SET_ENTITY_VISIBLE, object, false, false);
    Call<void>(SET_ENTITY_AS_MISSION_ENTITY, object, true, false);
    Call<void>(FREEZE_ENTITY_POSITION, object, true);
    Call<void>(SET_ENTITY_HAS_GRAVITY, object, false);
    Call<void>(SET_ENTITY_DYNAMIC, object, false);
    Call<void>(SET_ENTITY_COLLISION, object, false, false);
    Call<void>(SET_ENTITY_COMPLETELY_DISABLE_COLLISION, object, false, false);
    return object;
}

} // namespace

bool Start(const std::wstring& dataRoot, void (*log)(const char*)) {
    g_log = log;
    g_root = std::filesystem::path(dataRoot) / L"private" / L"board-parts";
    g_started = std::filesystem::is_directory(g_root);
    if (!g_started) Log("board parts: none built (private\\board-parts missing); the native board shows");
    // The raw registration is checked against 3889's code, as the native board's.
    if (g_started && !streaming::Guards()) {
        Log("board parts: 3889 streaming function guards differ; the native board shows");
        g_started = false;
    }
    return g_started;
}

void SetLoadout(const std::vector<std::string>& models) {
    if (models == g_ride.models) return;
    g_ride.models = models;
    g_ride.announced = false;
    RemoveAll(g_ride);
}

const std::vector<std::string>& Loadout() {
    return g_ride.models;
}

bool Available(const std::string& model) {
    return g_started && std::filesystem::is_regular_file(g_root / (model + ".ydr"));
}

namespace {
// Shows `set` posed by `pose`; true when every part is out and posed.
bool Show(Set& set, const SvBoardPose& pose) {
    if (pose.size != sizeof(SvBoardPose) || pose.bone_count != 7) return false;
    for (const auto& bone : pose.world)
        for (float f : bone)
            if (!std::isfinite(f)) return false;
    posemath::M entity;
    std::memcpy(&entity, pose.entity, sizeof(entity));
    posemath::Q q;
    if (!posemath::RigidQuaternion(entity, q)) {
        if (!set.notRigidLogged) set.notRigidLogged = true, Log("board parts: a pose's entity matrix is not a rotation; parts not shown");
        return false;
    }
    bool all = true;
    std::string missing;
    for (const std::string& name : set.models)
        if (!Ready(name)) all = false, missing = name;
    if (!all) {  // the native board shows until every part is in
        const ULONGLONG now = GetTickCount64();
        if (!set.waitingSince) set.waitingSince = now;
        else if (now - set.waitingSince > 3000) {
            Logf("board parts: still waiting for %s%s", missing.c_str(), &set == &g_preview ? " (store preview)" : "");
            set.waitingSince = now + 60000;
        }
        return false;
    }
    set.waitingSince = 0;
    if (set.shown.empty()) {
        for (const std::string& name : set.models) {
            Shown s;
            s.name = name;
            s.object = Create(name, pose);
            if (!s.object) {
                Logf("board parts: %s: CREATE_OBJECT_NO_OFFSET failed", name.c_str());
                g_models[name].failed = true;
                RemoveAll(set);
                return false;
            }
            set.shown.push_back(s);
        }
        if (!set.announced)
            Logf(&set == &g_ride ? "board parts: riding %s%s" : "board parts: store preview of %s%s", set.models.front().c_str(),
                 set.models.size() > 1 ? " and the rest of the setup" : "");
        set.interior = -1;
        set.announced = true;
    }
    bool posed = true;
    for (Shown& s : set.shown) {
        if (!Call<bool>(DOES_ENTITY_EXIST, s.object)) {
            RemoveAll(set);
            return false;
        }
        Call<void>(SET_ENTITY_COORDS_NO_OFFSET, s.object, pose.entity[12], pose.entity[13], pose.entity[14], false, false, false);
        Call<void>(SET_ENTITY_QUATERNION, s.object, q.x, q.y, q.z, q.w);
        posed = boardnative::PoseObject(s.object, pose, s.cache, g_log) && posed;
    }
    // Inside an interior GTA draws only what is in its rooms: the parts go
    // into the player's room (a store), and out again outside.
    const int ped = Call<int>(PLAYER_PED_ID);
    const int interior = Call<int>(GET_INTERIOR_FROM_ENTITY, ped);
    const std::uint32_t room = interior ? Call<std::uint32_t>(GET_ROOM_KEY_FROM_ENTITY, ped) : 0;
    if (interior != set.interior || room != set.room) {
        for (Shown& s : set.shown) {
            if (interior && room) Call<void>(FORCE_ROOM_FOR_ENTITY, s.object, interior, room);
            else Call<void>(CLEAR_ROOM_FOR_ENTITY, s.object);
        }
        set.interior = interior, set.room = room;
    }
    for (Shown& s : set.shown) Call<void>(SET_ENTITY_VISIBLE, s.object, posed, false);
    return posed;
}
} // namespace

bool Tick(bool skating, const SvBoardPose& pose) {
    if (!g_started || g_ride.models.empty() || !skating) {
        RemoveAll(g_ride);
        return false;
    }
    return Show(g_ride, pose);
}

bool Preview(const std::vector<std::string>& models, const SvBoardPose& pose) {
    if (!g_started || models.empty()) {
        RemoveAll(g_preview);
        g_preview.models.clear();
        return false;
    }
    if (models != g_preview.models) {
        RemoveAll(g_preview);
        g_preview.models = models;
    }
    return Show(g_preview, pose);
}

bool Display(int index, const std::vector<std::string>& models, const SvBoardPose& pose) {
    if (!g_started || index < 0 || index >= 8) return false;
    Set& set = g_display[index];
    if (models != set.models) {
        RemoveAll(set);
        set.models = models;
        set.announced = true;  // boards on show are logged by the shop
    }
    return !models.empty() && Show(set, pose);
}

void ClearDisplay() {
    for (Set& set : g_display) {
        RemoveAll(set);
        set.models.clear();
    }
}

bool Hand(const std::vector<std::string>& models, const SvBoardPose& pose) {
    if (!g_started) return false;
    if (models != g_hand.models) {
        RemoveAll(g_hand);
        g_hand.models = models;
        g_hand.announced = true;
    }
    return !models.empty() && Show(g_hand, pose);
}

bool Crew(int slot, const std::vector<std::string>& models, const SvBoardPose& pose) {
    if (!g_started || slot < 0 || slot >= 4) return false;
    Set& set = g_crew[slot];
    if (models != set.models) {
        RemoveAll(set);
        set.models = models;
        set.announced = true;
    }
    return !models.empty() && Show(set, pose);
}

std::vector<int> Entities() {
    std::vector<int> out;
    for (const Set* set : {&g_ride, &g_preview})
        for (const Shown& s : set->shown)
            if (s.object) out.push_back(s.object);
    for (const Shown& s : g_hand.shown)
        if (s.object) out.push_back(s.object);
    for (const Set* group : {g_display, g_crew})
        for (std::size_t i = 0; i < (group == g_display ? 8u : 4u); ++i)
            for (const Shown& s : group[i].shown)
                if (s.object) out.push_back(s.object);
    return out;
}

void Stop() {
    RemoveAll(g_ride);
}

} // namespace boardparts

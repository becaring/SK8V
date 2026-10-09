#include "map_states.h"
#include <windows.h>
#include <main.h>
#include <filesystem>
#include <fstream>
#include <sstream>
#include "natives.h"
#include "host_util.h"

namespace mapstates {
namespace {
using gta::Call;

struct State {
    std::string name;
    std::uint32_t hash = 0;
    float lo[2] = {};
    float hi[2] = {};
};
std::vector<State> g_states;
std::vector<std::uint32_t> g_last;
bool g_sent = false;

// Ask about states whose bounds come within this many metres of the player:
// the runtime builds 128 m around the skater and the rebuild trails by up to
// 64 m, so a state is known before its collision can be needed.
constexpr float kReach = 256.0f;
}  // namespace

std::size_t Load(const std::string& worldCache) {
    g_states.clear();
    g_last.clear();
    g_sent = false;
    std::filesystem::path stem(worldCache);
    stem.replace_extension();
    std::ifstream in(stem.string() + ".map-states.txt");
    std::string line;
    while (std::getline(in, line)) {
        if (line.empty() || line[0] == '#') continue;
        std::istringstream f(line);
        State s;
        std::size_t triangles = 0;
        float z0 = 0, z1 = 0;
        if (!(f >> s.name >> triangles >> s.lo[0] >> s.lo[1] >> z0 >> s.hi[0] >> s.hi[1] >> z1)) continue;
        s.hash = util::Joaat(s.name);
        g_states.push_back(std::move(s));
    }
    return g_states.size();
}

bool Refresh(float x, float y, bool force, std::vector<std::uint32_t>& active, std::string& names) {
    active.clear();
    names.clear();
    for (const State& s : g_states) {
        if (x < s.lo[0] - kReach || x > s.hi[0] + kReach || y < s.lo[1] - kReach || y > s.hi[1] + kReach) continue;
        if (!Call<BOOL>(gta::IS_IPL_ACTIVE, s.name.c_str())) continue;
        active.push_back(s.hash);
        if (!names.empty()) names += ", ";
        names += s.name;
    }
    if (!force && g_sent && active == g_last) return false;
    g_last = active;
    g_sent = true;
    return true;
}

}  // namespace mapstates

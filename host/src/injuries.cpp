#include "injuries.h"
#include "natives.h"
#include <windows.h>
#include <algorithm>
#include <cstdio>

namespace injuries {
namespace {
using namespace gta;

constexpr float kHard = 9.0f;        // m/s: a knock this hard or more hurts for a while
constexpr const char* kWalk = "move_m@injured";

float g_worst = 0.0f;
ULONGLONG g_until = 0;
bool g_applied = false;

} // namespace

void Hit(float speed) {
    if (speed > g_worst) g_worst = speed;
}

void BailOver(void (*log)(const char*)) {
    const float worst = g_worst;
    g_worst = 0.0f;
    if (worst < kHard) return;
    // 30 s at the threshold, 12 s more per m/s, at most 2 minutes.
    const float seconds = std::min(120.0f, 30.0f + (worst - kHard) * 12.0f);
    const ULONGLONG until = GetTickCount64() + static_cast<ULONGLONG>(seconds * 1000.0f);
    if (until > g_until) g_until = until;
    if (log) {
        char line[96];
        std::snprintf(line, sizeof(line), "injuries: a %.1f m/s knock: limping for %.0f s", worst, seconds);
        log(line);
    }
}

void Tick(int ped, bool walking, bool enabled, void (*log)(const char*)) {
    const bool hurt = enabled && GetTickCount64() < g_until;
    if (hurt && walking) {
        Call<void>(REQUEST_ANIM_SET, kWalk);
        if (!g_applied && Call<BOOL>(HAS_ANIM_SET_LOADED, kWalk)) {
            Call<void>(SET_PED_MOVEMENT_CLIPSET, ped, kWalk, 1.0f);
            g_applied = true;
        }
    } else if (g_applied && (!hurt || !walking)) {
        Call<void>(RESET_PED_MOVEMENT_CLIPSET, ped, 1.0f);
        g_applied = false;
        if (!hurt && log) log("injuries: walking normally again");
    }
    if (!enabled) g_until = 0;
}

} // namespace injuries

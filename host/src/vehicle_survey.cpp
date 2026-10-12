#include "vehicle_survey.h"
#include "natives.h"
#include "runtime_bridge.h"
#include <windows.h>
#include <cstdint>
#include <cstdio>

namespace vehiclesurvey {
namespace {
constexpr float kReach = 20.0f;
using namespace gta;

// How the last survey's probes went: no result, nothing hit, another
// vehicle hit, this one hit.
struct Tally {
    int vehicle = 0;
    float min[3]{}, max[3]{};
    int pending = 0, none = 0, other = 0, hits = 0;
};

// One probe against the surveyed vehicle, in its model space: from and to
// become world points. A hit counts when it is that vehicle, or when its
// point lies inside the vehicle's box (the line runs inside the box and meets
// only vehicles, so it can only be this one's body).
std::uint32_t __cdecl Probe(void* ctx, const float* from, const float* to, float* hit) {
    Tally& t = *static_cast<Tally*>(ctx);
    const Vector3 a = Call<Vector3>(GET_OFFSET_FROM_ENTITY_IN_WORLD_COORDS, t.vehicle, from[0], from[1], from[2]);
    const Vector3 b = Call<Vector3>(GET_OFFSET_FROM_ENTITY_IN_WORLD_COORDS, t.vehicle, to[0], to[1], to[2]);
    const int test = Call<int>(START_EXPENSIVE_SYNCHRONOUS_SHAPE_TEST_LOS_PROBE, a.x, a.y, a.z, b.x, b.y, b.z, 2 | 16,
                               Call<int>(PLAYER_PED_ID), 7);
    BOOL any = 0;
    Vector3 end{}, normal{};
    Entity entity = 0;
    if (Call<int>(GET_SHAPE_TEST_RESULT, test, &any, &end, &normal, &entity) != 2) return ++t.pending, 0;
    if (!any) return ++t.none, 0;
    const Vector3 local = Call<Vector3>(GET_OFFSET_FROM_ENTITY_GIVEN_WORLD_COORDS, t.vehicle, end.x, end.y, end.z);
    const bool inside = local.x > t.min[0] - 0.1f && local.x < t.max[0] + 0.1f && local.y > t.min[1] - 0.1f &&
                        local.y < t.max[1] + 0.1f && local.z > t.min[2] - 0.1f && local.z < t.max[2] + 0.1f;
    if (entity != t.vehicle && !inside) return ++t.other, 0;
    ++t.hits;
    hit[0] = local.x, hit[1] = local.y, hit[2] = local.z;
    return 1;
}

} // namespace

void Tick(RuntimeBridge& runtime, float radius, void (*log)(const char*)) {
    if (!runtime.HasVehicleSurvey()) return;
    // The pool scan twice a second is plenty: a model is surveyed only once.
    static unsigned long s_next = 0;
    const unsigned long now = GetTickCount();
    if (static_cast<long>(now - s_next) < 0) return;
    s_next = now + 500;
    static int vehicles[256];
    const int n = worldGetAllVehicles(vehicles, 256);
    const int ped = Call<int>(PLAYER_PED_ID);
    const Vector3 me = Call<Vector3>(GET_ENTITY_COORDS, ped, 0);
    // GTA's shape tests met no vehicle farther than about 20 m (the first
    // test: every far survey hit nothing, the same model close by did), so
    // the nearest vehicle within that goes first.
    const float reach = radius < kReach ? radius : kReach;
    int v = 0;
    std::uint32_t model = 0;
    float best = reach * reach;
    for (int pass = 0; pass < 8 && !v; ++pass) {
        int pick = 0;
        float pickD = best;
        for (int i = 0; i < n; ++i) {
            const Vector3 p = Call<Vector3>(GET_ENTITY_COORDS, vehicles[i], 0);
            const float dx = p.x - me.x, dy = p.y - me.y, dz = p.z - me.z, d = dx * dx + dy * dy + dz * dz;
            if (d < pickD && vehicles[i]) pick = i + 1, pickD = d;
        }
        if (!pick) break;
        const int candidate = vehicles[pick - 1];
        vehicles[pick - 1] = 0;  // not again this call
        const std::uint32_t m = Call<std::uint32_t>(GET_ENTITY_MODEL, candidate);
        if (runtime.VehicleNeedsShape(m)) v = candidate, model = m;
    }
    if (!v) return;
    {
        Vector3 lo{}, hi{};
        Call<void>(GET_MODEL_DIMENSIONS, model, &lo, &hi);
        Tally t{v, {lo.x, lo.y, lo.z}, {hi.x, hi.y, hi.z}};
        const int tris = runtime.VehicleSurvey(model, t.min, t.max, Probe, &t);
        if (log) {
            char line[240];
            std::snprintf(line, sizeof(line),
                          "vehicle shape: model %08x surveyed: %d triangles%s (%.1f x %.1f x %.1f m; probes: %d hit, %d other, %d "
                          "nothing, %d no result)",
                          model, tris, tris <= 10 ? " = its box" : "", hi.x - lo.x, hi.y - lo.y, hi.z - lo.z, t.hits, t.other,
                          t.none, t.pending);
            log(line);
        }
    }
}

} // namespace vehiclesurvey

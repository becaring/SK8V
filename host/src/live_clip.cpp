#include "live_clip.h"

#include "live_clip_layout.h"
#include "host_util.h"
#include "natives.h"

#include <windows.h>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <thread>
#include <vector>

namespace liveclip {
namespace {

constexpr const char* kDict = "skatev_live";
constexpr const char* kClip = "skatev_body";
constexpr int kMaxBones = 512;

// Marker frame-0 x; channel k of frame f is at base + (k - kMarkerChannel) + f * kChannels.
std::atomic<float*> g_base{nullptr};
std::atomic<int> g_scan{0}; // 0 idle, 1 running, 2 done
std::atomic<int> g_hits{0};
std::atomic<std::uint64_t> g_writes{0};
const Track* g_map[kMaxBones]{}; // skeleton bone index -> clip track
std::uintptr_t g_mapped = 0;
bool g_playing = false, g_flip = false, g_suspended = false;
constexpr std::uint32_t kPlayAnimTask = 0x87B9A382; // SCRIPT_TASK_PLAY_ANIM
DWORD g_taskedAt = 0;
int g_tasked = 0;
// First-write convention check (Compare): worst |dot| of GTA's local rotation
// against the clip's, as stored and conjugated; worst translation difference.
std::atomic<int> g_compare{0}; // 0 pending, 1 done (not yet logged), 2 logged
float g_dot = 2, g_dotConj = 2, g_dt = 0;
int g_compared = 0;
DWORD g_lastReport = 0;

bool MarkerAt(const float* m) {
    return std::memcmp(m, kMarker[0], 12) == 0 && std::memcmp(m + kChannels, kMarker[1], 12) == 0;
}

// Committed private read-write memory, read through ReadProcessMemory so a
// region freed under the scan fails the read instead of faulting.
void Scan() {
    const HANDLE self = GetCurrentProcess();
    constexpr SIZE_T kChunk = 1 << 20;
    std::vector<std::uint8_t> buf(kChunk + 4 * (kChannels + 3));
    float* found = nullptr;
    int hits = 0;
    MEMORY_BASIC_INFORMATION mbi{};
    for (auto* a = static_cast<std::uint8_t*>(nullptr); VirtualQuery(a, &mbi, sizeof(mbi)) == sizeof(mbi);
         a = static_cast<std::uint8_t*>(mbi.BaseAddress) + mbi.RegionSize) {
        if (mbi.State != MEM_COMMIT || mbi.Type != MEM_PRIVATE || mbi.Protect != PAGE_READWRITE) continue;
        auto* lo = static_cast<std::uint8_t*>(mbi.BaseAddress);
        auto* hi = lo + mbi.RegionSize;
        for (auto* p = lo; p < hi; p += kChunk) {
            // Overlap so a marker whose frame-1 copy is in the next chunk is still seen.
            const SIZE_T want = static_cast<SIZE_T>(std::min<std::uintptr_t>(buf.size(), hi - p));
            SIZE_T got = 0;
            if (!ReadProcessMemory(self, p, buf.data(), want, &got) || got < 12) continue;
            const SIZE_T scan = std::min<SIZE_T>(got, kChunk);
            for (SIZE_T o = 0; o + 12 <= scan; o += 4) {
                if (std::memcmp(buf.data() + o, kMarker[0], 12) != 0) continue;
                if (o + 4 * kChannels + 12 > got) continue;
                if (!MarkerAt(reinterpret_cast<const float*>(buf.data() + o))) continue;
                ++hits;
                if (!found) found = reinterpret_cast<float*>(p + o);
            }
        }
    }
    g_hits = hits;
    g_base = hits == 1 ? found : nullptr; // two copies: which one GTA evaluates is unknown
    g_scan = 2;
}

void Map(std::uintptr_t skeleton) {
    g_mapped = skeleton;
    std::memset(g_map, 0, sizeof(g_map));
    const std::uintptr_t data = *reinterpret_cast<const std::uintptr_t*>(skeleton);
    const std::uintptr_t bones = data ? *reinterpret_cast<const std::uintptr_t*>(data + 0x20) : 0;
    const int count = *reinterpret_cast<const int*>(skeleton + 0x20);
    for (int i = 0; bones && i < count && i < kMaxBones; ++i) {
        const std::uint16_t tag = *reinterpret_cast<const std::uint16_t*>(bones + 80 * i + 0x44);
        for (const Track& t : kTracks)
            if (t.tag == tag) g_map[i] = &t;
    }
}

} // namespace

void Tick(int ped, std::uintptr_t skeleton, void (*log)(const char*)) {
    using gta::Call;
    if (skeleton && skeleton != g_mapped) Map(skeleton);
    if (!Call<BOOL>(gta::HAS_ANIM_DICT_LOADED, kDict)) {
        Call<void>(gta::REQUEST_ANIM_DICT, kDict);
        return;
    }
    const int status = Call<int>(gta::GET_SCRIPT_TASK_STATUS, ped, kPlayAnimTask);
    // Left to GTA while the gun is out (Suspend).
    if (!g_suspended) {
        // Tasked once; again only when GTA reports no play-anim task (re-tasking
        // every tick while IS_ENTITY_PLAYING_ANIM was still false restarted it
        // before it began). The task runs once the per-tick placement keeps tasks.
        if (!g_playing || (status == 7 && GetTickCount() - g_taskedAt > 1000)) {
            // Looped, held by the task; blend in over 125 ms.
            Call<void>(gta::TASK_PLAY_ANIM, ped, kDict, kClip, 8.0f, -8.0f, -1, 1, 0.0f, 0, 0, 0);
            g_playing = true;
            g_taskedAt = GetTickCount();
            ++g_tasked;
        }
        // Pinned between the two (identical) frames, the phase moved every tick
        // so GTA evaluates the clip afresh (B4).
        Call<void>(gta::SET_ENTITY_ANIM_SPEED, ped, kDict, kClip, 0.0f);
        Call<void>(gta::SET_ENTITY_ANIM_CURRENT_TIME, ped, kDict, kClip, (g_flip = !g_flip) ? 0.25f : 0.75f);
    }
    if (!g_base.load() && g_scan.load() != 1) {
        g_scan = 1;
        std::thread(Scan).detach();
    }
    if (g_compare.load() == 1) {
        char line[256];
        std::snprintf(line, sizeof(line),
                      "live clip: convention check over %d bones: min |dot| as stored %.4f, conjugated %.4f; max translation diff %.4f m",
                      g_compared, g_dot, g_dotConj, g_dt);
        log(line);
        g_compare = 2;
    }
    const DWORD now = GetTickCount();
    if (util::g_verbose && now - g_lastReport >= 5000) {
        g_lastReport = now;
        const bool playing = Call<BOOL>(gta::IS_ENTITY_PLAYING_ANIM, ped, kDict, kClip, 3) != 0;
        int mapped = 0;
        for (const Track* t : g_map) mapped += t != nullptr;
        char line[256];
        std::snprintf(line, sizeof(line), "live clip: playing %d, task status %d (tasked %d), scan %d, marker hits %d, data %p, bones mapped %d, writes %llu",
                      static_cast<int>(playing), status, g_tasked, g_scan.load(),
                      g_hits.load(), static_cast<void*>(g_base.load()), mapped,
                      static_cast<unsigned long long>(g_writes.load()));
        log(line);
    }
}

void Stop(int ped) {
    if (!g_playing) return;
    g_playing = false;
    gta::Call<void>(gta::STOP_ANIM_TASK, ped, kDict, kClip, -4.0f);
}

void Suspend(int ped, bool on) {
    g_suspended = on;
    if (on) Stop(ped); // off: Tick tasks the clip again (g_playing is false)
}

bool BeginWrite() {
    float* base = g_base.load(std::memory_order_acquire);
    if (!base) return false;
    if (!MarkerAt(base)) { // the dictionary was unloaded or moved
        g_base = nullptr;
        if (g_scan.load() == 2) g_scan = 0;
        return false;
    }
    g_writes.fetch_add(1, std::memory_order_relaxed);
    return true;
}

bool Comparing() { return g_compare.load(std::memory_order_relaxed) == 0; }

void Compare(int index, const float t[3], const float q[4]) {
    if (index < 0 || index >= kMaxBones || !g_map[index]) return;
    const float* c = g_base.load(std::memory_order_relaxed) - kMarkerChannel;
    const Track& k = *g_map[index];
    const float* cq = c + k.q;
    const float* ct = c + k.t;
    g_dot = std::min(g_dot, std::fabs(q[0] * cq[0] + q[1] * cq[1] + q[2] * cq[2] + q[3] * cq[3]));
    g_dotConj = std::min(g_dotConj, std::fabs(-q[0] * cq[0] - q[1] * cq[1] - q[2] * cq[2] + q[3] * cq[3]));
    for (int i = 0; i < 3; ++i) g_dt = std::max(g_dt, std::fabs(t[i] - ct[i]));
    ++g_compared;
}

void EndCompare() { g_compare = 1; }

void WriteBone(int index, const float t[3], const float q[4]) {
    if (index < 0 || index >= kMaxBones || !g_map[index]) return;
    float* base = g_base.load(std::memory_order_relaxed);
    const Track& k = *g_map[index];
    for (int f = 0; f < 2; ++f) {
        float* frame = base - kMarkerChannel + f * kChannels;
        std::memcpy(frame + k.t, t, 12);
        std::memcpy(frame + k.q, q, 16);
    }
}

} // namespace liveclip

// Adapted for Legacy from Sol4ra's LS-Skate-LiveCollision (GTA V Enhanced),
// shared with SK8V by its author (see physics_level.h).
#include "physics_level.h"

#include "game_probe.h"

#include <windows.h>
#include <psapi.h>
#include <main.h>
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <thread>
#include <unordered_map>
#include <vector>

#include "host_util.h"

namespace physlevel {
namespace {

constexpr std::uint64_t kMask = 0x7FFFFFFFFFF0ull; // phInst pointer without the slot's state bits
constexpr std::uint64_t kStride = 0x30;            // one table record

void (*g_log)(const char*) = nullptr;
void (*g_onTable)(std::uintptr_t, std::uintptr_t, std::uintptr_t) = nullptr;
std::atomic<std::uintptr_t> g_reported{0}; // the table the runtime was last told about
std::uintptr_t g_imgLo = 0, g_imgHi = 0;
std::atomic<bool> g_running{false};
DWORD g_nextTry = 0;

template <class... A>
void Logf(const char* fmt, A... a) {
    util::Logf(g_log, fmt, a...);
}

template <class T>
bool Rd(std::uintptr_t a, T& v) { return probe::ReadLive(a, &v, sizeof v); }
// ReadLive refuses more than 64 KiB at once; the table and polygon arrays are bigger.
bool RdBig(std::uintptr_t a, void* out, size_t n) {
    for (size_t o = 0; o < n; o += 0x8000) {
        if (!probe::ReadLive(a + o, static_cast<std::uint8_t*>(out) + o, std::min<size_t>(0x8000, n - o))) return false;
    }
    return true;
}
bool HeapPtr(std::uint64_t v) { return v > 0x10000 && v < 0x7FFFFFFFFFFFull && (v & 7) == 0; }
bool InImage(std::uint64_t v) { return v >= g_imgLo && v < g_imgHi; }
double NowMs() {
    LARGE_INTEGER q, f;
    QueryPerformanceCounter(&q);
    QueryPerformanceFrequency(&f);
    return double(q.QuadPart) * 1000.0 / double(f.QuadPart);
}

// Find the table: every private read-write cell holding one of `insts` (exact
// or with state bits) votes for the table base its level index implies.
std::uintptr_t Locate(std::vector<std::uint64_t> insts) {
    std::sort(insts.begin(), insts.end());
    insts.erase(std::unique(insts.begin(), insts.end()), insts.end());
    std::vector<std::uint32_t> index(insts.size(), 0xFFFFFFFFu);
    for (size_t k = 0; k < insts.size(); ++k) {
        std::uint16_t i = 0;
        if (Rd(insts[k] + 0x18, i)) index[k] = i;
    }
    const auto self = reinterpret_cast<std::uintptr_t>(insts.data());
    const auto selfEnd = self + insts.size() * 8;
    std::unordered_map<std::uint64_t, std::uint32_t> votes;
    std::uint64_t best = 0;
    std::uint32_t top = 0;
    // Settled once one base holds nearly every instance that has a level index.
    const auto indexed = static_cast<std::uint32_t>(std::count_if(index.begin(), index.end(), [](std::uint32_t i) { return i != 0xFFFFFFFFu; }));
    const std::uint32_t settled = std::max<std::uint32_t>(8, indexed - indexed / 10);
    constexpr size_t kPage = 0x1000, kChunk = 1 << 20;
    std::vector<std::uint8_t> buf(kChunk);
    std::vector<PSAPI_WORKING_SET_EX_INFORMATION> ws(kChunk / kPage);
    const HANDLE me = GetCurrentProcess();
    MEMORY_BASIC_INFORMATION mbi{};
    for (std::uintptr_t a = 0x10000; top < settled && a < 0x7FFFFFFF0000ull && VirtualQuery(reinterpret_cast<void*>(a), &mbi, sizeof mbi) == sizeof mbi;
         a = reinterpret_cast<std::uintptr_t>(mbi.BaseAddress) + mbi.RegionSize) {
        if (mbi.State != MEM_COMMIT || mbi.Type != MEM_PRIVATE || (mbi.Protect & PAGE_GUARD) || !(mbi.Protect & PAGE_READWRITE)) continue;
        const auto lo = reinterpret_cast<std::uintptr_t>(mbi.BaseAddress), hi = lo + mbi.RegionSize;
        for (auto p = lo; p < hi && top < settled; p += kChunk) {
            const SIZE_T want = std::min<std::uintptr_t>(kChunk, hi - p);
            // Only pages in RAM: the table is touched every physics frame, and reading paged-out
            // memory pulls it back from disk (a hard disk: 33 s searches and stalled frames, 2026-10-09).
            const size_t pages = want / kPage;
            for (size_t k = 0; k < pages; ++k) ws[k].VirtualAddress = reinterpret_cast<void*>(p + k * kPage);
            const bool known = QueryWorkingSetEx(me, ws.data(), static_cast<DWORD>(pages * sizeof ws[0])) != 0;
            for (size_t k0 = 0; k0 < pages;) {
                if (known && !ws[k0].VirtualAttributes.Valid) { ++k0; continue; }
                size_t k1 = k0 + 1;
                while (k1 < pages && (!known || ws[k1].VirtualAttributes.Valid)) ++k1;
                SIZE_T got = 0;
                const auto run = p + k0 * kPage;
                ReadProcessMemory(me, reinterpret_cast<void*>(run), buf.data(), (k1 - k0) * kPage, &got);
                for (SIZE_T o = 0; o + 8 <= got; o += 8) {
                    std::uint64_t v;
                    std::memcpy(&v, &buf[o], 8);
                    const std::uint64_t m = v & kMask;
                    if (m < insts.front() || m > insts.back()) continue;
                    const auto at = run + o;
                    if (at >= self && at < selfEnd) continue;
                    const auto it = std::lower_bound(insts.begin(), insts.end(), m);
                    if (it == insts.end() || *it != m) continue;
                    const std::uint32_t ix = index[it - insts.begin()];
                    if (ix == 0xFFFFFFFFu || at < ix * kStride) continue;
                    const auto n = ++votes[at - ix * kStride];
                    if (n > top) { top = n; best = at - ix * kStride; }
                }
                k0 = k1;
            }
        }
    }
    Logf("physics level: locate: %zu instances named by GTA, best table candidate %llx with %u hits", insts.size(),
         static_cast<unsigned long long>(best), top);
    return top >= std::max<size_t>(8, insts.size() / 2) ? best : 0;
}

struct Census {
    int used = 0, last = -1, mismatch = 0;
};

// One pass over the table: live instances (vtable in the image) and their
// level index against the slot. Returns false when it no longer looks like one.
// Mismatches are slots still holding a freed instance whose memory was reused; the runtime
// skips them (live.rs), and after much streaming they outnumber the live ones (owner runs
// 2026-10-07: 499/499 named instances at their slots, 4600 of 6000 slots stale), so only
// instances that name their own slot count.
bool Walk(std::uintptr_t base, Census& c) {
    std::vector<std::uint8_t> chunk(4096 * kStride);
    int empties = 0;
    for (std::uint32_t i0 = 0; i0 < 0x10000 && empties <= 4096; i0 += 4096) {
        if (!RdBig(base + i0 * kStride, chunk.data(), chunk.size())) break;
        for (std::uint32_t j = 0; j < 4096; ++j) {
            const std::uint32_t i = i0 + j;
            std::uint64_t slot;
            std::memcpy(&slot, &chunk[j * kStride], 8);
            const std::uint64_t inst = slot & kMask;
            if (!inst) {
                if (++empties > 4096 && c.last >= 0) break;
                continue;
            }
            empties = 0;
            std::uint8_t ih[0x1A];
            std::uint64_t vt;
            if (!HeapPtr(inst) || !probe::ReadLive(inst, ih, sizeof ih)) continue;
            std::memcpy(&vt, ih, 8);
            if (!InImage(vt)) continue;
            ++c.used;
            c.last = int(i);
            std::uint16_t li;
            std::memcpy(&li, ih + 0x18, 2);
            if (li != i) ++c.mismatch;
        }
    }
    return c.used - c.mismatch >= 8;
}

// Tells the runtime where the table is (0: withdrawn), once per change.
void Report(std::uintptr_t table) {
    if (g_reported.exchange(table) == table) return;
    if (g_onTable) g_onTable(table, table ? g_imgLo : 0, table ? g_imgHi : 0);
}

void Worker(std::vector<std::uint64_t> insts) {
    const double t0 = NowMs();
    const std::uintptr_t base = Locate(std::move(insts));
    if (!base) {
        Logf("physics level: table not found (%.0f ms); will retry", NowMs() - t0);
        Report(0);
        g_running = false;
        return;
    }
    Logf("physics level: table at %llx, %.0f ms to find", static_cast<unsigned long long>(base), NowMs() - t0);
    int reports = 0;
    for (int bad = 0; bad < 3;) {
        Census c;
        const double t = NowMs();
        const bool ok = Walk(base, c);
        const double ms = NowMs() - t;
        bad = ok ? 0 : bad + 1;
        if (ok) Report(base);
        if (!ok || reports == 0 || (util::g_verbose && (reports < 4 || reports % 12 == 0)))
            Logf("physics level: %s: %d slots used (last %d), %d level-index mismatches (%.1f ms)", ok ? "ok" : "walk failed",
                 c.used, c.last, c.mismatch, ms);
        ++reports;
        Sleep(5000);
    }
    Logf("physics level: table lost; locating again");
    Report(0);
    g_running = false;
}

} // namespace

void Start(void (*log)(const char*), void (*onTable)(std::uintptr_t, std::uintptr_t, std::uintptr_t)) {
    g_log = log;
    g_onTable = onTable;
    g_reported = 0; // a script restart (leaving the Rockstar Editor) starts a new runtime: tell it again
    auto* base = reinterpret_cast<std::uint8_t*>(GetModuleHandleW(nullptr));
    const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS64*>(base + reinterpret_cast<const IMAGE_DOS_HEADER*>(base)->e_lfanew);
    g_imgLo = reinterpret_cast<std::uintptr_t>(base);
    g_imgHi = g_imgLo + nt->OptionalHeader.SizeOfImage;
    g_nextTry = GetTickCount(); // the locator needs the world's peds, cars and props spawned: Tick waits for 20 of them
    log("physics level: reader on: GTA's loaded collision is the static map once the table reads cleanly");
}

void Tick() {
    if (!g_log) return;
    if (g_running || GetTickCount() < g_nextTry) return;
    std::vector<std::uint64_t> insts;
    int h[1024];
    auto add = [&](int count) {
        for (int i = 0; i < count; ++i) {
            const auto e = reinterpret_cast<std::uintptr_t>(getScriptHandleBaseAddress(h[i]));
            if (const auto f = probe::FragInstOf(e)) insts.push_back(f);
        }
    };
    add(worldGetAllPeds(h, 1024));
    add(worldGetAllVehicles(h, 1024));
    add(worldGetAllObjects(h, 1024));
    // Too few spawned yet: look again shortly (the board waits on this after a spawn); else 5 s between searches.
    g_nextTry = GetTickCount() + (insts.size() < 20 ? 500 : 5000);
    if (insts.size() < 20) return;
    g_running = true;
    std::thread(Worker, std::move(insts)).detach();
}

} // namespace physlevel

#include "stall_sampler.h"
#include "image_code_ranges.h"
#include "host_util.h"
#include <windows.h>
#include <psapi.h>
#include <winternl.h>
#include <algorithm>
#include <atomic>
#include <cstdio>
#include <cstring>
#include <map>

namespace stall {

namespace {
void (*g_log)(const char*) = nullptr;
template <class... A>
void Logf(const char* fmt, A... a) {
    util::Logf(g_log, fmt, a...);
}
} // namespace

void SetLog(void (*log)(const char*)) { g_log = log; }

int FindCode(const std::vector<Module>& modules, std::uintptr_t address) {
    for (std::size_t i = 0; i < modules.size(); ++i)
        for (const auto& [begin, end] : modules[i].code)
            if (address >= begin && address < end) return static_cast<int>(i);
    return -1;
}

bool IsCallSite(const std::uint8_t* b, std::size_t n) {
    // call rel32: E8 xx xx xx xx
    if (n >= 5 && b[n - 5] == 0xE8) return true;
    // call r/m64: FF /2, optionally REX-prefixed (the prefix sits before FF and
    // does not change where FF is). Length from ModRM (+SIB, +displacement).
    for (std::size_t k = 2; k <= 7 && k <= n; ++k) {
        if (b[n - k] != 0xFF) continue;
        const std::uint8_t modrm = b[n - k + 1];
        if (((modrm >> 3) & 7) != 2) continue;
        const unsigned mod = modrm >> 6, rm = modrm & 7;
        std::size_t len = 2;
        if (mod != 3) {
            if (rm == 4) {
                if (k < 3) continue;
                const std::uint8_t sib = b[n - k + 2];
                ++len;
                if (mod == 0 && (sib & 7) == 5) len += 4;
            } else if (mod == 0 && rm == 5) {
                len += 4;
            }
            if (mod == 1) len += 1;
            if (mod == 2) len += 4;
        }
        if (len == k) return true;
    }
    return false;
}

std::string Symbolize(const std::vector<Module>& modules, std::uintptr_t address) {
    char buf[96];
    for (const Module& m : modules) {
        for (const auto& [begin, end] : m.code) {
            if (address >= begin && address < end) {
                std::snprintf(buf, sizeof(buf), "%s+0x%llx", m.name.c_str(), static_cast<unsigned long long>(address - m.base));
                return buf;
            }
        }
    }
    std::snprintf(buf, sizeof(buf), "0x%llx", static_cast<unsigned long long>(address));
    return buf;
}

namespace {

constexpr double kStallMs = 40.0;   // start sampling after this long without a frame
constexpr double kLogMinMs = 60.0;  // log stalls at least this long
constexpr double kLogMaxMs = 5000.0; // longer gaps are menus/loading screens
constexpr DWORD kSampleEveryMs = 4;
constexpr std::size_t kMaxSamples = 64;
constexpr std::size_t kStackBytes = 8192;
constexpr std::size_t kFrames = 10;
constexpr int kMaxLoggedStalls = 400;

struct Sample {
    std::uintptr_t rip = 0, rsp = 0;
    std::size_t bytes = 0;
    std::uint8_t stack[kStackBytes];
};

struct ThreadSlot {
    std::atomic<DWORD> id{0};
    HANDLE handle = nullptr;
    std::uintptr_t teb = 0;
    Sample samples[kMaxSamples];
    std::size_t count = 0;
};

std::atomic<std::int64_t> g_lastBeat{0};
std::atomic<bool> g_started{false}, g_stop{false};
ThreadSlot g_threads[RoleCount];
HANDLE g_watchdog = nullptr;
std::vector<Module> g_modules;
LARGE_INTEGER g_freq{};

std::int64_t Now() {
    LARGE_INTEGER t;
    QueryPerformanceCounter(&t);
    return t.QuadPart;
}
double Ms(std::int64_t ticks) { return static_cast<double>(ticks) * 1000.0 / static_cast<double>(g_freq.QuadPart); }

void RefreshModules() {
    HMODULE handles[1024];
    DWORD needed = 0;
    if (!EnumProcessModules(GetCurrentProcess(), handles, sizeof(handles), &needed)) return;
    std::vector<Module> modules;
    for (DWORD i = 0; i < std::min<DWORD>(needed / sizeof(HMODULE), 1024); ++i) {
        const auto base = reinterpret_cast<std::uintptr_t>(handles[i]);
        const auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(base);
        if (dos->e_magic != IMAGE_DOS_SIGNATURE) continue;
        const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS*>(base + dos->e_lfanew);
        if (nt->Signature != IMAGE_NT_SIGNATURE) continue;
        Module m;
        char name[MAX_PATH]{};
        GetModuleBaseNameA(GetCurrentProcess(), handles[i], name, MAX_PATH);
        m.name = name;
        m.base = base;
        for (const auto& r : gtare::CodeRanges(IMAGE_FIRST_SECTION(nt), nt->FileHeader.NumberOfSections, nt->OptionalHeader.SizeOfImage))
            m.code.emplace_back(base + r.begin, base + r.end);
        modules.push_back(std::move(m));
    }
    g_modules = std::move(modules);
}

bool Open(ThreadSlot& slot, DWORD id) {
    slot.handle = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, FALSE, id);
    if (!slot.handle) return false;
    using QueryFn = NTSTATUS(NTAPI*)(HANDLE, THREADINFOCLASS, PVOID, ULONG, PULONG);
    struct Basic { NTSTATUS exit; PVOID teb; CLIENT_ID client; KAFFINITY affinity; LONG priority, basePriority; } info{};
    const auto query = reinterpret_cast<QueryFn>(GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "NtQueryInformationThread"));
    if (!query || query(slot.handle, static_cast<THREADINFOCLASS>(0), &info, sizeof(info), nullptr) < 0) return false;
    slot.teb = reinterpret_cast<std::uintptr_t>(info.teb);
    return slot.teb != 0;
}

// Suspended: read the context and copy the live stack (the TIB's stack range
// follows fiber switches). No locks, no allocation.
void Take(ThreadSlot& slot) {
    if (!slot.handle || slot.count >= kMaxSamples) return;
    if (SuspendThread(slot.handle) == static_cast<DWORD>(-1)) return;
    CONTEXT ctx{};
    ctx.ContextFlags = CONTEXT_CONTROL;
    Sample& s = slot.samples[slot.count];
    if (GetThreadContext(slot.handle, &ctx)) {
        const auto* tib = reinterpret_cast<const NT_TIB*>(slot.teb);
        const auto base = reinterpret_cast<std::uintptr_t>(tib->StackBase);
        const auto limit = reinterpret_cast<std::uintptr_t>(tib->StackLimit);
        s.rip = ctx.Rip;
        s.rsp = ctx.Rsp;
        s.bytes = 0;
        if (ctx.Rsp >= limit && ctx.Rsp < base) {
            s.bytes = std::min<std::size_t>(kStackBytes, base - ctx.Rsp);
            std::memcpy(s.stack, reinterpret_cast<const void*>(ctx.Rsp), s.bytes);
        }
        ++slot.count;
    }
    ResumeThread(slot.handle);
}

// RIP plus the stack words that are return addresses into module code.
std::vector<std::uintptr_t> Frames(const Sample& s) {
    std::vector<std::uintptr_t> out{s.rip};
    for (std::size_t at = 0; at + 8 <= s.bytes && out.size() < kFrames; at += 8) {
        std::uintptr_t value;
        std::memcpy(&value, s.stack + at, 8);
        const int m = FindCode(g_modules, value);
        if (m < 0) continue;
        std::uintptr_t begin = 0;
        for (const auto& [b, e] : g_modules[m].code)
            if (value >= b && value < e) begin = b;
        if (value - begin < 7) continue;
        if (IsCallSite(reinterpret_cast<const std::uint8_t*>(value - 7), 7)) out.push_back(value);
    }
    return out;
}

void Report(double stallMs, int& logged) {
    MEMORYSTATUSEX mem{sizeof(mem)};
    GlobalMemoryStatusEx(&mem);
    Logf("stall %.1f ms: %zu main / %zu render samples, physical memory %llu MB free of %llu MB (load %lu%%)", stallMs,
         g_threads[Main].count, g_threads[Render].count, mem.ullAvailPhys >> 20, mem.ullTotalPhys >> 20, mem.dwMemoryLoad);
    static const char* const kRole[RoleCount] = {"main", "render"};
    for (int r = 0; r < RoleCount; ++r) {
        std::map<std::string, int> stacks;
        for (std::size_t i = 0; i < g_threads[r].count; ++i) {
            std::string sig;
            for (std::uintptr_t f : Frames(g_threads[r].samples[i])) {
                if (!sig.empty()) sig += " < ";
                sig += Symbolize(g_modules, f);
            }
            ++stacks[sig];
        }
        std::vector<std::pair<int, std::string>> ranked;
        for (auto& [sig, n] : stacks) ranked.emplace_back(n, sig);
        std::sort(ranked.rbegin(), ranked.rend());
        for (std::size_t i = 0; i < ranked.size() && i < 4; ++i)
            Logf("stall   %s x%d: %s", kRole[r], ranked[i].first, ranked[i].second.c_str());
    }
    ++logged;
}

DWORD WINAPI Watchdog(LPVOID) {
    RefreshModules();
    std::int64_t nextRefresh = Now();
    std::int64_t stallBeat = 0; // the beat a stall is being sampled for
    double stallMs = 0.0;
    int logged = 0;
    while (!g_stop.load()) {
        Sleep(kSampleEveryMs);
        const std::int64_t beat = g_lastBeat.load();
        const std::int64_t now = Now();
        if (stallBeat) {
            if (beat != stallBeat) { // frames resumed
                stallMs = Ms(beat - stallBeat);
                if (stallMs >= kLogMinMs && stallMs <= kLogMaxMs && logged < kMaxLoggedStalls) Report(stallMs, logged);
                stallBeat = 0;
                for (auto& t : g_threads) t.count = 0;
            } else {
                for (auto& t : g_threads) Take(t);
            }
            continue;
        }
        if (Ms(now - beat) > kStallMs) {
            stallBeat = beat;
            for (auto& t : g_threads) Take(t);
            continue;
        }
        if (now >= nextRefresh) {
            RefreshModules();
            nextRefresh = now + g_freq.QuadPart * 10;
        }
        const DWORD render = g_threads[Render].id.load();
        if (render && !g_threads[Render].handle && !Open(g_threads[Render], render)) g_threads[Render].id = 0;
    }
    return 0;
}

} // namespace

void Beat() {
    if (!g_started.load()) {
        QueryPerformanceFrequency(&g_freq);
        g_lastBeat = Now();
        const DWORD id = GetCurrentThreadId();
        g_threads[Main].id = id;
        if (!Open(g_threads[Main], id)) {
            Logf("stall sampler: cannot open the main thread (error %lu); disabled", GetLastError());
            g_started = true;
            return;
        }
        g_started = true;
        g_watchdog = CreateThread(nullptr, 0, Watchdog, nullptr, 0, nullptr);
        Logf("stall sampler: watching main thread %lu (sampling after %.0f ms without a script frame)", id, kStallMs);
        return;
    }
    g_lastBeat = Now();
}

void RegisterRender() {
    DWORD expected = 0;
    g_threads[Render].id.compare_exchange_strong(expected, GetCurrentThreadId());
}

void Stop() {
    g_stop = true;
    if (g_watchdog) {
        WaitForSingleObject(g_watchdog, 1000);
        CloseHandle(g_watchdog);
        g_watchdog = nullptr;
    }
}

} // namespace stall

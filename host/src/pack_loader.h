#pragma once
// Loads the SkateV pack, update\x64\dlcpacks\skatev\dlc.rpf, on a clean GTA
// install: no OpenIV, no archive edits. GTA 1.0.3889.0 image:
//   +0x963ED0 reads common:/data/dlclist.xml, mounts each listed path with
//     +0x916734 (call at +0x963F6A), then finishes with +0x929E70 (call at
//     +0x963F7D). We note whether dlcpacks:/skatev/ was listed (an OpenIV mods
//     dlclist) and otherwise mount it just before the finish.
//   +0x136816C opens every RPF7 and always decrypts its table of contents:
//     entries as NG (call at +0x13682F8; the header's encryption is at
//     [rbx+0xB4]), names as NG or AES (+0x136F784, encryption in ecx; call at
//     +0x136839D). There is no clear-text case: OPEN archives load only because
//     OpenIV patches this. Both calls skip decryption for OPEN headers only;
//     retail archives (NG/AES) take the original path.
// Installed from DllMain: ASIs load about 2 s before GTA opens its first archive.
// A call site that does not hold the expected call (another mod hooked it, or
// the code is not unpacked yet) leaves everything unpatched and is retried.
#include "host_util.h"
#include "streaming.h"

#include <windows.h>
#include <cstdint>
#include <cstring>
#include <string>

namespace packloader {
namespace detail {

constexpr std::uintptr_t kMount = 0x916734, kFinish = 0x929e70, kEntries = 0x1374f50, kNames = 0x136f784;
struct Site {
    std::uintptr_t at, target;
};
constexpr Site kSites[6] = {{0x963f6a, kMount},   {0x963f7d, kFinish},  {0x13682f8, kEntries},
                            {0x136839d, kNames}, {0x169b992, 0x169e5e0}, {0x169bc19, 0x169e5e0}};
constexpr char kItem[] = "dlcpacks:/skatev/";

inline std::uintptr_t g_base = 0;
inline bool g_listed = false;
inline std::wstring g_pack;
inline util::LogFn g_log = nullptr;

inline std::uintptr_t Base() { return reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr)); }

inline void __fastcall OnMount(void* mgr, const char* path) {
    if (path && !_strnicmp(path, kItem, sizeof(kItem) - 2)) g_listed = true;
    reinterpret_cast<void(__fastcall*)(void*, const char*)>(g_base + kMount)(mgr, path);
}

// The board's weapon-wheel icon: our hud.gfx (tools/build-board-icon.py) beside the pack.
// Every archive's files reach the inner registrar +0x169E5E0(mgr, &id, name, handle, collection,
// ...) from +0x169B7D8, either from the registration cache (call at +0x169B992) or from the
// archive itself (call at +0x169BC19). For "hud.gfx" we pass our loose file's handle instead,
// so the slot points at our file from its first registration (base game, title update, any
// pack) and the movie is never loaded from the retail copy. Owner runs 2026-10-07 ruled out
// the alternatives: pack overlays lose to the title update's change set, the raw registrar
// refuses a loose file for that slot, and swapping the slot after the HUD loaded froze the game.
// Our handle is made like the raw registrar +0x169E4D4 makes one: device = +0x1364148(path,
// true); collection = device vfunc +0x168; file = (vfunc +0x148 ? device : +0x1357730())
// vfunc +0x1a8(path); handle = collection << 16 | file.
constexpr std::uintptr_t kInner = 0x169e5e0;
using Inner = void*(__fastcall*)(void*, std::uint32_t*, const char*, std::uint32_t, std::uintptr_t, std::uintptr_t,
                                  std::uintptr_t, std::uintptr_t, std::uintptr_t);
inline std::string g_hud;       // ANSI path of our hud.gfx, "" when absent
inline std::uint32_t g_hudHandle = ~0u;
inline bool g_hudTried = false;

inline std::uint32_t LooseHandle(const char* path) {
    __try {
        const auto vt = [](void* o, int off) { return (*static_cast<void***>(o))[off / 8]; };
        void* dev = reinterpret_cast<void*(__fastcall*)(const char*, bool)>(g_base + 0x1364148)(path, true);
        if (!dev) return ~0u;
        const int collection = reinterpret_cast<int(__fastcall*)(void*)>(vt(dev, 0x168))(dev);
        void* src = reinterpret_cast<bool(__fastcall*)(void*)>(vt(dev, 0x148))(dev)
                        ? dev
                        : reinterpret_cast<void*(__fastcall*)()>(g_base + 0x1357730)();
        const int file = reinterpret_cast<int(__fastcall*)(void*, const char*)>(vt(src, 0x1a8))(src, path);
        if (file == -1 || collection < 0) return ~0u;
        return static_cast<std::uint32_t>(collection) << 16 | static_cast<std::uint32_t>(file);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return ~0u;
    }
}

inline void* __fastcall OnImageFile(void* mgr, std::uint32_t* id, const char* name, std::uint32_t handle,
                                    std::uintptr_t collection, std::uintptr_t a6, std::uintptr_t a7,
                                    std::uintptr_t a8, std::uintptr_t a9) {
    if (!g_hud.empty() && name && !_stricmp(name, "hud.gfx")) {
        if (!g_hudTried) {
            g_hudTried = true;
            g_hudHandle = LooseHandle(g_hud.c_str());
            if (g_hudHandle == ~0u) util::Logf(g_log, "pack loader: could not open %s; wheel icon unavailable", g_hud.c_str());
        }
        if (g_hudHandle != ~0u) {
            util::Logf(g_log, "pack loader: hud.gfx from collection %u (handle %08x) points at ours (%08x)",
                       static_cast<unsigned>(collection), handle, g_hudHandle);
            handle = g_hudHandle;
        }
    }
    return reinterpret_cast<Inner>(g_base + kInner)(mgr, id, name, handle, collection, a6, a7, a8, a9);
}

inline void __fastcall OnFinish(void* mgr) {
    if (g_listed) {
        util::Logf(g_log, "pack loader: %s is already in dlclist.xml", kItem);
    } else if (GetFileAttributesW(g_pack.c_str()) != INVALID_FILE_ATTRIBUTES) {
        char item[sizeof(kItem)];
        std::memcpy(item, kItem, sizeof(kItem));
        reinterpret_cast<void(__fastcall*)(void*, const char*)>(g_base + kMount)(mgr, item);
        util::Logf(g_log, "pack loader: mounted %s", kItem);
    } else {
        util::Logf(g_log, "pack loader: %s missing; board weapon, live clip and wheel icon unavailable",
                   util::Narrow(g_pack).c_str());
    }
    reinterpret_cast<void(__fastcall*)(void*)>(g_base + kFinish)(mgr);
}

inline bool Matches(std::uintptr_t base) {
    __try {
        for (const Site& s : kSites) {
            const auto* p = reinterpret_cast<const std::uint8_t*>(base + s.at);
            std::int32_t rel;
            std::memcpy(&rel, p + 1, 4);
            if (p[0] != 0xE8 || s.at + 5 + rel != s.target) return false;
        }
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

// jmp [rip+0]; dq to
inline std::uint8_t* Jump(std::uint8_t* p, std::uintptr_t to) {
    const std::uint8_t op[6] = {0xFF, 0x25, 0, 0, 0, 0};
    std::memcpy(p, op, 6);
    std::memcpy(p + 6, &to, 8);
    return p + 14;
}

inline bool Patch(std::uintptr_t base) {
    auto* mem = static_cast<std::uint8_t*>(util::AllocNear(base, 4096));
    if (!mem) return false;
    std::uint8_t* relay[6];
    std::uint8_t* p = mem;
    relay[0] = p;
    p = Jump(p, reinterpret_cast<std::uintptr_t>(&OnMount));
    relay[1] = p;
    p = Jump(p, reinterpret_cast<std::uintptr_t>(&OnFinish));
    // cmp dword [rbx+0xB4], 'OPEN'; jne +3; mov al, 1; ret; jmp entries
    relay[2] = p;
    const std::uint8_t entries[15] = {0x81, 0xBB, 0xB4, 0, 0, 0, 'O', 'P', 'E', 'N', 0x75, 0x03, 0xB0, 0x01, 0xC3};
    std::memcpy(p, entries, sizeof(entries));
    p = Jump(p + sizeof(entries), base + kEntries);
    // cmp ecx, 'OPEN'; jne +1; ret; jmp names
    relay[3] = p;
    const std::uint8_t names[9] = {0x81, 0xF9, 'O', 'P', 'E', 'N', 0x75, 0x01, 0xC3};
    std::memcpy(p, names, sizeof(names));
    p = Jump(p + sizeof(names), base + kNames);
    relay[4] = relay[5] = p;
    Jump(p, reinterpret_cast<std::uintptr_t>(&OnImageFile));
    FlushInstructionCache(GetCurrentProcess(), mem, 4096);
    for (int i = 0; i < 6; ++i) {
        const std::intptr_t rel = reinterpret_cast<std::intptr_t>(relay[i]) - static_cast<std::intptr_t>(base + kSites[i].at + 5);
        if (rel < INT32_MIN || rel > INT32_MAX) return false;
    }
    for (int i = 0; i < 6; ++i) {
        auto* at = reinterpret_cast<std::uint8_t*>(base + kSites[i].at + 1);
        const auto rel = static_cast<std::int32_t>(reinterpret_cast<std::intptr_t>(relay[i]) -
                                                   static_cast<std::intptr_t>(base + kSites[i].at + 5));
        DWORD old = 0;
        VirtualProtect(at, 4, PAGE_EXECUTE_READWRITE, &old);
        std::memcpy(at, &rel, 4);
        VirtualProtect(at, 4, old, &old);
    }
    FlushInstructionCache(GetCurrentProcess(), reinterpret_cast<void*>(base), 0x1700000);
    return true;
}

inline DWORD WINAPI Retry(LPVOID) {
    for (int ms = 0; ms < 15000; ++ms) {
        if (Matches(g_base)) {
            util::Logf(g_log, Patch(g_base) ? "pack loader: installed after %d ms" : "pack loader: patch failed (%d ms)",
                       ms);
            return 0;
        }
        Sleep(1);
    }
    util::Logf(g_log, "pack loader: GTA code not as expected; pack not loaded");
    return 0;
}

} // namespace detail

// From DllMain. `gameDir` ends with a backslash.
inline void Install(const std::wstring& gameDir, util::LogFn log) {
    using namespace detail;
    g_log = log;
    g_base = Base();
    g_pack = gameDir + L"update\\x64\\dlcpacks\\skatev\\dlc.rpf";
    if (GetFileAttributesW((gameDir + L"update\\x64\\dlcpacks\\skatev\\hud.gfx").c_str()) != INVALID_FILE_ATTRIBUTES)
        g_hud = streaming::NarrowAnsi(gameDir + L"update\\x64\\dlcpacks\\skatev\\hud.gfx");
    if (Matches(g_base)) {
        util::Logf(log, Patch(g_base) ? "pack loader: installed" : "pack loader: patch failed");
    } else if (HANDLE t = CreateThread(nullptr, 0, Retry, nullptr, 0, nullptr)) {
        CloseHandle(t);
    }
}

} // namespace packloader

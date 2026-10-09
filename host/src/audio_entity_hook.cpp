#include "audio_entity_hook.h"

#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <initializer_list>

namespace audiohook {
namespace {

constexpr int kMaxSlots = 64;
constexpr int kBefore = 2; // qwords copied ahead of slot 0 (RTTI locator side)

void** g_entity = nullptr;
void* g_original = nullptr;
void* g_copy[kBefore + kMaxSlots]{};

// push rcx/rdx/r8/r9; sub rsp,0x68; save xmm0-3; edx = slot; call onCall;
// restore; jmp original. Entry rsp is 8 mod 16, so after 4 pushes and 0x68
// the call is 16-aligned with 0x20 bytes of shadow space.
std::size_t WriteThunk(std::uint8_t* p, int slot, void* onCall, void* original) {
    std::uint8_t* const start = p;
    auto put = [&](std::initializer_list<std::uint8_t> b) {
        for (std::uint8_t x : b) *p++ = x;
    };
    auto put64 = [&](const void* v) {
        std::memcpy(p, &v, 8);
        p += 8;
    };
    put({0x51, 0x52, 0x41, 0x50, 0x41, 0x51});        // push rcx, rdx, r8, r9
    put({0x48, 0x83, 0xEC, 0x68});                    // sub rsp, 0x68
    put({0x0F, 0x11, 0x44, 0x24, 0x20});              // movups [rsp+0x20], xmm0
    put({0x0F, 0x11, 0x4C, 0x24, 0x30});              // movups [rsp+0x30], xmm1
    put({0x0F, 0x11, 0x54, 0x24, 0x40});              // movups [rsp+0x40], xmm2
    put({0x0F, 0x11, 0x5C, 0x24, 0x50});              // movups [rsp+0x50], xmm3
    put({0xBA});                                      // mov edx, slot
    std::memcpy(p, &slot, 4);
    p += 4;
    put({0x48, 0xB8});                                // mov rax, onCall
    put64(onCall);
    put({0xFF, 0xD0});                                // call rax
    put({0x0F, 0x10, 0x44, 0x24, 0x20});              // movups xmm0, [rsp+0x20]
    put({0x0F, 0x10, 0x4C, 0x24, 0x30});
    put({0x0F, 0x10, 0x54, 0x24, 0x40});
    put({0x0F, 0x10, 0x5C, 0x24, 0x50});
    put({0x48, 0x83, 0xC4, 0x68});                    // add rsp, 0x68
    put({0x41, 0x59, 0x41, 0x58, 0x5A, 0x59});        // pop r9, r8, rdx, rcx
    put({0x48, 0xB8});                                // mov rax, original
    put64(original);
    put({0xFF, 0xE0});                                // jmp rax
    return static_cast<std::size_t>(p - start);
}

} // namespace

bool Install(void* entity, int first, int last, void (*onCall)(void* self, int slot), void (*log)(const char*)) {
    if (g_entity || !entity || first < 0 || last >= kMaxSlots || last < first) return false;
    auto** object = static_cast<void**>(entity);
    auto** vtable = static_cast<void**>(*object);
    constexpr std::size_t kThunk = 128;
    auto* code = static_cast<std::uint8_t*>(
        VirtualAlloc(nullptr, kThunk * (last - first + 1), MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE));
    if (!code) return false;
    std::memcpy(g_copy, vtable - kBefore, sizeof(void*) * (kBefore + last + 1));
    for (int slot = first; slot <= last; ++slot) {
        std::uint8_t* thunk = code + kThunk * (slot - first);
        WriteThunk(thunk, slot, reinterpret_cast<void*>(onCall), vtable[slot]);
        g_copy[kBefore + slot] = thunk;
    }
    FlushInstructionCache(GetCurrentProcess(), code, kThunk * (last - first + 1));
    g_entity = object;
    g_original = vtable;
    *object = &g_copy[kBefore];
    char line[160];
    std::snprintf(line, sizeof(line), "audio entity hook: %p vtable %p -> copy, slots %d..%d wrapped", entity,
                  static_cast<void*>(vtable), first, last);
    log(line);
    return true;
}

void Uninstall() {
    if (g_entity && *g_entity == &g_copy[kBefore]) *g_entity = g_original;
    g_entity = nullptr;
}

} // namespace audiohook

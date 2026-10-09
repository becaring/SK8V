#pragma once
// Custom streamed models registered from loose files (the native board and the
// posed ped colliders). GTA 1.0.3889.0 image: the raw registration at
// +169E4D4 takes SIX arguments (the sixth is read at +169E581); the type loader
// +91691C is the one DLC_ITYP_REQUEST's original mounter calls (name only).
// No executable patches or copied loaders. Registrations persist for the process.
#include <windows.h>
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <string>

namespace streaming {

using Register = std::uint32_t*(__fastcall*)(std::uint32_t* id, const char* path, bool, const char* name, bool, bool);
using TypeLoad = void(__fastcall*)(const char* ytypName);

inline std::uintptr_t Base() { return reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr)); }

// The registrar and type loader carry the prologues measured on 3889.
inline bool Guards() {
    const std::uintptr_t base = Base();
    __try {
        const auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(base);
        if (dos->e_magic != IMAGE_DOS_SIGNATURE) return false;
        const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS64*>(base + dos->e_lfanew);
        if (nt->Signature != IMAGE_NT_SIGNATURE || nt->OptionalHeader.SizeOfImage < 0x2f5d7c4) return false;
        const unsigned char reg[] = {0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x18, 0x48, 0x89, 0x7c,
                                     0x24, 0x20, 0x41, 0x54, 0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xec, 0x50};
        const unsigned char type[] = {0x40, 0x53, 0x48, 0x81, 0xec, 0x20, 0x01, 0x00, 0x00, 0x48, 0x8b, 0xd9, 0x48,
                                      0x8d, 0x4c, 0x24, 0x21, 0x33, 0xd2, 0x41, 0xb8, 0xff, 0x00, 0x00, 0x00};
        return !std::memcmp(reinterpret_cast<void*>(base + 0x169e4d4), reg, sizeof(reg)) &&
               !std::memcmp(reinterpret_cast<void*>(base + 0x91691c), type, sizeof(type));
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

// The streaming module is initialised (registrations would be lost before).
inline bool Ready() {
    const std::uintptr_t base = Base();
    __try {
        return *reinterpret_cast<const std::uint32_t*>(base + 0x2f5d7c0) != 0 &&
               *reinterpret_cast<const std::uintptr_t*>(base + 0x2f5d7b8) != 0;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

inline Register Registrar() { return reinterpret_cast<Register>(Base() + 0x169e4d4); }
inline TypeLoad TypeLoader() { return reinterpret_cast<TypeLoad>(Base() + 0x91691c); }

// The game's raw file API takes an ANSI path; "" when the conversion is lossy.
inline std::string NarrowAnsi(const std::filesystem::path& path) {
    const auto p = path.wstring();
    BOOL used = FALSE;
    const int n = WideCharToMultiByte(CP_ACP, WC_NO_BEST_FIT_CHARS, p.c_str(), -1, nullptr, 0, nullptr, &used);
    if (n <= 1) return {};
    std::string out(n, '\0');
    if (!WideCharToMultiByte(CP_ACP, WC_NO_BEST_FIT_CHARS, p.c_str(), -1, out.data(), n, nullptr, &used) || used) return {};
    out.pop_back();
    return out;
}

} // namespace streaming

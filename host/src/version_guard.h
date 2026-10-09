#pragma once
#include <windows.h>
#include <cstdint>
#include <vector>

// The ScriptHookV SDK's eGameVersion enum stops at 1.0.617; read the host
// executable's file version instead. SkateV targets GTA V Legacy 1.0.3889.0 only.
struct GameVersion {
    std::uint16_t major, minor, build, revision;
};

inline constexpr GameVersion kRequiredVersion{1, 0, 3889, 0};

inline bool IsSupportedLegacy(const GameVersion& v) {
    return v.major == kRequiredVersion.major && v.minor == kRequiredVersion.minor &&
           v.build == kRequiredVersion.build && v.revision == kRequiredVersion.revision;
}

inline GameVersion ReadHostVersion() {
    GameVersion out{};
    wchar_t path[MAX_PATH]{};
    const DWORD n = GetModuleFileNameW(nullptr, path, MAX_PATH);
    if (n == 0 || n >= MAX_PATH) return out;
    DWORD handle = 0;
    const DWORD size = GetFileVersionInfoSizeW(path, &handle);
    if (size == 0) return out;
    std::vector<unsigned char> data(size);
    if (!GetFileVersionInfoW(path, 0, size, data.data())) return out;
    VS_FIXEDFILEINFO* info = nullptr;
    UINT len = 0;
    if (!VerQueryValueW(data.data(), L"\\", reinterpret_cast<void**>(&info), &len) || !info) return out;
    out.major = HIWORD(info->dwFileVersionMS);
    out.minor = LOWORD(info->dwFileVersionMS);
    out.build = HIWORD(info->dwFileVersionLS);
    out.revision = LOWORD(info->dwFileVersionLS);
    return out;
}

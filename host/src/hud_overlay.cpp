// Skate 3's original HUD over GTA (see hud_overlay.h): ScriptHookV glue.
// Script thread: fetches the runtime's draw list and textures (ABI 8 hud
// block), sets the viewport/safe zone, hides on pause/loading screens.
// Render thread (IDXGISwapChain::Present callback): hands the latest
// publication to hud_renderer. The two meet in one mutex-guarded copy.
#include "stall_sampler.h"
#include "hud_overlay.h"
#include "host_util.h"
#include <windows.h>
#include <d3d11.h>
#include <main.h>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <memory>
#include <mutex>
#include <vector>
#include "hud_renderer.h"
#include "xray_renderer.h"
#include "natives.h"
#include "skatev_runtime.h"

namespace hudoverlay {
namespace {
using gta::Call;

using hudrender::TextureSet;

// Script thread -> render thread, under `mutex`.
struct Published {
    hudrender::Frame frame;
    xrayrender::Frame xray;
    ULONGLONG stamp = 0;
};

struct Shared {
    LogFn log = nullptr;
    void* runtime = nullptr;
    SvSetHudViewportFn setViewport = nullptr;
    SvGetHudTextureFn getTexture = nullptr;
    SvGetHudFrameFn getFrame = nullptr;
    SvGetXrayFn getXray = nullptr; // optional (HallOfMeatXray=0: unused)
    // Optional: Skate's TRAX banner for GTA's radio (runtime hud.rs).
    std::uint32_t(__cdecl* showRadio)(void*, const char*, const char*, const char*, const char*) = nullptr;
    bool enabled = false;
    bool registered = false;
    float maxAspect = 0.0f;
    float safeOverride = -1.0f;
    // Script thread only.
    SvHudViewport sent{};
    std::vector<SvHudDraw> draws;
    std::vector<SvHudVertex> vertices;
    std::uint32_t fetchedGeneration = 0;
    std::vector<SvXrayVertex> xray;
    bool xrayShown = false;
    std::shared_ptr<const TextureSet> textures;
    bool wasShown = false;
    // Both threads.
    std::mutex mutex;
    Published pub;
    std::atomic<UINT> backbufferWidth{0}, backbufferHeight{0};
    std::atomic<bool> stopping{false};
} g;

template <class... A>
void Logf(const char* fmt, A... a) {
    util::Logf(g.log, fmt, a...);
}

void OnPresent(void* swapChainPtr) {
    stall::RegisterRender();
    if (g.stopping.load() || !swapChainPtr) return;
    auto* sc = static_cast<IDXGISwapChain*>(swapChainPtr);
    DXGI_SWAP_CHAIN_DESC scd{};
    if (FAILED(sc->GetDesc(&scd))) return;
    g.backbufferWidth.store(scd.BufferDesc.Width);
    g.backbufferHeight.store(scd.BufferDesc.Height);
    static hudrender::Frame frame; // render thread's copy of the publication
    static xrayrender::Frame xray;
    {
        std::lock_guard<std::mutex> lock(g.mutex);
        Published& p = g.pub;
        // Stale publication (script thread stalled: loading, pause): hide.
        const bool fresh = GetTickCount64() - p.stamp < 500;
        frame.visible = p.frame.visible && fresh;
        if (frame.visible && p.frame.serial != frame.serial) {
            frame.serial = p.frame.serial;
            frame.width = p.frame.width;
            frame.height = p.frame.height;
            frame.draws = p.frame.draws;
            frame.vertices = p.frame.vertices;
        }
        if (p.frame.textures && p.frame.textureGeneration != frame.textureGeneration) {
            frame.textures = p.frame.textures;
            frame.textureGeneration = p.frame.textureGeneration;
        }
        xray.visible = p.xray.visible && fresh;
        if (xray.visible) {
            xray.vertices = p.xray.vertices;
            xray.camera = p.xray.camera;
        }
    }
    xrayrender::Present(sc, xray); // under the HUD
    hudrender::Present(sc, frame);
}

// ---- script thread ---------------------------------------------------------
float IniFloat(const std::wstring& ini, const wchar_t* key, float def) {
    const std::string v = util::IniString(ini, key);
    return v.empty() ? def : static_cast<float>(std::atof(v.c_str()));
}

void FetchTextures(std::uint32_t count, std::uint32_t generation) {
    auto set = std::make_shared<TextureSet>();
    for (std::uint32_t i = 0; i < count; ++i) {
        SvHudTexture info{};
        info.size = sizeof(info);
        if (!g.getTexture(g.runtime, i, &info, nullptr, 0) || info.width == 0 || info.height == 0 ||
            info.width > 8192 || info.height > 8192) {
            Logf("SkateV HUD: texture %u unavailable", i);
            return;
        }
        hudrender::Texture t;
        t.width = info.width;
        t.height = info.height;
        t.rgba.resize(static_cast<std::size_t>(info.width) * info.height * 4);
        if (!g.getTexture(g.runtime, i, &info, t.rgba.data(), static_cast<std::uint32_t>(t.rgba.size()))) return;
        set->push_back(std::move(t));
    }
    g.textures = set;
    g.fetchedGeneration = generation;
    std::lock_guard<std::mutex> lock(g.mutex);
    g.pub.frame.textures = set;
    g.pub.frame.textureGeneration = generation;
    Logf("SkateV HUD: %u textures fetched (generation %u)", count, generation);
}
} // namespace

void Start(const std::wstring& iniPath, void* runtime, LogFn log) {
    g.log = log;
    g.enabled = false;
    if (IniFloat(iniPath, L"Hud", 1.0f) == 0.0f) {
        Logf("SkateV HUD: off (Hud=0)");
        return;
    }
    const HMODULE module = GetModuleHandleW(L"SkateVRuntime.dll");
    if (!runtime || !module) {
        Logf("SkateV HUD: runtime not loaded; original HUD off");
        return;
    }
    g.setViewport = reinterpret_cast<SvSetHudViewportFn>(GetProcAddress(module, "sv_set_hud_viewport"));
    g.getTexture = reinterpret_cast<SvGetHudTextureFn>(GetProcAddress(module, "sv_get_hud_texture"));
    g.getFrame = reinterpret_cast<SvGetHudFrameFn>(GetProcAddress(module, "sv_get_hud_frame"));
    if (!g.setViewport || !g.getTexture || !g.getFrame) {
        Logf("SkateV HUD: runtime has no HUD exports; original HUD off");
        return;
    }
    g.runtime = runtime;
    g.showRadio = reinterpret_cast<decltype(g.showRadio)>(GetProcAddress(module, "sv_show_radio"));
    g.getXray = IniFloat(iniPath, L"HallOfMeatXray", 1.0f) != 0.0f
                    ? reinterpret_cast<SvGetXrayFn>(GetProcAddress(module, "sv_get_xray"))
                    : nullptr;
    g.maxAspect = IniFloat(iniPath, L"HudMaxAspect", 0.0f);
    g.safeOverride = IniFloat(iniPath, L"HudSafeZone", -1.0f);
    g.stopping.store(false);
    if (!g.registered) {
        hudrender::SetLog(log);
        xrayrender::SetLog(log);
        hudrender::SetCompositeOverride(static_cast<int>(IniFloat(iniPath, L"HudComposite", -1.0f)));
        presentCallbackRegister(OnPresent);
        g.registered = true;
    }
    g.enabled = true;
    Logf("SkateV HUD: original trickdisplay on (HudMaxAspect %.3f, HudSafeZone %.2f), Hall of Meat x-ray %s",
         g.maxAspect, g.safeOverride, g.getXray ? "on" : "off");
}

bool ShowRadio(const char* top, const char* middle, const char* bottom, const char* station) {
    return g.enabled && g.runtime && g.showRadio && g.showRadio(g.runtime, top, middle, bottom, station) == 1;
}

void Tick(bool skating) {
    if (!g.enabled) return;
    const UINT w = g.backbufferWidth.load(), h = g.backbufferHeight.load();
    float safe = g.safeOverride >= 0.5f ? std::fmin(g.safeOverride, 1.0f) : Call<float>(gta::GET_SAFE_ZONE_SIZE);
    if (!(safe >= 0.5f && safe <= 1.0f)) safe = 1.0f;
    if (w && h) {
        SvHudViewport v{};
        v.size = sizeof(v);
        v.width = w;
        v.height = h;
        v.safe_zone = safe;
        v.max_aspect = g.maxAspect;
        v.flags = 1;
        if (g.safeOverride < 0.5f) {
            // GTA's own HUD inset (safe zone plus its ultrawide placement, where the minimap is): a
            // top-left aligned origin, mirrored (the area is centred). Never less than the safe zone.
            float x = 0.0f, y = 0.0f;
            Call<void>(gta::SET_SCRIPT_GFX_ALIGN, 'L', 'T');
            Call<void>(gta::SET_SCRIPT_GFX_ALIGN_PARAMS, 0.0f, 0.0f, 0.0f, 0.0f);
            Call<void>(gta::GET_SCRIPT_GFX_ALIGN_POSITION, 0.0f, 0.0f, &x, &y);
            Call<void>(gta::RESET_SCRIPT_GFX_ALIGN);
            const float model = (1.0f - safe) * 0.5f - 0.005f;
            if (x >= model && y >= model && x < 0.4f && y < 0.4f) {
                const float area[4] = {x, y, 1.0f - x, 1.0f - y};
                std::memcpy(v.hud_area, area, sizeof(area));
            }
        }
        if (std::memcmp(&v, &g.sent, sizeof(v)) != 0 && g.setViewport(g.runtime, &v)) {
            g.sent = v;
            Logf("SkateV HUD: viewport %ux%u safe zone %.2f, GTA HUD area %.4f,%.4f-%.4f,%.4f", w, h, safe,
                 v.hud_area[0], v.hud_area[1], v.hud_area[2], v.hud_area[3]);
        }
    }
    const bool paused = gta::FrontendActive();
    bool show = skating && !paused && w && h;
    SvHudFrame f{};
    f.size = sizeof(f);
    if (show) {
        bool ok = false;
        for (int attempt = 0; attempt < 3 && !ok; ++attempt) {
            ok = g.getFrame(g.runtime, &f, g.draws.data(), static_cast<std::uint32_t>(g.draws.size()),
                            g.vertices.data(), static_cast<std::uint32_t>(g.vertices.size())) != 0;
            if (!ok) {
                if (f.draw_count > g.draws.size()) g.draws.resize(f.draw_count + 32);
                if (f.vertex_count > g.vertices.size()) g.vertices.resize(f.vertex_count + 1024);
            }
        }
        show = ok && f.visible && f.draw_count > 0;
        if (show && f.texture_generation != g.fetchedGeneration) FetchTextures(f.texture_count, f.texture_generation);
        show = show && g.fetchedGeneration == f.texture_generation;
    }
    if (show != g.wasShown) {
        Logf("SkateV HUD: %s%s", show ? "shown" : "hidden", paused ? " (GTA menu)" : "");
        g.wasShown = show;
    }
    // Hall of Meat x-ray with the camera GTA rendered (same script frame).
    std::uint32_t xrayCount = 0;
    xrayrender::Camera camera{};
    if (g.getXray && skating && !paused && w && h) {
        SvXrayFrame xf{};
        xf.size = sizeof(xf);
        xrayCount = g.getXray(g.runtime, &xf, g.xray.data(), static_cast<std::uint32_t>(g.xray.size()));
        if (xrayCount == 0 && xf.vertex_count > g.xray.size()) {
            g.xray.resize(xf.vertex_count + 3072);
            xrayCount = g.getXray(g.runtime, &xf, g.xray.data(), static_cast<std::uint32_t>(g.xray.size()));
        }
        if (xrayCount) {
            const Vector3 c = Call<Vector3>(gta::GET_FINAL_RENDERED_CAM_COORD);
            const Vector3 r = Call<Vector3>(gta::GET_FINAL_RENDERED_CAM_ROT, 2);
            const float fov = Call<float>(gta::GET_FINAL_RENDERED_CAM_FOV);
            camera = {{c.x, c.y, c.z}, {r.x, r.y, r.z}, fov};
            if (!(fov > 1.0f && fov < 170.0f) || !std::isfinite(c.x + c.y + c.z + r.x + r.z)) xrayCount = 0;
        }
    }
    if ((xrayCount > 0) != g.xrayShown) {
        g.xrayShown = xrayCount > 0;
        if (g.xrayShown) {
            Logf("SkateV HUD: Hall of Meat x-ray shown (%u vertices, camera fov %.1f roll %.1f)", xrayCount, camera.fov,
                 camera.rot[1]);
        } else {
            Logf("SkateV HUD: Hall of Meat x-ray hidden");
        }
    }
    std::lock_guard<std::mutex> lock(g.mutex);
    Published& p = g.pub;
    p.stamp = GetTickCount64();
    p.xray.visible = xrayCount > 0;
    if (xrayCount) {
        p.xray.camera = camera;
        p.xray.vertices.resize(xrayCount);
        std::memcpy(p.xray.vertices.data(), g.xray.data(), xrayCount * sizeof(SvXrayVertex));
    }
    p.frame.visible = show;
    if (!show || f.serial == p.frame.serial) return;
    hudrender::Frame& out = p.frame;
    out.serial = f.serial;
    out.width = f.width;
    out.height = f.height;
    out.draws.clear();
    out.vertices.clear();
    for (std::uint32_t i = 0; i < f.draw_count; ++i) {
        const SvHudDraw& d = g.draws[i];
        if (d.vertex_count == 0 || d.first_vertex + d.vertex_count > f.vertex_count) continue;
        out.draws.push_back({d.texture, static_cast<std::uint32_t>(out.vertices.size()), d.vertex_count});
        for (std::uint32_t k = 0; k < d.vertex_count; ++k) {
            const SvHudVertex& v = g.vertices[d.first_vertex + k];
            out.vertices.push_back({v.x, v.y, v.u, v.v, {d.multiply[0], d.multiply[1], d.multiply[2], d.multiply[3]},
                                    {d.add[0], d.add[1], d.add[2], d.add[3]}});
        }
    }
}

void Shutdown(bool processExit) {
    g.stopping.store(true);
    if (g.registered) {
        presentCallbackUnregister(OnPresent);
        g.registered = false;
    }
    g.enabled = false;
    if (!processExit) {
        hudrender::Release();
        xrayrender::Release();
    }
}
} // namespace hudoverlay

#pragma once
// D3D11 rasteriser for the original Skate 3 HUD draw list (render thread).
// Independent of ScriptHookV so it can be exercised headless on WARP
// (tests/hud_d3d). See hud_renderer.cpp for the pipeline.
#include <windows.h>
#include <d3d11.h>
#include <dxgi.h>
#include <cstdint>
#include <memory>
#include <vector>

namespace hudrender {
using LogFn = void (*)(const char*);

// Backbuffer pixels (origin top-left), uv, and the draw's APT colour
// transform carried per vertex.
struct Vertex {
    float x, y, u, v;
    float mul[4];
    float add[4];
};
static_assert(sizeof(Vertex) == 48);

struct Draw {
    std::uint32_t texture, first, count;
};

// RGBA8, straight alpha, sRGB colour.
struct Texture {
    UINT width = 0, height = 0;
    std::vector<std::uint8_t> rgba;
};
using TextureSet = std::vector<Texture>;

struct Frame {
    bool visible = false;
    std::uint64_t serial = 0; // a new serial re-renders the offscreen pass
    UINT width = 0, height = 0; // size the vertices were laid out for
    std::vector<Draw> draws;
    std::vector<Vertex> vertices;
    std::shared_ptr<const TextureSet> textures;
    std::uint32_t textureGeneration = 0;
};

void SetLog(LogFn log);
// -1 (default): best available composite; 1 force the frame-copy path;
// 2 force the display-gamma blend (ini HudComposite, tests).
void SetCompositeOverride(int mode);
// Draws `frame` over the swapchain's current backbuffer, restoring all
// pipeline state it touched. Returns the composite mode (0 sRGB view,
// 1 frame copy, 2 gamma blend) or -1 when nothing was drawn.
int Present(IDXGISwapChain* swapChain, const Frame& frame);
// Releases every GPU object (device change, shutdown).
void Release();
}

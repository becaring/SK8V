#pragma once
// D3D11 pass for Skate 3's Hall of Meat x-ray (render thread): the runtime's
// broken-bone triangles (GTA world space, colours as the `defaulthom` shader
// writes them) projected with GTA's rendered camera and drawn over the frame
// with their own depth buffer, so the bones show through the body. Drawn
// before the HUD composite. Independent of ScriptHookV (tests/hud_d3d).
#include <windows.h>
#include <d3d11.h>
#include <dxgi.h>
#include <cstdint>
#include <vector>

namespace xrayrender {
using LogFn = void (*)(const char*);

// Same layout as SvXrayVertex.
struct Vertex {
    float x, y, z;
    float rgba[4];
};
static_assert(sizeof(Vertex) == 28);

// GTA camera: position, rotation (degrees, rotation order 2: pitch, roll,
// yaw) and vertical field of view in degrees.
struct Camera {
    float pos[3];
    float rot[3];
    float fov;
};

struct Frame {
    bool visible = false;
    std::vector<Vertex> vertices; // triangle list
    Camera camera{};
};

// Row-major world -> clip matrix for row vectors (clip = [x y z 1] * m):
// forward (-sin yaw cos pitch, cos yaw cos pitch, sin pitch), right
// (cos yaw, sin yaw, 0), roll ignored, left-handed depth 0..1.
void ViewProjection(const Camera& camera, float aspect, float nearZ, float farZ, float m[16]);

void SetLog(LogFn log);
// Draws `frame` over the swapchain's current backbuffer, restoring all
// pipeline state it touched. Returns true when something was drawn.
bool Present(IDXGISwapChain* swapChain, const Frame& frame);
// Releases every GPU object (device change, shutdown).
void Release();
}

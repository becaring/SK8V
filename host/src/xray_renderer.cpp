// D3D11 pass for Skate 3's Hall of Meat x-ray (see xray_renderer.h).
//
// The vertices already carry the retail shader's colour (computed by the
// runtime, rust/skatev-runtime/src/xray.rs), display encoded: they are
// written through a UNORM view of the backbuffer (an sRGB-only backbuffer
// gets the colour decoded first), straight-alpha blended, alpha channel
// untouched. A private depth buffer, cleared every frame, sorts the bones
// against each other only: the x-ray is drawn over GTA's scene. GTA's
// pipeline state is saved before and restored after; no reference to the
// backbuffer is held across presents.
#include "xray_renderer.h"
#include "d3d11_saved_state.h"
#include "host_util.h"
#include <d3dcompiler.h>
#include <cmath>
#include <cstdio>
#include <cstring>

namespace xrayrender {
namespace {
using d3dstate::SafeRelease;
using d3dstate::SavedState;

LogFn g_log = nullptr;

template <class... A>
void Logf(const char* fmt, A... a) {
    util::Logf(g_log, fmt, a...);
}

constexpr char kShaders[] = R"(
cbuffer Frame : register(b0) { row_major float4x4 vp; uint srgbTarget; float3 pad; };
struct VSIn { float3 pos : POSITION; float4 col : COLOR0; };
struct VSOut { float4 pos : SV_Position; float4 col : COLOR0; };
VSOut VsXray(VSIn i) {
    VSOut o;
    o.pos = mul(float4(i.pos, 1.0), vp);
    o.col = i.col;
    return o;
}
float3 Decode(float3 c) {
    c = saturate(c);
    return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}
float4 PsXray(VSOut i) : SV_Target {
    float4 c = saturate(i.col);
    if (srgbTarget != 0) c.rgb = Decode(c.rgb);
    return c;
}
)";

struct Constants {
    float vp[16];
    UINT srgbTarget;
    float pad[3];
};
static_assert(sizeof(Constants) == 80);

// Clip planes for a bail seen from a chase camera, metres.
constexpr float kNear = 0.05f;
constexpr float kFar = 500.0f;

struct Gpu {
    ID3D11Device* device = nullptr;
    ID3D11DeviceContext* ctx = nullptr;
    ID3D11VertexShader* vs = nullptr;
    ID3D11PixelShader* ps = nullptr;
    ID3D11InputLayout* layout = nullptr;
    ID3D11Buffer* vb = nullptr;
    UINT vbCapacity = 0;
    ID3D11Buffer* cb = nullptr;
    ID3D11BlendState* blend = nullptr;
    ID3D11RasterizerState* raster = nullptr;
    ID3D11DepthStencilState* depth = nullptr;
    ID3D11Texture2D* depthTex = nullptr;
    ID3D11DepthStencilView* dsv = nullptr;
    UINT depthWidth = 0, depthHeight = 0, depthSamples = 0;
    DXGI_FORMAT loggedFormat = DXGI_FORMAT_UNKNOWN;
    bool failed = false;
} gpu;

void ReleaseAll() {
    SafeRelease(gpu.dsv);
    SafeRelease(gpu.depthTex);
    gpu.depthWidth = gpu.depthHeight = gpu.depthSamples = 0;
    SafeRelease(gpu.vs);
    SafeRelease(gpu.ps);
    SafeRelease(gpu.layout);
    SafeRelease(gpu.vb);
    gpu.vbCapacity = 0;
    SafeRelease(gpu.cb);
    SafeRelease(gpu.blend);
    SafeRelease(gpu.raster);
    SafeRelease(gpu.depth);
    SafeRelease(gpu.ctx);
    SafeRelease(gpu.device);
    gpu.loggedFormat = DXGI_FORMAT_UNKNOWN;
}

ID3DBlob* Compile(const char* entry, const char* profile) {
    return d3dstate::CompileShader(kShaders, sizeof(kShaders) - 1, "skatev_xray.hlsl", entry, profile, g_log, "SkateV x-ray");
}

bool CreateDeviceObjects(ID3D11Device* device) {
    gpu.device = device;
    gpu.device->AddRef();
    gpu.device->GetImmediateContext(&gpu.ctx);
    ID3DBlob* vs = Compile("VsXray", "vs_5_0");
    ID3DBlob* ps = Compile("PsXray", "ps_5_0");
    bool ok = vs && ps &&
              SUCCEEDED(device->CreateVertexShader(vs->GetBufferPointer(), vs->GetBufferSize(), nullptr, &gpu.vs)) &&
              SUCCEEDED(device->CreatePixelShader(ps->GetBufferPointer(), ps->GetBufferSize(), nullptr, &gpu.ps));
    if (ok) {
        const D3D11_INPUT_ELEMENT_DESC elements[] = {
            {"POSITION", 0, DXGI_FORMAT_R32G32B32_FLOAT, 0, 0, D3D11_INPUT_PER_VERTEX_DATA, 0},
            {"COLOR", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 12, D3D11_INPUT_PER_VERTEX_DATA, 0},
        };
        ok = SUCCEEDED(device->CreateInputLayout(elements, 2, vs->GetBufferPointer(), vs->GetBufferSize(), &gpu.layout));
    }
    SafeRelease(vs);
    SafeRelease(ps);
    if (ok) {
        D3D11_BUFFER_DESC d{};
        d.ByteWidth = sizeof(Constants);
        d.Usage = D3D11_USAGE_DYNAMIC;
        d.BindFlags = D3D11_BIND_CONSTANT_BUFFER;
        d.CPUAccessFlags = D3D11_CPU_ACCESS_WRITE;
        ok = SUCCEEDED(device->CreateBuffer(&d, nullptr, &gpu.cb));
    }
    if (ok) {
        D3D11_BLEND_DESC b{};
        auto& rt = b.RenderTarget[0];
        rt.BlendEnable = TRUE;
        rt.SrcBlend = D3D11_BLEND_SRC_ALPHA;
        rt.DestBlend = D3D11_BLEND_INV_SRC_ALPHA;
        rt.BlendOp = D3D11_BLEND_OP_ADD;
        rt.SrcBlendAlpha = D3D11_BLEND_ZERO;
        rt.DestBlendAlpha = D3D11_BLEND_ONE;
        rt.BlendOpAlpha = D3D11_BLEND_OP_ADD;
        rt.RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_RED | D3D11_COLOR_WRITE_ENABLE_GREEN |
                                   D3D11_COLOR_WRITE_ENABLE_BLUE;
        ok = SUCCEEDED(device->CreateBlendState(&b, &gpu.blend));
    }
    if (ok) {
        D3D11_RASTERIZER_DESC r{};
        r.FillMode = D3D11_FILL_SOLID;
        r.CullMode = D3D11_CULL_NONE;
        r.DepthClipEnable = TRUE;
        ok = SUCCEEDED(device->CreateRasterizerState(&r, &gpu.raster));
    }
    if (ok) {
        D3D11_DEPTH_STENCIL_DESC d{};
        d.DepthEnable = TRUE;
        d.DepthWriteMask = D3D11_DEPTH_WRITE_MASK_ALL;
        d.DepthFunc = D3D11_COMPARISON_LESS;
        ok = SUCCEEDED(device->CreateDepthStencilState(&d, &gpu.depth));
    }
    if (!ok) {
        Logf("SkateV x-ray: D3D11 setup failed; x-ray off");
        ReleaseAll();
        gpu.failed = true;
    }
    return ok;
}

bool EnsureDepth(UINT w, UINT h, UINT samples) {
    if (gpu.dsv && gpu.depthWidth == w && gpu.depthHeight == h && gpu.depthSamples == samples) return true;
    SafeRelease(gpu.dsv);
    SafeRelease(gpu.depthTex);
    D3D11_TEXTURE2D_DESC d{};
    d.Width = w;
    d.Height = h;
    d.MipLevels = 1;
    d.ArraySize = 1;
    d.Format = DXGI_FORMAT_D24_UNORM_S8_UINT;
    d.SampleDesc.Count = samples;
    d.Usage = D3D11_USAGE_DEFAULT;
    d.BindFlags = D3D11_BIND_DEPTH_STENCIL;
    if (FAILED(gpu.device->CreateTexture2D(&d, nullptr, &gpu.depthTex))) return false;
    D3D11_DEPTH_STENCIL_VIEW_DESC v{};
    v.Format = d.Format;
    v.ViewDimension = samples > 1 ? D3D11_DSV_DIMENSION_TEXTURE2DMS : D3D11_DSV_DIMENSION_TEXTURE2D;
    if (FAILED(gpu.device->CreateDepthStencilView(gpu.depthTex, &v, &gpu.dsv))) return false;
    gpu.depthWidth = w;
    gpu.depthHeight = h;
    gpu.depthSamples = samples;
    return true;
}

bool EnsureVertexBuffer(UINT count) {
    return d3dstate::EnsureVertexBuffer(gpu.device, gpu.vb, gpu.vbCapacity, count, 16384, sizeof(Vertex));
}

// A writable view for the backbuffer format: typeless 8-bit formats get a
// UNORM view; `srgb` reports an sRGB view (colours must be decoded).
bool ViewFormat(DXGI_FORMAT f, DXGI_FORMAT& view, bool& srgb) {
    srgb = false;
    switch (f) {
    case DXGI_FORMAT_R8G8B8A8_TYPELESS:
        view = DXGI_FORMAT_R8G8B8A8_UNORM;
        return true;
    case DXGI_FORMAT_B8G8R8A8_TYPELESS:
        view = DXGI_FORMAT_B8G8R8A8_UNORM;
        return true;
    case DXGI_FORMAT_R8G8B8A8_UNORM_SRGB:
    case DXGI_FORMAT_B8G8R8A8_UNORM_SRGB:
        view = f;
        srgb = true;
        return true;
    default:
        view = f;
        return true;
    }
}
} // namespace

void ViewProjection(const Camera& camera, float aspect, float nearZ, float farZ, float m[16]) {
    const float deg = 3.14159265358979f / 180.0f;
    const float pitch = camera.rot[0] * deg, yaw = camera.rot[2] * deg;
    const float f[3] = {-std::sin(yaw) * std::cos(pitch), std::cos(yaw) * std::cos(pitch), std::sin(pitch)};
    const float r[3] = {std::cos(yaw), std::sin(yaw), 0.0f};
    const float u[3] = {r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]};
    const float* e = camera.pos;
    const float ky = 1.0f / std::tan(camera.fov * deg * 0.5f);
    const float kx = ky / (aspect > 0.0f ? aspect : 1.0f);
    const float a = farZ / (farZ - nearZ), b = -nearZ * farZ / (farZ - nearZ);
    // View rows (row vectors): x' = d.r, y' = d.u, z' = d.f with d = p - e.
    const float vx[4] = {r[0], r[1], r[2], -(r[0] * e[0] + r[1] * e[1] + r[2] * e[2])};
    const float vy[4] = {u[0], u[1], u[2], -(u[0] * e[0] + u[1] * e[1] + u[2] * e[2])};
    const float vz[4] = {f[0], f[1], f[2], -(f[0] * e[0] + f[1] * e[1] + f[2] * e[2])};
    for (int i = 0; i < 4; ++i) {
        m[i * 4 + 0] = vx[i] * kx;
        m[i * 4 + 1] = vy[i] * ky;
        m[i * 4 + 2] = vz[i] * a + (i == 3 ? b : 0.0f);
        m[i * 4 + 3] = vz[i];
    }
}

void SetLog(LogFn log) { g_log = log; }

void Release() {
    ReleaseAll();
    gpu.failed = false;
}

bool Present(IDXGISwapChain* sc, const Frame& frame) {
    if (gpu.failed || !sc || !frame.visible || frame.vertices.size() < 3) return false;
    ID3D11Device* device = nullptr;
    if (FAILED(sc->GetDevice(__uuidof(ID3D11Device), reinterpret_cast<void**>(&device)))) return false;
    if (device != gpu.device) {
        ReleaseAll();
        if (!CreateDeviceObjects(device)) {
            device->Release();
            return false;
        }
    }
    device->Release(); // gpu.device holds its own reference
    if (gpu.device->GetDeviceRemovedReason() != S_OK) {
        ReleaseAll();
        return false;
    }
    const UINT count = static_cast<UINT>(frame.vertices.size() / 3 * 3);
    if (!EnsureVertexBuffer(count)) return false;

    ID3D11Texture2D* backbuffer = nullptr;
    if (FAILED(sc->GetBuffer(0, __uuidof(ID3D11Texture2D), reinterpret_cast<void**>(&backbuffer)))) return false;
    D3D11_TEXTURE2D_DESC bd{};
    backbuffer->GetDesc(&bd);
    DXGI_FORMAT viewFormat{};
    bool srgb = false;
    ViewFormat(bd.Format, viewFormat, srgb);
    const bool ms = bd.SampleDesc.Count > 1;
    D3D11_RENDER_TARGET_VIEW_DESC rv{};
    rv.Format = viewFormat;
    rv.ViewDimension = ms ? D3D11_RTV_DIMENSION_TEXTURE2DMS : D3D11_RTV_DIMENSION_TEXTURE2D;
    ID3D11RenderTargetView* rtv = nullptr;
    if (FAILED(gpu.device->CreateRenderTargetView(backbuffer, &rv, &rtv)) ||
        !EnsureDepth(bd.Width, bd.Height, bd.SampleDesc.Count)) {
        SafeRelease(rtv);
        backbuffer->Release();
        return false;
    }
    if (bd.Format != gpu.loggedFormat) {
        Logf("SkateV x-ray: backbuffer %ux%u format %d samples %u (%s view)", bd.Width, bd.Height,
             static_cast<int>(bd.Format), bd.SampleDesc.Count, srgb ? "sRGB" : "UNORM");
        gpu.loggedFormat = bd.Format;
    }
    ID3D11DeviceContext* c = gpu.ctx;
    SavedState saved;
    saved.Save(c);
    D3D11_MAPPED_SUBRESOURCE m{};
    bool drawn = false;
    if (SUCCEEDED(c->Map(gpu.vb, 0, D3D11_MAP_WRITE_DISCARD, 0, &m))) {
        std::memcpy(m.pData, frame.vertices.data(), count * sizeof(Vertex));
        c->Unmap(gpu.vb, 0);
        Constants k{};
        ViewProjection(frame.camera, static_cast<float>(bd.Width) / static_cast<float>(bd.Height), kNear, kFar, k.vp);
        k.srgbTarget = srgb ? 1u : 0u;
        if (SUCCEEDED(c->Map(gpu.cb, 0, D3D11_MAP_WRITE_DISCARD, 0, &m))) {
            std::memcpy(m.pData, &k, sizeof(k));
            c->Unmap(gpu.cb, 0);
            c->ClearDepthStencilView(gpu.dsv, D3D11_CLEAR_DEPTH, 1.0f, 0);
            c->OMSetRenderTargets(1, &rtv, gpu.dsv);
            D3D11_VIEWPORT vp{0.0f, 0.0f, static_cast<float>(bd.Width), static_cast<float>(bd.Height), 0.0f, 1.0f};
            c->RSSetViewports(1, &vp);
            c->RSSetState(gpu.raster);
            c->OMSetDepthStencilState(gpu.depth, 0);
            const float factor[4] = {0, 0, 0, 0};
            c->OMSetBlendState(gpu.blend, factor, 0xFFFFFFFFu);
            const UINT stride = sizeof(Vertex), offset = 0;
            c->IASetInputLayout(gpu.layout);
            c->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c->IASetVertexBuffers(0, 1, &gpu.vb, &stride, &offset);
            c->VSSetShader(gpu.vs, nullptr, 0);
            c->VSSetConstantBuffers(0, 1, &gpu.cb);
            c->PSSetShader(gpu.ps, nullptr, 0);
            c->PSSetConstantBuffers(0, 1, &gpu.cb);
            c->GSSetShader(nullptr, nullptr, 0);
            c->HSSetShader(nullptr, nullptr, 0);
            c->DSSetShader(nullptr, nullptr, 0);
            c->Draw(count, 0);
            drawn = true;
        }
    }
    saved.Restore(c);
    rtv->Release();
    backbuffer->Release();
    return drawn;
}
} // namespace xrayrender

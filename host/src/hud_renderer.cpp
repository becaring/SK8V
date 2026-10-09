// D3D11 rasteriser for Skate 3's original HUD draw list (see hud_renderer.h).
//
// Pipeline (same maths as the donor's scoring_hud + hud_render.wgsl +
// hud_composite.wgsl, and rust/skate-hud/src/raster.rs):
//  1. offscreen R8G8B8A8 sRGB target (backbuffer size), cleared transparent;
//  2. each draw samples an sRGB texture (bilinear, clamp) and outputs
//     saturate(texel * multiply + add), blended straight-alpha in linear
//     (colour SrcAlpha/InvSrcAlpha, alpha One/InvSrcAlpha);
//  3. that premultiplied target is composited over the frame in linear:
//     mode 0: an sRGB view of the backbuffer + One/InvSrcAlpha blending;
//     mode 1: (no sRGB view, 8-bit, single-sample) the backbuffer is copied
//             and the shader blends in linear and re-encodes, exactly;
//     mode 2: (other formats) premultiplied blend in display gamma.
// The offscreen pass reruns only when a new frame serial arrives. GTA's
// pipeline state is saved before and restored after; no reference to the
// backbuffer is held across presents (ResizeBuffers stays legal).
#include "hud_renderer.h"
#include "d3d11_saved_state.h"
#include "host_util.h"
#include <d3dcompiler.h>
#include <cstdio>
#include <cstring>

namespace hudrender {
namespace {
using d3dstate::SafeRelease;
using d3dstate::SavedState;

LogFn g_log = nullptr;
int g_compositeOverride = -1; // -1 auto, 1 frame copy, 2 gamma blend

template <class... A>
void Logf(const char* fmt, A... a) {
    util::Logf(g_log, fmt, a...);
}
constexpr char kShaders[] = R"(
cbuffer Frame : register(b0) { float2 invSize; uint mode; float pad; };
struct VSIn { float2 pos : POSITION; float2 uv : TEXCOORD0; float4 mul : COLOR0; float4 add : COLOR1; };
struct VSOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 mul : COLOR0; float4 add : COLOR1; };
Texture2D tex0 : register(t0);
Texture2D tex1 : register(t1);
SamplerState samp : register(s0);
VSOut VsHud(VSIn i) {
    VSOut o;
    o.pos = float4(i.pos.x * invSize.x * 2.0 - 1.0, 1.0 - i.pos.y * invSize.y * 2.0, 0.0, 1.0);
    o.uv = i.uv; o.mul = i.mul; o.add = i.add;
    return o;
}
// hud_render.wgsl
float4 PsHud(VSOut i) : SV_Target { return saturate(tex0.Sample(samp, i.uv) * i.mul + i.add); }
struct FullOut { float4 pos : SV_Position; };
FullOut VsFull(uint id : SV_VertexID) {
    FullOut o;
    float2 uv = float2((id << 1) & 2, id & 2);
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}
float3 Encode(float3 c) {
    c = saturate(c);
    float3 lo = c * 12.92;
    float3 hi = 1.055 * pow(max(c, 1e-6), 1.0 / 2.4) - 0.055;
    return c <= 0.0031308 ? lo : hi;
}
// hud_composite.wgsl (premultiplied); mode 2 re-encodes for a gamma target.
float4 PsComposite(FullOut i) : SV_Target {
    float4 h = tex0.Load(int3(i.pos.xy, 0));
    if (mode == 2) {
        float3 c = h.a > 0.0 ? Encode(h.rgb / h.a) * h.a : float3(0.0, 0.0, 0.0);
        return float4(c, h.a);
    }
    return h;
}
// Mode 1: linear blend over a copy of the frame, written encoded.
float4 PsCompositeCopy(FullOut i) : SV_Target {
    float4 h = tex0.Load(int3(i.pos.xy, 0));
    float3 bg = tex1.Load(int3(i.pos.xy, 0)).rgb;
    return float4(Encode(h.rgb + bg * (1.0 - h.a)), 1.0);
}
)";

struct FrameConstants {
    float invSize[2];
    UINT mode;
    float pad;
};

struct Gpu {
    ID3D11Device* device = nullptr;
    ID3D11DeviceContext* ctx = nullptr;
    ID3D11VertexShader* vsHud = nullptr;
    ID3D11VertexShader* vsFull = nullptr;
    ID3D11PixelShader* psHud = nullptr;
    ID3D11PixelShader* psComposite = nullptr;
    ID3D11PixelShader* psCompositeCopy = nullptr;
    ID3D11InputLayout* layout = nullptr;
    ID3D11Buffer* vb = nullptr;
    UINT vbCapacity = 0;
    ID3D11Buffer* cb = nullptr;
    ID3D11BlendState* blendHud = nullptr;
    ID3D11BlendState* blendPremul = nullptr;
    ID3D11BlendState* blendNone = nullptr;
    ID3D11SamplerState* linear = nullptr;
    ID3D11RasterizerState* raster = nullptr;
    ID3D11DepthStencilState* depth = nullptr;
    std::vector<ID3D11ShaderResourceView*> textures;
    std::uint32_t textureGeneration = 0;
    ID3D11Texture2D* target = nullptr;
    ID3D11RenderTargetView* targetRtv = nullptr;
    ID3D11ShaderResourceView* targetSrv = nullptr;
    UINT targetWidth = 0, targetHeight = 0;
    ID3D11Texture2D* copy = nullptr;
    ID3D11ShaderResourceView* copySrv = nullptr;
    UINT copyWidth = 0, copyHeight = 0;
    DXGI_FORMAT copyFormat = DXGI_FORMAT_UNKNOWN;
    DXGI_FORMAT srgbFailedFor = DXGI_FORMAT_UNKNOWN; // backbuffer format without an sRGB view
    int loggedMode = -1;
    std::uint64_t renderedSerial = ~0ull;
    bool failed = false;
} gpu;

void ReleaseSizeDependent() {
    SafeRelease(gpu.targetSrv);
    SafeRelease(gpu.targetRtv);
    SafeRelease(gpu.target);
    SafeRelease(gpu.copySrv);
    SafeRelease(gpu.copy);
    gpu.targetWidth = gpu.targetHeight = gpu.copyWidth = gpu.copyHeight = 0;
    gpu.copyFormat = DXGI_FORMAT_UNKNOWN;
    gpu.renderedSerial = ~0ull;
}

void ReleaseTextures() {
    for (auto*& t : gpu.textures) SafeRelease(t);
    gpu.textures.clear();
    gpu.textureGeneration = 0;
}

void ReleaseAll() {
    ReleaseSizeDependent();
    ReleaseTextures();
    SafeRelease(gpu.vsHud);
    SafeRelease(gpu.vsFull);
    SafeRelease(gpu.psHud);
    SafeRelease(gpu.psComposite);
    SafeRelease(gpu.psCompositeCopy);
    SafeRelease(gpu.layout);
    SafeRelease(gpu.vb);
    gpu.vbCapacity = 0;
    SafeRelease(gpu.cb);
    SafeRelease(gpu.blendHud);
    SafeRelease(gpu.blendPremul);
    SafeRelease(gpu.blendNone);
    SafeRelease(gpu.linear);
    SafeRelease(gpu.raster);
    SafeRelease(gpu.depth);
    SafeRelease(gpu.ctx);
    SafeRelease(gpu.device);
    gpu.srgbFailedFor = DXGI_FORMAT_UNKNOWN;
    gpu.loggedMode = -1;
}

ID3DBlob* Compile(const char* entry, const char* profile) {
    return d3dstate::CompileShader(kShaders, sizeof(kShaders) - 1, "skatev_hud.hlsl", entry, profile, g_log, "SkateV HUD");
}

bool CreateDeviceObjects(ID3D11Device* device) {
    gpu.device = device;
    gpu.device->AddRef();
    gpu.device->GetImmediateContext(&gpu.ctx);
    ID3DBlob* vs = Compile("VsHud", "vs_5_0");
    ID3DBlob* vsFull = Compile("VsFull", "vs_5_0");
    ID3DBlob* ps = Compile("PsHud", "ps_5_0");
    ID3DBlob* psC = Compile("PsComposite", "ps_5_0");
    ID3DBlob* psCC = Compile("PsCompositeCopy", "ps_5_0");
    bool ok = vs && vsFull && ps && psC && psCC;
    if (ok) {
        ok = SUCCEEDED(device->CreateVertexShader(vs->GetBufferPointer(), vs->GetBufferSize(), nullptr, &gpu.vsHud)) &&
             SUCCEEDED(device->CreateVertexShader(vsFull->GetBufferPointer(), vsFull->GetBufferSize(), nullptr,
                                                  &gpu.vsFull)) &&
             SUCCEEDED(device->CreatePixelShader(ps->GetBufferPointer(), ps->GetBufferSize(), nullptr, &gpu.psHud)) &&
             SUCCEEDED(device->CreatePixelShader(psC->GetBufferPointer(), psC->GetBufferSize(), nullptr,
                                                 &gpu.psComposite)) &&
             SUCCEEDED(device->CreatePixelShader(psCC->GetBufferPointer(), psCC->GetBufferSize(), nullptr,
                                                 &gpu.psCompositeCopy));
    }
    if (ok) {
        const D3D11_INPUT_ELEMENT_DESC elements[] = {
            {"POSITION", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 0, D3D11_INPUT_PER_VERTEX_DATA, 0},
            {"TEXCOORD", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 8, D3D11_INPUT_PER_VERTEX_DATA, 0},
            {"COLOR", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 16, D3D11_INPUT_PER_VERTEX_DATA, 0},
            {"COLOR", 1, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 32, D3D11_INPUT_PER_VERTEX_DATA, 0},
        };
        ok = SUCCEEDED(device->CreateInputLayout(elements, 4, vs->GetBufferPointer(), vs->GetBufferSize(), &gpu.layout));
    }
    SafeRelease(vs);
    SafeRelease(vsFull);
    SafeRelease(ps);
    SafeRelease(psC);
    SafeRelease(psCC);
    if (ok) {
        D3D11_BUFFER_DESC cb{};
        cb.ByteWidth = sizeof(FrameConstants);
        cb.Usage = D3D11_USAGE_DYNAMIC;
        cb.BindFlags = D3D11_BIND_CONSTANT_BUFFER;
        cb.CPUAccessFlags = D3D11_CPU_ACCESS_WRITE;
        ok = SUCCEEDED(device->CreateBuffer(&cb, nullptr, &gpu.cb));
    }
    if (ok) {
        D3D11_BLEND_DESC b{};
        auto& rt = b.RenderTarget[0];
        rt.BlendEnable = TRUE;
        rt.SrcBlend = D3D11_BLEND_SRC_ALPHA; // AlphaMode2d::Blend
        rt.DestBlend = D3D11_BLEND_INV_SRC_ALPHA;
        rt.BlendOp = D3D11_BLEND_OP_ADD;
        rt.SrcBlendAlpha = D3D11_BLEND_ONE;
        rt.DestBlendAlpha = D3D11_BLEND_INV_SRC_ALPHA;
        rt.BlendOpAlpha = D3D11_BLEND_OP_ADD;
        rt.RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL;
        ok = SUCCEEDED(device->CreateBlendState(&b, &gpu.blendHud));
        // PREMULTIPLIED_ALPHA_BLENDING; GTA's backbuffer alpha is left alone.
        rt.SrcBlend = D3D11_BLEND_ONE;
        rt.RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_RED | D3D11_COLOR_WRITE_ENABLE_GREEN |
                                   D3D11_COLOR_WRITE_ENABLE_BLUE;
        ok = ok && SUCCEEDED(device->CreateBlendState(&b, &gpu.blendPremul));
        rt.BlendEnable = FALSE;
        ok = ok && SUCCEEDED(device->CreateBlendState(&b, &gpu.blendNone));
    }
    if (ok) {
        D3D11_SAMPLER_DESC s{};
        s.Filter = D3D11_FILTER_MIN_MAG_MIP_LINEAR; // Bevy's default linear sampler
        s.AddressU = s.AddressV = s.AddressW = D3D11_TEXTURE_ADDRESS_CLAMP;
        s.MaxLOD = D3D11_FLOAT32_MAX;
        s.ComparisonFunc = D3D11_COMPARISON_NEVER;
        ok = SUCCEEDED(device->CreateSamplerState(&s, &gpu.linear));
    }
    if (ok) {
        D3D11_RASTERIZER_DESC r{};
        r.FillMode = D3D11_FILL_SOLID;
        r.CullMode = D3D11_CULL_NONE; // APT shapes are 2D; both windings draw
        r.DepthClipEnable = TRUE;
        ok = SUCCEEDED(device->CreateRasterizerState(&r, &gpu.raster));
    }
    if (ok) {
        D3D11_DEPTH_STENCIL_DESC d{};
        d.DepthEnable = FALSE;
        d.DepthWriteMask = D3D11_DEPTH_WRITE_MASK_ZERO;
        d.DepthFunc = D3D11_COMPARISON_ALWAYS;
        ok = SUCCEEDED(device->CreateDepthStencilState(&d, &gpu.depth));
    }
    if (!ok) {
        Logf("SkateV HUD: D3D11 setup failed; the original HUD is off for this session");
        ReleaseAll();
        gpu.failed = true;
        return false;
    }
    Logf("SkateV HUD: D3D11 renderer ready");
    return true;
}

bool UploadTextures(const TextureSet& set, std::uint32_t generation) {
    ReleaseTextures();
    for (const Texture& t : set) {
        D3D11_TEXTURE2D_DESC d{};
        d.Width = t.width;
        d.Height = t.height;
        d.MipLevels = 1;
        d.ArraySize = 1;
        d.Format = DXGI_FORMAT_R8G8B8A8_UNORM_SRGB; // Rgba8UnormSrgb, as the donor loads them
        d.SampleDesc.Count = 1;
        d.Usage = D3D11_USAGE_IMMUTABLE;
        d.BindFlags = D3D11_BIND_SHADER_RESOURCE;
        D3D11_SUBRESOURCE_DATA init{t.rgba.data(), t.width * 4, 0};
        ID3D11Texture2D* tex = nullptr;
        ID3D11ShaderResourceView* srv = nullptr;
        if (FAILED(gpu.device->CreateTexture2D(&d, &init, &tex)) ||
            FAILED(gpu.device->CreateShaderResourceView(tex, nullptr, &srv))) {
            SafeRelease(tex);
            Logf("SkateV HUD: texture upload failed");
            ReleaseTextures();
            return false;
        }
        SafeRelease(tex);
        gpu.textures.push_back(srv);
    }
    gpu.textureGeneration = generation;
    gpu.renderedSerial = ~0ull;
    Logf("SkateV HUD: %zu textures uploaded", gpu.textures.size());
    return true;
}

bool EnsureTarget(UINT w, UINT h) {
    if (gpu.target && gpu.targetWidth == w && gpu.targetHeight == h) return true;
    SafeRelease(gpu.targetSrv);
    SafeRelease(gpu.targetRtv);
    SafeRelease(gpu.target);
    D3D11_TEXTURE2D_DESC d{};
    d.Width = w;
    d.Height = h;
    d.MipLevels = 1;
    d.ArraySize = 1;
    d.Format = DXGI_FORMAT_R8G8B8A8_TYPELESS;
    d.SampleDesc.Count = 1;
    d.Usage = D3D11_USAGE_DEFAULT;
    d.BindFlags = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
    D3D11_RENDER_TARGET_VIEW_DESC rv{};
    rv.Format = DXGI_FORMAT_R8G8B8A8_UNORM_SRGB; // Rgba8UnormSrgb target
    rv.ViewDimension = D3D11_RTV_DIMENSION_TEXTURE2D;
    D3D11_SHADER_RESOURCE_VIEW_DESC sv{};
    sv.Format = DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
    sv.ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2D;
    sv.Texture2D.MipLevels = 1;
    if (FAILED(gpu.device->CreateTexture2D(&d, nullptr, &gpu.target)) ||
        FAILED(gpu.device->CreateRenderTargetView(gpu.target, &rv, &gpu.targetRtv)) ||
        FAILED(gpu.device->CreateShaderResourceView(gpu.target, &sv, &gpu.targetSrv))) {
        SafeRelease(gpu.targetSrv);
        SafeRelease(gpu.targetRtv);
        SafeRelease(gpu.target);
        return false;
    }
    gpu.targetWidth = w;
    gpu.targetHeight = h;
    gpu.renderedSerial = ~0ull;
    return true;
}

// The 8-bit RGBA/BGRA family of a backbuffer format: typeless, UNORM, sRGB.
bool Family(DXGI_FORMAT f, DXGI_FORMAT& typeless, DXGI_FORMAT& unorm, DXGI_FORMAT& srgb) {
    switch (f) {
    case DXGI_FORMAT_R8G8B8A8_TYPELESS:
    case DXGI_FORMAT_R8G8B8A8_UNORM:
    case DXGI_FORMAT_R8G8B8A8_UNORM_SRGB:
        typeless = DXGI_FORMAT_R8G8B8A8_TYPELESS;
        unorm = DXGI_FORMAT_R8G8B8A8_UNORM;
        srgb = DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
        return true;
    case DXGI_FORMAT_B8G8R8A8_TYPELESS:
    case DXGI_FORMAT_B8G8R8A8_UNORM:
    case DXGI_FORMAT_B8G8R8A8_UNORM_SRGB:
        typeless = DXGI_FORMAT_B8G8R8A8_TYPELESS;
        unorm = DXGI_FORMAT_B8G8R8A8_UNORM;
        srgb = DXGI_FORMAT_B8G8R8A8_UNORM_SRGB;
        return true;
    default:
        return false;
    }
}

bool EnsureCopy(UINT w, UINT h, DXGI_FORMAT typeless, DXGI_FORMAT srgb) {
    if (gpu.copy && gpu.copyWidth == w && gpu.copyHeight == h && gpu.copyFormat == typeless) return true;
    SafeRelease(gpu.copySrv);
    SafeRelease(gpu.copy);
    D3D11_TEXTURE2D_DESC d{};
    d.Width = w;
    d.Height = h;
    d.MipLevels = 1;
    d.ArraySize = 1;
    d.Format = typeless;
    d.SampleDesc.Count = 1;
    d.Usage = D3D11_USAGE_DEFAULT;
    d.BindFlags = D3D11_BIND_SHADER_RESOURCE;
    D3D11_SHADER_RESOURCE_VIEW_DESC sv{};
    sv.Format = srgb;
    sv.ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2D;
    sv.Texture2D.MipLevels = 1;
    if (FAILED(gpu.device->CreateTexture2D(&d, nullptr, &gpu.copy)) ||
        FAILED(gpu.device->CreateShaderResourceView(gpu.copy, &sv, &gpu.copySrv))) {
        SafeRelease(gpu.copySrv);
        SafeRelease(gpu.copy);
        return false;
    }
    gpu.copyWidth = w;
    gpu.copyHeight = h;
    gpu.copyFormat = typeless;
    return true;
}

bool EnsureVertexBuffer(UINT count) {
    return d3dstate::EnsureVertexBuffer(gpu.device, gpu.vb, gpu.vbCapacity, count, 4096, sizeof(Vertex));
}

void SetConstants(UINT w, UINT h, UINT mode) {
    D3D11_MAPPED_SUBRESOURCE m{};
    if (FAILED(gpu.ctx->Map(gpu.cb, 0, D3D11_MAP_WRITE_DISCARD, 0, &m))) return;
    FrameConstants c{{1.0f / static_cast<float>(w), 1.0f / static_cast<float>(h)}, mode, 0.0f};
    std::memcpy(m.pData, &c, sizeof(c));
    gpu.ctx->Unmap(gpu.cb, 0);
}

void RenderHud(const Frame& f) {
    ID3D11DeviceContext* c = gpu.ctx;
    const float clear[4] = {0.0f, 0.0f, 0.0f, 0.0f};
    c->OMSetRenderTargets(1, &gpu.targetRtv, nullptr);
    c->ClearRenderTargetView(gpu.targetRtv, clear);
    D3D11_VIEWPORT vp{0.0f, 0.0f, static_cast<float>(gpu.targetWidth), static_cast<float>(gpu.targetHeight), 0.0f, 1.0f};
    c->RSSetViewports(1, &vp);
    if (f.vertices.empty() || !EnsureVertexBuffer(static_cast<UINT>(f.vertices.size()))) return;
    D3D11_MAPPED_SUBRESOURCE m{};
    if (FAILED(c->Map(gpu.vb, 0, D3D11_MAP_WRITE_DISCARD, 0, &m))) return;
    std::memcpy(m.pData, f.vertices.data(), f.vertices.size() * sizeof(Vertex));
    c->Unmap(gpu.vb, 0);
    // Vertices are in the pixels of the size the runtime laid them out for;
    // during a resize they scale with the new backbuffer for a frame.
    SetConstants(f.width ? f.width : gpu.targetWidth, f.height ? f.height : gpu.targetHeight, 0);
    const UINT stride = sizeof(Vertex), offset = 0;
    c->IASetInputLayout(gpu.layout);
    c->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    c->IASetVertexBuffers(0, 1, &gpu.vb, &stride, &offset);
    c->VSSetShader(gpu.vsHud, nullptr, 0);
    c->VSSetConstantBuffers(0, 1, &gpu.cb);
    c->PSSetShader(gpu.psHud, nullptr, 0);
    c->PSSetSamplers(0, 1, &gpu.linear);
    const float factor[4] = {0, 0, 0, 0};
    c->OMSetBlendState(gpu.blendHud, factor, 0xFFFFFFFFu);
    for (const Draw& d : f.draws) {
        if (d.texture >= gpu.textures.size()) continue;
        c->PSSetShaderResources(0, 1, &gpu.textures[d.texture]);
        c->Draw(d.count, d.first);
    }
    ID3D11ShaderResourceView* none[2] = {};
    c->PSSetShaderResources(0, 2, none);
}

void Composite(ID3D11RenderTargetView* rtv, UINT w, UINT h, int mode) {
    ID3D11DeviceContext* c = gpu.ctx;
    c->OMSetRenderTargets(1, &rtv, nullptr);
    D3D11_VIEWPORT vp{0.0f, 0.0f, static_cast<float>(w), static_cast<float>(h), 0.0f, 1.0f};
    c->RSSetViewports(1, &vp);
    SetConstants(w, h, static_cast<UINT>(mode));
    c->IASetInputLayout(nullptr);
    c->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    c->VSSetShader(gpu.vsFull, nullptr, 0);
    c->PSSetConstantBuffers(0, 1, &gpu.cb);
    const float factor[4] = {0, 0, 0, 0};
    ID3D11ShaderResourceView* srv[2] = {gpu.targetSrv, mode == 1 ? gpu.copySrv : nullptr};
    if (mode == 1) {
        c->PSSetShader(gpu.psCompositeCopy, nullptr, 0);
        c->OMSetBlendState(gpu.blendNone, factor, 0xFFFFFFFFu);
    } else {
        c->PSSetShader(gpu.psComposite, nullptr, 0);
        c->OMSetBlendState(gpu.blendPremul, factor, 0xFFFFFFFFu);
    }
    c->PSSetShaderResources(0, 2, srv);
    c->Draw(3, 0);
    ID3D11ShaderResourceView* none[2] = {};
    c->PSSetShaderResources(0, 2, none);
}

} // namespace

void SetLog(LogFn log) { g_log = log; }

void SetCompositeOverride(int mode) { g_compositeOverride = mode == 1 || mode == 2 ? mode : -1; }

void Release() { ReleaseAll(); gpu.failed = false; }

int Present(IDXGISwapChain* sc, const Frame& frame) {
    if (gpu.failed || !sc || !frame.visible || frame.draws.empty() || !frame.textures) return -1;
    ID3D11Device* device = nullptr;
    if (FAILED(sc->GetDevice(__uuidof(ID3D11Device), reinterpret_cast<void**>(&device)))) return -1;
    if (device != gpu.device) {
        if (gpu.device) Logf("SkateV HUD: new D3D11 device; recreating the renderer");
        ReleaseAll();
        if (!CreateDeviceObjects(device)) {
            device->Release();
            return -1;
        }
    }
    device->Release(); // gpu.device holds its own reference
    if (gpu.device->GetDeviceRemovedReason() != S_OK) {
        Logf("SkateV HUD: device removed; renderer released");
        ReleaseAll();
        return -1;
    }
    if (gpu.textureGeneration != frame.textureGeneration &&
        !UploadTextures(*frame.textures, frame.textureGeneration)) {
        return -1;
    }

    ID3D11Texture2D* backbuffer = nullptr;
    if (FAILED(sc->GetBuffer(0, __uuidof(ID3D11Texture2D), reinterpret_cast<void**>(&backbuffer)))) return -1;
    D3D11_TEXTURE2D_DESC bd{};
    backbuffer->GetDesc(&bd);
    if (!EnsureTarget(bd.Width, bd.Height)) {
        backbuffer->Release();
        return -1;
    }
    // Backbuffer view: sRGB if the swapchain allows it (mode 0), else native.
    const bool ms = bd.SampleDesc.Count > 1;
    D3D11_RENDER_TARGET_VIEW_DESC rv{};
    rv.ViewDimension = ms ? D3D11_RTV_DIMENSION_TEXTURE2DMS : D3D11_RTV_DIMENSION_TEXTURE2D;
    DXGI_FORMAT typeless{}, unorm{}, srgb{};
    const bool eightBit = Family(bd.Format, typeless, unorm, srgb);
    ID3D11RenderTargetView* rtv = nullptr;
    int mode = 2;
    if (eightBit && gpu.srgbFailedFor != bd.Format && g_compositeOverride < 1) {
        rv.Format = srgb;
        if (SUCCEEDED(gpu.device->CreateRenderTargetView(backbuffer, &rv, &rtv))) {
            mode = 0;
        } else {
            gpu.srgbFailedFor = bd.Format;
        }
    }
    if (!rtv) {
        rv.Format = eightBit ? unorm : bd.Format;
        if (FAILED(gpu.device->CreateRenderTargetView(backbuffer, &rv, &rtv))) {
            backbuffer->Release();
            return -1;
        }
        mode = (eightBit && !ms && g_compositeOverride != 2 && EnsureCopy(bd.Width, bd.Height, typeless, srgb)) ? 1 : 2;
    }
    if (mode != gpu.loggedMode) {
        static const char* names[] = {"sRGB view, hardware blend", "frame copy, exact linear blend",
                                      "display-gamma blend (approximate)"};
        Logf("SkateV HUD: backbuffer %ux%u format %d samples %u: composite via %s", bd.Width, bd.Height,
             static_cast<int>(bd.Format), bd.SampleDesc.Count, names[mode]);
        gpu.loggedMode = mode;
    }

    SavedState saved;
    saved.Save(gpu.ctx);
    gpu.ctx->GSSetShader(nullptr, nullptr, 0);
    gpu.ctx->HSSetShader(nullptr, nullptr, 0);
    gpu.ctx->DSSetShader(nullptr, nullptr, 0);
    gpu.ctx->RSSetState(gpu.raster);
    gpu.ctx->OMSetDepthStencilState(gpu.depth, 0);
    if (gpu.renderedSerial != frame.serial) {
        RenderHud(frame);
        gpu.renderedSerial = frame.serial;
    }
    if (mode == 1) gpu.ctx->CopyResource(gpu.copy, backbuffer);
    Composite(rtv, bd.Width, bd.Height, mode);
    saved.Restore(gpu.ctx);
    rtv->Release();
    backbuffer->Release(); // nothing is held across frames: ResizeBuffers stays legal
    return mode;
}
} // namespace hudrender

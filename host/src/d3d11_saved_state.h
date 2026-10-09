#pragma once
// GTA's D3D11 pipeline state around the overlay passes (hud_renderer,
// xray_renderer): everything they touch is saved before drawing and restored
// after, so GTA's own rendering continues untouched.
#include <d3d11.h>
#include <d3dcompiler.h>
#include <cstddef>
#include "host_util.h"

namespace d3dstate {
template <class T>
void SafeRelease(T*& p) {
    if (p) {
        p->Release();
        p = nullptr;
    }
}

// Everything the overlays touch, as GTA left it.
struct SavedState {
    ID3D11RenderTargetView* rtv[D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT]{};
    ID3D11DepthStencilView* dsv = nullptr;
    ID3D11BlendState* blend = nullptr;
    float blendFactor[4]{};
    UINT sampleMask = 0;
    ID3D11DepthStencilState* depth = nullptr;
    UINT stencilRef = 0;
    ID3D11RasterizerState* raster = nullptr;
    D3D11_VIEWPORT viewports[D3D11_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE]{};
    UINT viewportCount = D3D11_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE;
    D3D11_RECT scissors[D3D11_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE]{};
    UINT scissorCount = D3D11_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE;
    ID3D11InputLayout* layout = nullptr;
    D3D11_PRIMITIVE_TOPOLOGY topology = D3D11_PRIMITIVE_TOPOLOGY_UNDEFINED;
    ID3D11Buffer* vb = nullptr;
    UINT vbStride = 0, vbOffset = 0;
    ID3D11Buffer* ib = nullptr;
    DXGI_FORMAT ibFormat = DXGI_FORMAT_UNKNOWN;
    UINT ibOffset = 0;
    ID3D11VertexShader* vs = nullptr;
    ID3D11PixelShader* ps = nullptr;
    ID3D11GeometryShader* gs = nullptr;
    ID3D11HullShader* hs = nullptr;
    ID3D11DomainShader* ds = nullptr;
    ID3D11Buffer* vsCb = nullptr;
    ID3D11Buffer* psCb = nullptr;
    ID3D11ShaderResourceView* psSrv[2]{};
    ID3D11SamplerState* psSampler = nullptr;

    void Save(ID3D11DeviceContext* c) {
        c->OMGetRenderTargets(D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT, rtv, &dsv);
        c->OMGetBlendState(&blend, blendFactor, &sampleMask);
        c->OMGetDepthStencilState(&depth, &stencilRef);
        c->RSGetState(&raster);
        c->RSGetViewports(&viewportCount, viewports);
        c->RSGetScissorRects(&scissorCount, scissors);
        c->IAGetInputLayout(&layout);
        c->IAGetPrimitiveTopology(&topology);
        c->IAGetVertexBuffers(0, 1, &vb, &vbStride, &vbOffset);
        c->IAGetIndexBuffer(&ib, &ibFormat, &ibOffset);
        c->VSGetShader(&vs, nullptr, nullptr);
        c->PSGetShader(&ps, nullptr, nullptr);
        c->GSGetShader(&gs, nullptr, nullptr);
        c->HSGetShader(&hs, nullptr, nullptr);
        c->DSGetShader(&ds, nullptr, nullptr);
        c->VSGetConstantBuffers(0, 1, &vsCb);
        c->PSGetConstantBuffers(0, 1, &psCb);
        c->PSGetShaderResources(0, 2, psSrv);
        c->PSGetSamplers(0, 1, &psSampler);
    }

    void Restore(ID3D11DeviceContext* c) {
        c->OMSetRenderTargets(D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT, rtv, dsv);
        c->OMSetBlendState(blend, blendFactor, sampleMask);
        c->OMSetDepthStencilState(depth, stencilRef);
        c->RSSetState(raster);
        c->RSSetViewports(viewportCount, viewports);
        c->RSSetScissorRects(scissorCount, scissors);
        c->IASetInputLayout(layout);
        c->IASetPrimitiveTopology(topology);
        c->IASetVertexBuffers(0, 1, &vb, &vbStride, &vbOffset);
        c->IASetIndexBuffer(ib, ibFormat, ibOffset);
        c->VSSetShader(vs, nullptr, 0);
        c->PSSetShader(ps, nullptr, 0);
        c->GSSetShader(gs, nullptr, 0);
        c->HSSetShader(hs, nullptr, 0);
        c->DSSetShader(ds, nullptr, 0);
        c->VSSetConstantBuffers(0, 1, &vsCb);
        c->PSSetConstantBuffers(0, 1, &psCb);
        c->PSSetShaderResources(0, 2, psSrv);
        c->PSSetSamplers(0, 1, &psSampler);
        for (auto*& r : rtv) SafeRelease(r);
        SafeRelease(dsv);
        SafeRelease(blend);
        SafeRelease(depth);
        SafeRelease(raster);
        SafeRelease(layout);
        SafeRelease(vb);
        SafeRelease(ib);
        SafeRelease(vs);
        SafeRelease(ps);
        SafeRelease(gs);
        SafeRelease(hs);
        SafeRelease(ds);
        SafeRelease(vsCb);
        SafeRelease(psCb);
        for (auto*& s : psSrv) SafeRelease(s);
        SafeRelease(psSampler);
    }
};

// One entry point of an HLSL source; null (logged as `tag`) when it fails.
inline ID3DBlob* CompileShader(const char* source, std::size_t size, const char* name, const char* entry,
                               const char* profile, util::LogFn log, const char* tag) {
    ID3DBlob* code = nullptr;
    ID3DBlob* errors = nullptr;
    const HRESULT hr = D3DCompile(source, size, name, nullptr, nullptr, entry, profile, D3DCOMPILE_OPTIMIZATION_LEVEL3, 0,
                                  &code, &errors);
    if (FAILED(hr)) {
        util::Logf(log, "%s: shader %s failed (0x%08lx): %s", tag, entry, static_cast<unsigned long>(hr),
                   errors ? static_cast<const char*>(errors->GetBufferPointer()) : "");
        SafeRelease(code);
    }
    SafeRelease(errors);
    return code;
}

// A dynamic vertex buffer of at least `count` vertices (capacity doubles from `initial`).
inline bool EnsureVertexBuffer(ID3D11Device* device, ID3D11Buffer*& vb, UINT& capacity, UINT count, UINT initial,
                               UINT stride) {
    if (vb && capacity >= count) return true;
    SafeRelease(vb);
    UINT c = initial;
    while (c < count) c *= 2;
    D3D11_BUFFER_DESC d{};
    d.ByteWidth = c * stride;
    d.Usage = D3D11_USAGE_DYNAMIC;
    d.BindFlags = D3D11_BIND_VERTEX_BUFFER;
    d.CPUAccessFlags = D3D11_CPU_ACCESS_WRITE;
    if (FAILED(device->CreateBuffer(&d, nullptr, &vb))) return false;
    capacity = c;
    return true;
}

} // namespace d3dstate

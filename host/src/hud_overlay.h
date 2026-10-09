#pragma once
// Skate 3's original HUD over GTA: the runtime runs the APT trickdisplay movie
// and publishes a flat draw list (ABI 8 hud block); this module fetches it on
// the script thread and rasterises it on GTA's render thread through
// ScriptHookV's IDXGISwapChain::Present callback with a small D3D11 renderer
// (the donor's hud_render.wgsl / hud_composite.wgsl maths). GTA's pipeline
// state is saved and restored around every draw.
//
// ini [SkateV]: Hud=1 (0 disables), HudMaxAspect=2.4 (widest HUD region,
// 0 = default, -1 = full width), HudSafeZone=-1 (GTA's GET_SAFE_ZONE_SIZE;
// 0.5..1.0 overrides), HudComposite=-1 (auto; 1 forces the frame-copy
// composite, 2 the display-gamma blend: troubleshooting only).
#include <string>

namespace hudoverlay {
using LogFn = void (*)(const char*);

// After the runtime loaded: looks up the optional HUD exports and registers
// the Present callback. `runtime` is the sv_create handle (nullptr: inert).
void Start(const std::wstring& iniPath, void* runtime, LogFn log);
// Script thread, once per frame. `skating`: Skate is active (the HUD shows,
// unless the pause menu or a loading screen is up).
void Tick(bool skating);
// Script thread: shows Skate's TRAX banner (big top line, two smaller lines,
// GET_RADIO_STATION_NAME id for its logo). False when the HUD or the banner
// is unavailable (the caller notifies instead).
bool ShowRadio(const char* top, const char* middle, const char* bottom, const char* station);
// DllMain detach: unregisters the callback; releases GPU objects unless the
// process is exiting.
void Shutdown(bool processExit);
}

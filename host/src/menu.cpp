// In-game settings menu (see menu.h).
#include "menu.h"
#include <windows.h>
#include <main.h>
#include <map>
#include <string>
#include "host_util.h"
#include "natives.h"

namespace menu {
namespace {
using gta::Call;

// Frontend controls: arrows / d-pad, Enter / A, Backspace / B.
constexpr int kUp = 172, kDown = 173, kLeft = 174, kRight = 175, kAccept = 176, kCancel = 177;
constexpr int kVisibleRows = 12;
constexpr ULONGLONG kCaptureMs = 8000;

// GTA Interaction Menu proportions at 16:9 (width 432 px of 1920, rows 38 px of 1080).
constexpr float kW = 0.225f, kPad = 0.006f, kBanner = 0.09f, kSub = 0.03f, kRow = 0.035f, kChevron = 0.022f, kHelp = 0.06f;

std::wstring g_ini;
std::vector<Page> g_pages;
std::function<unsigned()> g_pad;
LogFn g_log = nullptr;
bool g_open = false;
bool g_closing = false;         // closed by Toggle / the pause menu this frame
int g_level = 0;                // 0 categories, 1 a page's items
int g_sel[2] = {0, 0};          // selection per level
int g_pageIndex = 0;
std::map<std::string, std::string> g_cache;  // INI text per key, refreshed on open
std::string g_status;
ULONGLONG g_statusUntil = 0;

// Chord capture: hold the buttons, release to set.
bool g_capturing = false, g_captureArmed = false;
unsigned g_captureMask = 0;
ULONGLONG g_captureStart = 0;

struct Repeat {
    ULONGLONG since = 0, next = 0;
} g_repeat[4];

using util::Wide;

std::string ReadIni(const Item& it) {
    const std::string v = util::IniString(g_ini, Wide(it.key));
    return v.empty() ? it.def : v;
}

void Refresh() {
    for (const Page& p : g_pages)
        for (const Item& it : p.items)
            if (!it.key.empty()) g_cache[it.key] = ReadIni(it);
}

void Say(const std::string& text) {
    g_status = text;
    g_statusUntil = GetTickCount64() + 2500;
}

void Commit(Item& it, const std::string& value) {
    if (!WritePrivateProfileStringW(L"SkateV", Wide(it.key).c_str(), Wide(value).c_str(), g_ini.c_str())) {
        Say("Could not save SkateVLegacy.ini");
        return;
    }
    g_cache[it.key] = value;
    if (it.apply) it.apply(value);
    Say(it.restart ? "Saved - applies after restarting GTA" : "Saved");
    if (g_log) g_log(("SkateV menu: " + it.key + "=" + value).c_str());
}

void Beep(const char* name) { Call<void>(gta::PLAY_SOUND_FRONTEND, -1, name, "HUD_FRONTEND_DEFAULT_SOUNDSET", 1); }

bool Fire(int slot, int control) {
    Repeat& r = g_repeat[slot];
    const ULONGLONG now = GetTickCount64();
    if (Call<BOOL>(gta::IS_DISABLED_CONTROL_JUST_PRESSED, 0, control)) {
        r.since = now, r.next = now + 350;
        return true;
    }
    if (!Call<BOOL>(gta::IS_DISABLED_CONTROL_PRESSED, 0, control)) {
        r.since = 0;
        return false;
    }
    if (r.since && now >= r.next) {
        r.next = now + 70;
        return true;
    }
    return false;
}

using gta::Align;

// Right: text ends at `edge`. Wrap: text starts at x and wraps at `edge`.
void Text(const std::string& s, float x, float y, float scale, int shade, Align align = Align::Left, float edge = 0.0f) {
    gta::Text(s.c_str(), x, y, scale, shade, shade, shade, 255, align, edge, false);
}

void Rect(float x, float y, float w, float h, int r, int g, int b, int a) {
    Call<void>(gta::DRAW_RECT, x + w / 2, y + h / 2, w, h, r, g, b, a);
}

// Small pixel-stepped triangle (1080p pixels), the chevron in the footer strip.
void Chevron(float cx, float cy, bool up) {
    for (int i = 0; i < 5; ++i) {
        const float w = (2.0f + 2.0f * i) / 1920.0f, h = 2.0f / 1080.0f;
        Call<void>(gta::DRAW_RECT, cx, cy + (up ? i - 2 : 2 - i) * h, w, h, 200, 200, 200, 255);
    }
}

void Row(float x, float y, const std::string& label, const std::string& value, bool selected) {
    if (selected) Rect(x, y, kW, kRow, 240, 240, 240, 235);
    else Rect(x, y, kW, kRow, 0, 0, 0, 160);
    const int shade = selected ? 10 : 255;
    Text(label, x + kPad, y + 0.003f, 0.34f, shade);
    if (!value.empty()) Text(value, 0.0f, y + 0.003f, 0.34f, shade, Align::Right, x + kW - kPad);
}

void Draw() {
    // Left-aligned inside GTA's safe zone, like the Interaction Menu.
    const float margin = (1.0f - Call<float>(gta::GET_SAFE_ZONE_SIZE)) / 2;
    const float x = margin + 0.02f;
    float y = margin + 0.04f;

    // Banner: black block, big title, orange rule under it.
    Rect(x, y, kW, kBanner, 12, 12, 12, 235);
    Rect(x, y + kBanner - 0.004f, kW, 0.004f, 255, 110, 0, 255);
    Text("SK8V", x + kPad, y + 0.008f, 0.8f, 255);
    y += kBanner;

    // Subtitle bar: page title left, "n / total" right.
    const int count = g_level ? static_cast<int>(g_pages[g_pageIndex].items.size()) : static_cast<int>(g_pages.size());
    const int sel = g_sel[g_level];
    Rect(x, y, kW, kSub, 255, 110, 0, 235);
    Text(Upper(g_level ? g_pages[g_pageIndex].title : "Settings"), x + kPad, y + 0.0035f, 0.3f, 255);
    Text(Counter(sel, count), 0.0f, y + 0.0035f, 0.3f, 255, Align::Right, x + kW - kPad);
    y += kSub;

    const int first = std::clamp(sel - kVisibleRows / 2, 0, std::max(0, count - kVisibleRows));
    const int shown = std::min(kVisibleRows, count - first);
    for (int i = first; i < first + shown; ++i, y += kRow) {
        if (!g_level) {
            Row(x, y, g_pages[static_cast<std::size_t>(i)].title, ">", i == sel);
            continue;
        }
        const Item& it = g_pages[g_pageIndex].items[static_cast<std::size_t>(i)];
        std::string value = it.kind == Kind::Action ? "" : i == sel ? ShowSelected(it, g_cache[it.key]) : Show(it, g_cache[it.key]);
        if (it.kind == Kind::Chord && g_capturing && i == sel) value = ChordName(g_captureMask) + " ...";
        Row(x, y, it.label + (it.restart ? " *" : ""), value, i == sel);
    }

    // Chevron strip, then the description box.
    Rect(x, y, kW, kChevron, 0, 0, 0, 160);
    Chevron(x + kW / 2 - 0.008f, y + kChevron / 2, true);
    Chevron(x + kW / 2 + 0.008f, y + kChevron / 2, false);
    y += kChevron;
    Rect(x, y, kW, kHelp, 0, 0, 0, 160);
    std::string text;
    int shade = 210;
    if (g_capturing) {
        text = g_pages[g_pageIndex].items[static_cast<std::size_t>(sel)].allowOff
                   ? "Hold the buttons, release to set. Backspace = off, Esc = cancel"
                   : "Hold the buttons, release to set. Esc = cancel";
    } else if (GetTickCount64() < g_statusUntil) {
        text = g_status, shade = 255;
    } else {
        text = g_level ? kHint : PageHint(g_pages[static_cast<std::size_t>(sel)].title);
    }
    Text(text, x + kPad, y + 0.005f, 0.28f, shade, Align::Wrap, x + kW - kPad);
}

void Capture(Item& it) {
    const unsigned pad = g_pad ? g_pad() & 0xF3FFu : 0;
    if (!g_captureArmed) {  // the press that started the capture is still down
        g_captureArmed = pad == 0;
        return;
    }
    g_captureMask |= pad;
    if (GetAsyncKeyState(VK_ESCAPE) & 0x8000 || GetTickCount64() - g_captureStart > kCaptureMs) {
        g_capturing = false;
    } else if (it.allowOff && (GetAsyncKeyState(VK_BACK) & 0x8000)) {
        g_capturing = false;
        Commit(it, "0x0000");
    } else if (pad == 0 && g_captureMask) {
        g_capturing = false;
        char text[16];
        std::snprintf(text, sizeof(text), "0x%04X", g_captureMask);
        Commit(it, text);
    }
}

void Navigate() {
    const int count = g_level ? static_cast<int>(g_pages[g_pageIndex].items.size()) : static_cast<int>(g_pages.size());
    int& sel = g_sel[g_level];
    if (Fire(0, kUp)) sel = (sel + count - 1) % count, Beep("NAV_UP_DOWN");
    if (Fire(1, kDown)) sel = (sel + 1) % count, Beep("NAV_UP_DOWN");
    const bool left = Fire(2, kLeft), right = Fire(3, kRight);
    const bool accept = Call<BOOL>(gta::IS_DISABLED_CONTROL_JUST_PRESSED, 0, kAccept);
    if (Call<BOOL>(gta::IS_DISABLED_CONTROL_JUST_PRESSED, 0, kCancel)) {
        Beep("BACK");
        if (g_level) g_level = 0;
        else g_open = false;
        return;
    }
    if (!g_level) {
        if (accept) g_pageIndex = sel, g_level = 1, g_sel[1] = 0, Beep("SELECT");
        return;
    }
    Item& it = g_pages[g_pageIndex].items[static_cast<std::size_t>(sel)];
    if (it.kind == Kind::Action) {
        if (accept) {
            Beep("SELECT");
            g_open = false;  // actions act on the game: give the controls back first
            if (it.run) it.run();
        }
    } else if (it.kind == Kind::Chord) {
        if (accept) g_capturing = true, g_captureArmed = false, g_captureMask = 0, g_captureStart = GetTickCount64();
    } else if (left || right || accept) {
        Commit(it, Step(it, g_cache[it.key], left ? -1 : 1));
        Beep("NAV_LEFT_RIGHT");
    }
}
}  // namespace

void Start(const std::wstring& ini, std::vector<Page> pages, std::function<unsigned()> padButtons, LogFn log) {
    g_ini = ini, g_pages = std::move(pages), g_pad = std::move(padButtons), g_log = log;
}

void Toggle() {
    if (g_pages.empty()) return;
    if (g_open) {
        g_open = g_capturing = false;
        g_closing = true;
        return;
    }
    if (gta::FrontendActive()) return;
    Refresh();
    g_open = true;
}

bool IsOpen() { return g_open; }

void Tick() {
    // Pause (199, 200) stays GTA's; one control at a time, never DISABLE_ALL_CONTROL_ACTIONS
    // (it stops Rockstar Editor recording, see main.cpp DisableSkateControls). While open
    // this is the only control loop (DisableSkateControls skips), so it also runs on the
    // frame the menu closes.
    const auto disableControls = [] {
        for (int c = 0; c < 360; ++c)
            if (c != 199 && c != 200) Call<void>(gta::DISABLE_CONTROL_ACTION, 0, c, 1);
    };
    if (!g_open) {
        if (g_closing) disableControls();
        g_closing = false;
        return;
    }
    if (gta::FrontendActive()) {
        g_open = g_capturing = false;
        disableControls();
        return;
    }
    disableControls();
    if (g_capturing) Capture(g_pages[g_pageIndex].items[static_cast<std::size_t>(g_sel[1])]);
    else Navigate();
    if (g_open) Draw();
}

}  // namespace menu

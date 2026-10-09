// Player records over GTA (see records_ui.h).
#include "records_ui.h"
#include "host_util.h"
#include <windows.h>
#include <main.h>
#include <cstdint>
#include <cstring>
#include <cstdio>
#include <ctime>
#include <string>
#include "natives.h"
#include "skatev_runtime.h"

namespace recordsui {
namespace {
using gta::Call;

// Skate 3 language ids (rust/skatev-runtime/src/records.rs LABELS) and the
// plain fallback shown when the converted HUD language table is missing.
enum Label { PlayerRecords, NewPersonalBest, HomTitle, LineTitle, HomRecord, NoScores, LabelCount };
const char* const kIds[LabelCount] = {
    "ID_LEADERBOARD_PLAYER_RECORDS", "ID_LEADERBOARD_NEW_PERSONAL_BEST", "ID_RECORD_BESTS_34",
    "ID_RECORD_BESTS_05",            "ID_HOM_ANNALS_OF_MEAT_RECORD",
    "ID_RANKED_LEADERBOARDS_NO_SCORES"};
const char* const kFallback[LabelCount] = {"Records", "New best", "Hall of Meat", "Line",
                                           "Hall of Meat best:", "-"};

// GTA's minimap in screen fractions, as skate-hud's hom.rs places the score
// box: anchored to the safe area's bottom-left, 1/5.674 of the screen tall,
// the box's bottom FEED_ROOM (0.25) minimap heights above it.
constexpr float kMinimapHeight = 1.0f / 5.674f;
constexpr float kFeedRoom = 0.25f;
constexpr ULONGLONG kShardMs = 4000;
// The book keeps a top ten; only a top-three place gets the call-out.
constexpr std::uint32_t kAnnounceTop = 3;

struct State {
    LogFn log = nullptr;
    void* rt = nullptr;
    SvRecordsSetContextFn setContext = nullptr;
    SvRecordsGetStateFn getState = nullptr;
    SvRecordsGetTopFn getTop = nullptr;
    bool ready = false;
    std::string labels[LabelCount];
    std::string character;
    ULONGLONG contextAt = 0;
    std::uint32_t seen = 0;
    bool seeded = false;
    int shard = 0;
    bool shardSent = false;
    ULONGLONG shardUntil = 0;
    std::string shardTitle, shardBody;
    int page = 0;
} g;

template <class... A>
void Logf(const char* fmt, A... a) {
    util::Logf(g.log, fmt, a...);
}

const char* Character(std::uint32_t model) {
    if (model == util::Joaat("player_zero")) return "Michael";
    if (model == util::Joaat("player_one")) return "Franklin";
    if (model == util::Joaat("player_two")) return "Trevor";
    return "Other";
}

std::string Number(std::uint32_t v) {
    std::string digits = std::to_string(v), out;
    for (std::size_t i = 0; i < digits.size(); ++i) {
        if (i && (digits.size() - i) % 3 == 0) out += ',';
        out += digits[i];
    }
    return out;
}

// White, outlined; rightEdge > 0: right-justified to it.
void Text(const std::string& s, float x, float y, float scale, bool centre = false, float rightEdge = -1.0f,
          int alpha = 255) {
    const gta::Align align = rightEdge > 0.0f ? gta::Align::Right : centre ? gta::Align::Centre : gta::Align::Left;
    gta::Text(s.c_str(), x, y, scale, 255, 255, 255, alpha, align, rightEdge);
}

void UpdateContext() {
    const ULONGLONG now = GetTickCount64();
    if (now - g.contextAt < 1000) return;
    g.contextAt = now;
    const Ped ped = Call<Ped>(gta::PLAYER_PED_ID);
    if (!Call<BOOL>(gta::DOES_ENTITY_EXIST, ped)) return;
    const std::string character = Character(Call<std::uint32_t>(gta::GET_ENTITY_MODEL, ped));
    if (character == g.character) return;
    g.character = character;
    // No zones: every score goes in under one empty spot.
    g.setContext(g.rt, g.character.c_str(), "", "");
}

bool Podium(std::uint32_t rank) { return rank >= 1 && rank <= kAnnounceTop; }

void StartShard(const SvRecordState& s) {
    if (!Podium(s.rank)) return;
    g.shardTitle = g.labels[s.rank == 1 ? NewPersonalBest : PlayerRecords];
    g.shardBody = g.labels[s.category == 1 ? HomTitle : LineTitle] + " " + Number(s.score) + "  -  #" +
                  std::to_string(s.rank);
    if (!g.shard) g.shard = Call<int>(gta::REQUEST_SCALEFORM_MOVIE, "MIDSIZED_MESSAGE");
    g.shardSent = false;
    g.shardUntil = GetTickCount64() + kShardMs;
    Logf("SkateV records: %s: %s", g.shardTitle.c_str(), g.shardBody.c_str());
}

void DrawShard(bool paused) {
    if (!g.shard) return;
    if (GetTickCount64() >= g.shardUntil) {
        Call<void>(gta::SET_SCALEFORM_MOVIE_AS_NO_LONGER_NEEDED, &g.shard);
        g.shard = 0;
        return;
    }
    if (!Call<BOOL>(gta::HAS_SCALEFORM_MOVIE_LOADED, g.shard)) return;
    if (!g.shardSent) {
        Call<BOOL>(gta::BEGIN_SCALEFORM_MOVIE_METHOD, g.shard, "SHOW_SHARD_MIDSIZED_MESSAGE");
        Call<void>(gta::SCALEFORM_MOVIE_METHOD_ADD_PARAM_TEXTURE_NAME_STRING, g.shardTitle.c_str());
        Call<void>(gta::SCALEFORM_MOVIE_METHOD_ADD_PARAM_TEXTURE_NAME_STRING, g.shardBody.c_str());
        Call<void>(gta::END_SCALEFORM_MOVIE_METHOD);
        g.shardSent = true;
    }
    if (!paused) Call<void>(gta::DRAW_SCALEFORM_MOVIE_FULLSCREEN, g.shard, 255, 255, 255, 255, 0);
}

// Under the Hall of Meat score box, in the gap above the minimap.
void DrawBailRecord(const SvRecordState& s) {
    if (!s.hom_best) return;
    float safe = Call<float>(gta::GET_SAFE_ZONE_SIZE);
    if (!(safe >= 0.5f && safe <= 1.0f)) safe = 1.0f;
    const float left = (1.0f - safe) * 0.5f, bottom = 1.0f - (1.0f - safe) * 0.5f;
    const float mapTop = bottom - kMinimapHeight, boxBottom = mapTop - kFeedRoom * kMinimapHeight;
    Text(g.labels[HomRecord] + " " + Number(s.hom_best), left, boxBottom + 0.006f, 0.3f);
}

void DrawColumn(std::uint32_t category, float x0, float x1, float y) {
    Text(g.labels[category == 1 ? HomTitle : LineTitle], x0, y, 0.38f);
    SvRecordEntry rows[10]{};
    const std::uint32_t n = g.getTop(g.rt, category, 0u, rows, 10);
    if (!n) {
        Text(g.labels[NoScores], x0, y + 0.04f, 0.32f, false, -1.0f, 180);
        return;
    }
    for (std::uint32_t i = 0; i < n; ++i) {
        const SvRecordEntry& e = rows[i];
        const float ry = y + 0.04f + 0.03f * static_cast<float>(i);
        const std::time_t t = static_cast<std::time_t>(e.time);
        std::tm tm{};
        char date[16] = "";
        if (t && !localtime_s(&tm, &t)) std::strftime(date, sizeof(date), "%m/%d", &tm);
        Text(std::to_string(i + 1) + "  " + std::string(e.character_utf8) + "  " + date, x0, ry, 0.32f);
        Text(Number(e.score), 0.0f, ry, 0.32f, false, x1);
    }
}

void DrawBoard() {
    Call<void>(gta::DRAW_RECT, 0.5f, 0.5f, 0.64f, 0.58f, 0, 0, 0, 175);
    Text(g.labels[PlayerRecords], 0.5f, 0.225f, 0.55f, true);
    DrawColumn(1, 0.195f, 0.485f, 0.285f);
    DrawColumn(2, 0.515f, 0.805f, 0.285f);
}
} // namespace

void Start(void* runtime, LogFn log) {
    g.log = log;
    const HMODULE module = GetModuleHandleW(L"SkateVRuntime.dll");
    if (!runtime || !module) return;
    const auto open = reinterpret_cast<SvRecordsOpenFn>(GetProcAddress(module, "sv_records_open"));
    g.setContext = reinterpret_cast<SvRecordsSetContextFn>(GetProcAddress(module, "sv_records_set_context"));
    g.getState = reinterpret_cast<SvRecordsGetStateFn>(GetProcAddress(module, "sv_records_get_state"));
    g.getTop = reinterpret_cast<SvRecordsGetTopFn>(GetProcAddress(module, "sv_records_get_top"));
    const auto label = reinterpret_cast<SvRecordsLabelFn>(GetProcAddress(module, "sv_records_label"));
    if (!open || !g.setContext || !g.getState || !g.getTop || !label) {
        Logf("SkateV records: runtime has no record exports; records off");
        return;
    }
    char local[MAX_PATH]{};
    if (!GetEnvironmentVariableA("LOCALAPPDATA", local, MAX_PATH)) return;
    const std::string path = std::string(local) + "\\SkateV\\records.json";
    g.rt = runtime;
    if (!open(runtime, path.c_str())) {
        Logf("SkateV records: could not open %s", path.c_str());
        return;
    }
    int found = 0;
    for (int i = 0; i < LabelCount; ++i) {
        char buf[128]{};
        const bool ok = label(runtime, kIds[i], buf, sizeof(buf)) != 0;
        g.labels[i] = ok ? buf : kFallback[i];
        found += ok ? 1 : 0;
    }
    g.ready = true;
    Logf("SkateV records: book %s, %d/%d Skate labels", path.c_str(), found, static_cast<int>(LabelCount));
}

void NextPage() {
    if (!g.ready) return;
    g.page = !g.page;
}

bool Showing() { return g.page != 0; }

void Tick(bool skating, bool homBail) {
    if (!g.ready) return;
    const bool paused = gta::FrontendActive();
    if (skating) UpdateContext();
    SvRecordState s{};
    s.size = sizeof(s);
    if (!g.getState(g.rt, &s)) return;
    if (!g.seeded) {
        g.seen = s.sequence;
        g.seeded = true;
    } else if (s.sequence != g.seen) {
        g.seen = s.sequence;
        StartShard(s);
    }
    DrawShard(paused);
    if (paused) return;
    if (skating && homBail) DrawBailRecord(s);
    if (g.page) DrawBoard();
}
}

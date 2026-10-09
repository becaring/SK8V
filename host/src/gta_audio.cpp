#include "audio_entity_hook.h"
#include "gta_audio.h"

#include "gta_audio_dsp.h"
#include "gta_audio_re.h"
#include "host_util.h"
#include "natives.h"
#include "skatev_runtime.h"

#include <windows.h>
#include <shlobj.h>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <set>
#include <string>
#include <vector>

namespace gtaaudio {
namespace {
using gta::Call;

constexpr int kSlots = SV_AUDIO_EMITTER_COUNT;
constexpr int kRate = SV_AUDIO_SAMPLE_RATE;
constexpr int kChunk = 4096;          // max frames moved per slot per feeder pass
constexpr DWORD kLingerMs = 2000;     // keep a stream this long after its emitter went quiet
constexpr int kMaxDrops = 20;         // GTA dropped our sounds this often -> silent
const char* const kSlotName[kSlots] = {"board", "body", "speed"};

enum class Path { Off, Gta };

struct Settings {
    bool on = true;
    std::string cache;
    float masterGain = 1.0f;
    bool tracker = false;
    bool editorTone = false; // AudioEditorTone (dev): 440 Hz beeps with the clip sound in the Rockstar Editor
    // GTA category for every Skate stream (update.rpf categories.dat22). The
    // category chain is the bridge's whole loss; 0xD4AE89CA (unnamed, under
    // game_world/Weapons) is the loudest plain one: chain -2 dB, rolloff x7.5.
    // Names hash with atStringHash; "0x..." is a raw hash; "SOUND" keeps the
    // sound's own. See evidence/2026-10-02/audio-gta-calibration.md.
    std::string category = "0xD4AE89CA";
    // Level of Skate's PCM fed to every GTA stream (dB), so Skate keeps its
    // retail balance against GTA's mix. Applied to the samples (through the
    // limiter) because GTA caps a voice at 0 dB. Set in game from the menu.
    float gainDb = 1.4f;
    int bufferMs = 50;
};

void (*g_log)(const char*) = nullptr;
void Log(const char* s) {
    if (g_log) g_log(s);
}
template <class... A>
void Logf(const char* fmt, A... a) {
    util::Logf(g_log, fmt, a...);
}

Settings g_set;
bool g_started = false;
std::atomic<Path> g_path{Path::Off};
gtare::Audio g_re;
volatile std::uint8_t* g_mixerWaitFlag = nullptr;
void* g_category = nullptr;
std::uint32_t g_targetBytes = 4800, g_ringBytes = 32768;

// Runtime (ABI 8 audio block, all optional).
void* g_rt = nullptr;
SvAudioConfigureFn g_configure = nullptr;
SvAudioSetPausedFn g_setPaused = nullptr;
SvAudioEmittersFn g_emitters = nullptr;
SvAudioPullFn g_pull = nullptr;
SvAudioGetStatusFn g_status = nullptr;
bool g_runtimeAudio = false; // configured and pullable

std::atomic<bool> g_probeOn{false}; // editor test tone on the board stream
std::atomic<bool> g_paused{false};
std::atomic<bool> g_pauseRequested{false};
std::atomic<std::uint64_t> g_scriptTickMs{0};

struct Slot {
    // feeder <-> game thread
    std::atomic<GtaRingBuffer*> ring{nullptr}; // ring of the live sound
    std::atomic<bool> merged{false};           // mixed into the board stream
    std::atomic<float> pcmGain{1.0f};          // AudioGtaGainDb as a factor
    Limiter limiter;                           // feeder only: the gain without clipping
    std::mutex infoLock;
    SvAudioEmitter info{};                     // latest from sv_audio_emitters / pull
    bool haveInfo = false;
    // feeder only
    float gain = 0.0f;
    double toneT = 0.0;
    // game thread only (soundRef is also written by GTA's audio engine)
    void* volatile soundRef = nullptr;
    void* group = nullptr;
    void* sound = nullptr; // what we created (for drop detection)
    bool live = false;
    bool listener = false;
    DWORD createdAt = 0, retryAt = 0, lastWanted = 0, lastSounding = 0;
    int creations = 0, failures = 0;
};
Slot g_slots[kSlots];
int g_drops = 0;

HANDLE g_feeder = nullptr;
std::atomic<bool> g_run{false};

void Record(int slot, const float* mix, int frames);
void EditorPlay(float* out, int frames);
std::atomic<bool> g_editorPlay{false}; // the board stream plays the clip at the editor's playhead

// ---- runtime --------------------------------------------------------------
SvAudioEmitter Info(int i, bool* have = nullptr) {
    std::lock_guard<std::mutex> lock(g_slots[i].infoLock);
    if (have) *have = g_slots[i].haveInfo;
    return g_slots[i].info;
}

// Adds `frames` of slot i's source into `mix` with its gain (feeder thread).
void AddSource(int i, float* mix, int frames) {
    // Includes the editor tone. Keep its cursor frozen while silent.
    if (g_paused.load(std::memory_order_relaxed)) return;
    Slot& s = g_slots[i];
    static thread_local float buf[kChunk];
    if (i == SV_AUDIO_EMITTER_BOARD && g_editorPlay.load()) {
        EditorPlay(buf, frames);
        for (int k = 0; k < frames; ++k) mix[k] += buf[k];
        if (!g_probeOn.load()) return;
    }
    if (i == SV_AUDIO_EMITTER_BOARD && g_probeOn.load()) {
        ProbeTone(buf, frames, s.toneT, kRate, 0.35f);
        for (int k = 0; k < frames; ++k) mix[k] += buf[k];
        return;
    }
    if (!g_runtimeAudio) return;
    SvAudioEmitter info{};
    info.size = sizeof(info);
    const std::uint32_t got = g_pull(g_rt, static_cast<std::uint32_t>(i), buf, static_cast<std::uint32_t>(frames), &info);
    float target = s.gain;
    if (got > 0) {
        target = std::isfinite(info.gain) ? std::clamp(info.gain, 0.0f, 4.0f) : 0.0f;
        std::lock_guard<std::mutex> lock(s.infoLock);
        s.info = info;
        s.haveInfo = true;
    }
    const float step = (target - s.gain) / static_cast<float>(frames);
    float g = s.gain;
    for (int k = 0; k < frames; ++k, g += step) mix[k] += buf[k] * g;
    s.gain = target;
}

// Tops up every live ring; returns whether any ring is live.
bool FeedGta() {
    static float mix[kChunk];
    static std::int16_t pcm[kChunk];
    bool any = false;
    for (int i = 0; i < kSlots; ++i) {
        Slot& s = g_slots[i];
        GtaRingBuffer* r = s.ring.load(std::memory_order_acquire);
        if (!r) continue;
        any = true;
        const std::int32_t avail = RingAvailable(r);
        const std::int32_t want = static_cast<std::int32_t>(g_targetBytes) - std::max(avail, 0);
        int frames = std::min(want / 2, kChunk);
        if (frames < 64) continue;
        std::fill(mix, mix + frames, 0.0f);
        AddSource(i, mix, frames);
        if (i == SV_AUDIO_EMITTER_BOARD) {
            for (int j = 1; j < kSlots; ++j)
                if (g_slots[j].merged.load()) AddSource(j, mix, frames);
        }
        Record(i, mix, frames); // before the gain: the editor's playback applies it again
        s.limiter.Process(mix, static_cast<std::size_t>(frames), s.pcmGain.load());
        FloatToPcm16(mix, pcm, frames, 1.0f, 1.0f);
        RingPush(r, pcm, static_cast<std::uint32_t>(frames * 2));
    }
    return any;
}

DWORD WINAPI FeederMain(void*) {
    HANDLE timer = CreateWaitableTimerExW(nullptr, nullptr, 0x00000002 /*CREATE_WAITABLE_TIMER_HIGH_RESOLUTION*/,
                                          TIMER_ALL_ACCESS);
    bool live = false;
    while (g_run.load()) {
        // 3 ms while a ring is live; with none, 20 ms still refreshes the
        // emitter state well within a frame of the script thread reading it.
        const LONGLONG waitMs = live ? 3 : 20;
        if (timer) {
            LARGE_INTEGER due;
            due.QuadPart = -waitMs * 10000;
            SetWaitableTimer(timer, &due, 0, nullptr, nullptr, FALSE);
            WaitForSingleObject(timer, 50);
        } else {
            Sleep(live ? 2 : 20);
        }
        const bool paused = AudioShouldPause(GetTickCount64(), g_scriptTickMs.load(), g_pauseRequested.load());
        if (g_paused.exchange(paused) != paused) {
            // The feeder is the sole runtime pause writer. Native GTA calls
            // remain on the script thread; this watchdog uses host time only.
            if (g_setPaused && g_rt && g_runtimeAudio) g_setPaused(g_rt, paused ? 1u : 0u);
            for (Slot& s : g_slots) RingSilence(s.ring.load(std::memory_order_acquire));
        }
        if (g_runtimeAudio) {
            SvAudioEmitter infos[kSlots]{};
            for (auto& e : infos) e.size = sizeof(e);
            const std::uint32_t n = std::min<std::uint32_t>(g_emitters(g_rt, infos, kSlots), kSlots);
            for (std::uint32_t i = 0; i < n; ++i) {
                std::lock_guard<std::mutex> lock(g_slots[i].infoLock);
                // Keep the position of the newest pull when it is newer.
                if (!g_slots[i].haveInfo || infos[i].frame >= g_slots[i].info.frame) g_slots[i].info = infos[i];
                else g_slots[i].info.flags = infos[i].flags, g_slots[i].info.queued_frames = infos[i].queued_frames;
                g_slots[i].haveInfo = true;
            }
        }
        live = g_path.load() == Path::Gta && FeedGta();
    }
    if (timer) CloseHandle(timer);
    return 0;
}

// ---- GTA path (script thread) ---------------------------------------------
// Plain functions without C++ unwinding, so GTA calls can sit in __try.
const char* volatile g_step = "";

struct CreateArgs {
    int slot;
    bool listener;
    float pos[4];
    void* ped; // CPed* (type checked) or null
    GtaRingBuffer* ring;
};

int CreateRaw(CreateArgs* a, void** groupOut, void* volatile* ref) {
    alignas(16) std::uint8_t params[gtare::kParamsSize];
    std::memset(params, 0, sizeof(params));
    g_step = "audSoundInitParams ctor";
    g_re.initParamsCtor(params);
    params[gtare::kParamsBucket] = *g_re.initParamsBucket;
    void* group = nullptr;
    if (a->listener) {
        *reinterpret_cast<std::int16_t*>(params + gtare::kParamsPan) = 0; // non-positional, as Bink's screen movies
    } else {
        alignas(16) float pos[4] = {a->pos[0], a->pos[1], a->pos[2], 0.0f};
        std::memcpy(params, pos, 16);
        if (g_set.tracker && a->ped) {
            g_step = "entity tracker";
            *reinterpret_cast<void**>(params + gtare::kParamsTracker) = g_re.entityTracker(a->ped);
        }
        g_step = "naEnvironmentGroup::Create";
        group = g_re.envCreate("SkateV");
        if (group) {
            g_step = "naEnvironmentGroup::Init";
            g_re.envInit(group, nullptr, g_re.envArgDistance, 0, 4000, g_re.envArgScale, 1000);
            g_step = "naEnvironmentGroup::SetPosition";
            g_re.envSetPosition(group, pos);
            if (a->ped) {
                g_step = "naEnvironmentGroup interior from ped";
                g_re.envSetInteriorFromEntity(group, a->ped);
            }
            *reinterpret_cast<void**>(params + gtare::kParamsEnvGroup) = group;
        }
    }
    if (g_category) *reinterpret_cast<void**>(params + gtare::kParamsCategory) = g_category;
    *groupOut = group;
    *ref = nullptr;
    g_step = "CreateSound_PersistentReference";
    g_re.createSoundByName(g_re.frontendEntity, "BINK_MONO_SOUND", const_cast<void**>(ref), params);
    void* sound = *ref;
    if (!sound) return 1;
    g_step = "InitStreamPlayer";
    if (!g_re.initStreamPlayer(sound, a->ring, 1, kRate)) {
        g_step = "StopAndForget (InitStreamPlayer failed)";
        g_re.stopAndForget(sound, false);
        *ref = nullptr;
        return 2;
    }
    if (!a->listener && !g_set.tracker) {
        void* settings = gtare::RequestedSettings(g_re, sound);
        if (settings) {
            alignas(16) float pos[4] = {a->pos[0], a->pos[1], a->pos[2], 0.0f};
            g_step = "audRequestedSettings::SetPosition";
            g_re.requestedSetPosition(settings, pos);
        }
    }
    g_step = "PrepareAndPlay";
    g_re.prepareAndPlay(sound, nullptr, false, 0, 0);
    return 0;
}

int CreateGuarded(CreateArgs* a, void** groupOut, void* volatile* ref) {
    __try {
        return CreateRaw(a, groupOut, ref);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return -static_cast<int>(GetExceptionCode() & 0x7FFFFFFF) - 1;
    }
}

int StopGuarded(void* sound) {
    __try {
        g_step = "StopAndForget";
        g_re.stopAndForget(sound, false);
        return 0;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return -1;
    }
}

// Room state of a positional stream's environment group after this frame's
// update: inside an interior, and the room reverb sends GTA left (cleared when
// the ped is back outside).
struct RoomState {
    bool inside = false;
    bool reset = false;
    float sends[3] = {};
};

// SetInteriorSettings (+0x4A8164) copies an interior room's reverb sends into
// the group; GTA's update feeds them to the sound every frame on the path our
// Init arguments select (Bink's), and nothing clears them when the location
// becomes invalid. GTA's own entity-backed groups null the interior and room
// on that edge (update +0x4CF2F8); a fresh group (Init) has zero sends. Do the
// same here so a stream does not keep a metro's reverb outside.
void UpdateRoom(std::uint8_t* g, RoomState* room) {
    room->inside = (g[gtare::kEnvFlags] & gtare::kEnvFlagInterior) != 0;
    if (room->inside) return;
    float* sends = reinterpret_cast<float*>(g + gtare::kEnvRoomReverb);
    if (sends[0] == 0.0f && sends[1] == 0.0f && sends[2] == 0.0f) return;
    std::memcpy(room->sends, sends, sizeof(room->sends));
    room->reset = true;
    sends[0] = sends[1] = sends[2] = 0.0f;
    *reinterpret_cast<void**>(g + gtare::kEnvInterior) = nullptr;
    *reinterpret_cast<void**>(g + gtare::kEnvRoom) = nullptr;
}

int UpdateGuarded(void* sound, void* group, const float* pos16, void* ped, bool tracker, RoomState* room) {
    __try {
        if (!tracker) {
            void* settings = gtare::RequestedSettings(g_re, sound);
            g_step = "audRequestedSettings::SetPosition";
            if (settings) g_re.requestedSetPosition(settings, pos16);
        }
        if (group) {
            g_step = "naEnvironmentGroup::SetPosition";
            g_re.envSetPosition(group, pos16);
            g_step = "naEnvironmentGroup interior from ped";
            if (ped) {
                g_re.envSetInteriorFromEntity(group, ped);
                if (g_re.envRoomLayoutOk) {
                    g_step = "naEnvironmentGroup room reset";
                    UpdateRoom(static_cast<std::uint8_t*>(group), room);
                }
            }
        }
        return 0;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return -1;
    }
}

void* LookupCategory(std::uint32_t hash) {
    __try {
        return g_re.getCategoryPtr(g_re.categoryManager, hash);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return nullptr;
    }
}

void* PlayerPedAddress(int& handle) {
    handle = Call<Ped>(gta::PLAYER_PED_ID);
    std::uint8_t* p = getScriptHandleBaseAddress(handle);
    if (!p || p[gtare::kEntityTypeOffset] != 4) return nullptr; // 4 = ped
    return p;
}

void Disable(const char* why);

void StopSlot(int i, const char* why) {
    Slot& s = g_slots[i];
    s.ring.store(nullptr, std::memory_order_release); // the ring itself is never freed
    void* sound = s.soundRef;
    if (sound && StopGuarded(sound) != 0) Logf("gta audio: %s: exception in StopAndForget", kSlotName[i]);
    if (s.live) Logf("gta audio: %s stream stopped (%s) after %.1f s", kSlotName[i], why, (GetTickCount() - s.createdAt) / 1000.0);
    s.live = false;
    s.sound = nullptr;
    s.group = nullptr;
    s.soundRef = nullptr;
}

bool CreateSlot(int i, const float pos[3], void* ped) {
    Slot& s = g_slots[i];
    const auto entityId = *reinterpret_cast<const std::uint16_t*>(static_cast<std::uint8_t*>(g_re.frontendEntity) +
                                                                  gtare::kEntityAudioId);
    if (entityId == 0xFFFF) {
        Logf("gta audio: %s: frontend audio entity not registered yet (id ffff); retry later", kSlotName[i]);
        return false;
    }
    CreateArgs a{};
    a.slot = i;
    a.listener = s.listener;
    std::memcpy(a.pos, pos, sizeof(float) * 3);
    a.ped = ped;
    a.ring = NewStreamRing(g_ringBytes, g_targetBytes);
    if (!a.ring) {
        Logf("gta audio: %s: out of memory for the ring buffer", kSlotName[i]);
        return false;
    }
    void* group = nullptr;
    const int rc = CreateGuarded(&a, &group, &s.soundRef);
    ++s.creations;
    if (rc < 0) {
        Logf("gta audio: %s: EXCEPTION 0x%08x during %s; GTA path abandoned", kSlotName[i],
             static_cast<unsigned>(-(rc + 1)) | 0x80000000u, g_step);
        Disable("exception in GTA audio call");
        return false;
    }
    if (rc != 0) {
        Logf("gta audio: %s: %s (creation %d, env group %p)", kSlotName[i],
             rc == 1 ? "CreateSound returned no sound" : "InitStreamPlayer refused the ring", s.creations, group);
        return false;
    }
    s.sound = s.soundRef;
    s.group = group;
    s.live = true;
    s.createdAt = GetTickCount();
    s.failures = 0;
    void* settings = gtare::RequestedSettings(g_re, s.sound);
    Logf("gta audio: %s stream #%d live: sound %p, requested settings %p, ring %p (%u bytes, %u queued), env group %p, "
         "%s, entity id %04x, category %p",
         kSlotName[i], s.creations, s.sound, settings, a.ring, g_ringBytes,
         static_cast<unsigned>(RingAvailable(a.ring)), group,
         s.listener ? "non-positional (Pan 0)" : g_set.tracker ? "positional, tracks the player ped" : "positional",
         entityId, g_category);
    s.ring.store(a.ring, std::memory_order_release);
    return true;
}

void StopAllGta(const char* why) {
    for (int i = 0; i < kSlots; ++i) {
        if (g_slots[i].live || g_slots[i].soundRef) StopSlot(i, why);
        g_slots[i].merged = false;
    }
}

void Disable(const char* why) {
    if (g_path.load() == Path::Off) return;
    Logf("gta audio: Skate audio off (silent): %s", why);
    StopAllGta("disabled");
    g_path = Path::Off;
}

// ---- Rockstar Editor (no script frames) -------------------------------------
// The editor runs no script frames, so Tick never runs there. GTA still updates its audio entities on the main
// thread: audio_entity_hook.h wraps the frontend entity's virtuals, Tick
// finds the one called once per frame on the main thread (the tick slot), and
// EditorTick runs from it while script frames are stopped. AudioEditorTone
// (default on): the probe beeps through a GTA stream there, to hear whether
// GTA plays, and the editor exports, our sound.
constexpr int kHookFirst = 6, kHookLast = 26; // vtable +0x1a3d090; slots 0..5 are class-id checks
DWORD g_mainThread = 0;
std::atomic<int> g_tickSlot{-1};
std::atomic<std::uint32_t> g_calls[kHookLast + 1][2]{};  // [slot][on the main thread]
std::atomic<std::uint64_t> g_lastScriptMs{0};            // last script frame (Tick)
std::atomic<std::uint64_t> g_lastCallLog{0};
bool g_editor = false;                                   // main thread only

// Calls per `per` (frames or seconds) since the last drain: " slot:main/other".
std::string DrainCalls(double per, int* oncePerFrame = nullptr) {
    std::string out;
    double best = 0.05;
    for (int i = kHookFirst; i <= kHookLast; ++i) {
        const double m = g_calls[i][1].exchange(0) / per, o = g_calls[i][0].exchange(0) / per;
        if (m == 0 && o == 0) continue;
        char b[40];
        std::snprintf(b, sizeof(b), " %d:%.2f/%.2f", i, m, o);
        out += b;
        if (oncePerFrame && std::fabs(m - 1.0) < best) best = std::fabs(m - 1.0), *oncePerFrame = i;
    }
    return out.empty() ? " none" : out;
}

// ---- Skate's sound in the editor --------------------------------------------
// While skating, every stream's PCM goes into a game-time ring (the last 3 min).
// When GTA writes a clip (Documents\Rockstar Games\GTA V\videos\clips\*.clip;
// its game-time range at +0x170 / +0x174), that range is saved to
// %LOCALAPPDATA%\SkateV\editor-audio\<clip>.sk8a; files whose clip was deleted
// are removed at start. In the editor the board stream plays the clip holding
// the playhead, GTA5.exe+0x1f62d9c (recorded game time,
// evidence/2026-10-05/editor-audio.md), through GTA, so the export has it.
constexpr std::uintptr_t kPlayhead = 0x1f62d9c;
constexpr int kRecordSeconds = 180;
struct ClipHeader {
    char magic[4];
    std::uint32_t rate, startMs, endMs, savedUnix;
};
struct ClipAudio {
    std::wstring file;
    ClipHeader h{};
    std::vector<std::int16_t> pcm; // loaded on first play, then kept (the feeder may be reading it)
};
GameTimeRing* g_record = nullptr;
std::atomic<bool> g_recording{false};
std::atomic<std::uint32_t> g_gameMs{0}; // GET_GAME_TIMER at the last script frame
std::atomic<std::uint64_t> g_gameAt{0}; // GetTickCount64 then
std::int64_t g_recCursor[kSlots] = {-1, -1, -1}; // feeder only
std::wstring g_clipsDir, g_editorDir;
FILETIME g_sessionStart{}, g_clipsStamp{};
std::set<std::wstring> g_clipsDone;
bool g_clipsPending = false;
std::vector<std::unique_ptr<ClipAudio>> g_clipIndex; // main thread
std::atomic<ClipAudio*> g_playing{nullptr};
struct Playhead {
    double ms = 0.0, rate = 0.0, at = 0.0;
};
std::mutex g_playheadLock;
Playhead g_playhead;
PlayheadFollower g_follower; // feeder only

double NowMs() {
    static const double perMs = [] {
        LARGE_INTEGER f;
        QueryPerformanceFrequency(&f);
        return f.QuadPart / 1000.0;
    }();
    LARGE_INTEGER c;
    QueryPerformanceCounter(&c);
    return c.QuadPart / perMs;
}

// Feeder: a stream's PCM at the game time it plays.
void Record(int slot, const float* mix, int frames) {
    if (!g_record || !g_recording.load(std::memory_order_relaxed)) return;
    const std::uint64_t late = std::min<std::uint64_t>(GetTickCount64() - g_gameAt.load(), 100);
    const std::int64_t now = (static_cast<std::int64_t>(g_gameMs.load()) + static_cast<std::int64_t>(late)) * kRate / 1000;
    std::int64_t& c = g_recCursor[slot];
    if (c < 0 || std::llabs(c - now) > kRate / 20) c = now; // contiguous while within 50 ms of game time
    g_record->Add(c, mix, frames);
    c += frames;
}

bool WriteAll(const std::wstring& path, const void* a, DWORD an, const void* b, DWORD bn) {
    HANDLE f = CreateFileW(path.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, 0, nullptr);
    if (f == INVALID_HANDLE_VALUE) return false;
    DWORD w1 = 0, w2 = 0;
    const bool ok = WriteFile(f, a, an, &w1, nullptr) && WriteFile(f, b, bn, &w2, nullptr) && w1 == an && w2 == bn;
    CloseHandle(f);
    if (!ok) DeleteFileW(path.c_str());
    return ok;
}

// Reads `n` bytes at `at`; false if the file is shorter or cannot be opened.
bool ReadAt(const std::wstring& path, LONGLONG at, void* out, DWORD n) {
    HANDLE f = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
                           OPEN_EXISTING, 0, nullptr);
    if (f == INVALID_HANDLE_VALUE) return false;
    LARGE_INTEGER pos;
    pos.QuadPart = at;
    DWORD got = 0;
    const bool ok = SetFilePointerEx(f, pos, nullptr, FILE_BEGIN) && ReadFile(f, out, n, &got, nullptr) && got == n;
    CloseHandle(f);
    return ok;
}

std::wstring Stem(const std::wstring& name) { return name.substr(0, name.find_last_of(L'.')); }

void SaveClip(const std::wstring& name) {
    std::uint8_t h[0x180];
    if (!ReadAt(g_clipsDir + name, 0, h, sizeof(h))) return; // still being written: next scan
    std::uint32_t saved, start, end;
    std::memcpy(&saved, h + 0x168, 4);
    std::memcpy(&start, h + 0x170, 4);
    std::memcpy(&end, h + 0x174, 4);
    if (end <= start || end - start > 600000) return;
    g_clipsDone.insert(name);
    std::vector<std::int16_t> pcm(static_cast<std::size_t>(end - start) * kRate / 1000);
    if (!g_record->Read(static_cast<std::int64_t>(start) * kRate / 1000, pcm.data(), static_cast<std::int64_t>(pcm.size())))
        return; // no Skate sound in it
    const ClipHeader ch{{'S', 'K', '8', 'A'}, kRate, start, end, saved};
    const bool ok = WriteAll(g_editorDir + Stem(name) + L".sk8a", &ch, sizeof(ch), pcm.data(),
                             static_cast<DWORD>(pcm.size() * sizeof(std::int16_t)));
    Logf("editor audio: %s Skate's sound for %ls (game time %u..%u ms)", ok ? "saved" : "could not save", name.c_str(),
         start, end);
}

// This session's new clips. The script thread, or the main thread on entering the editor (never both at once).
void ScanClips(bool force) {
    if (!g_record || g_clipsDir.empty()) return;
    WIN32_FILE_ATTRIBUTE_DATA d{};
    if (!GetFileAttributesExW(g_clipsDir.c_str(), GetFileExInfoStandard, &d)) return;
    if (!force && !g_clipsPending && CompareFileTime(&d.ftLastWriteTime, &g_clipsStamp) == 0) return;
    g_clipsStamp = d.ftLastWriteTime;
    g_clipsPending = false;
    WIN32_FIND_DATAW fd;
    HANDLE find = FindFirstFileW((g_clipsDir + L"*.clip").c_str(), &fd);
    if (find == INVALID_HANDLE_VALUE) return;
    do {
        if (CompareFileTime(&fd.ftLastWriteTime, &g_sessionStart) < 0 || g_clipsDone.count(fd.cFileName)) continue;
        SaveClip(fd.cFileName);
        if (!g_clipsDone.count(fd.cFileName)) g_clipsPending = true;
    } while (FindNextFileW(find, &fd));
    FindClose(find);
}

// Saved clip sound whose clip is gone (deleted in the editor or by hand).
void PruneEditorAudio() {
    WIN32_FIND_DATAW fd;
    HANDLE find = FindFirstFileW((g_editorDir + L"*.sk8a").c_str(), &fd);
    if (find == INVALID_HANDLE_VALUE) return;
    int removed = 0;
    do {
        if (GetFileAttributesW((g_clipsDir + Stem(fd.cFileName) + L".clip").c_str()) == INVALID_FILE_ATTRIBUTES &&
            DeleteFileW((g_editorDir + fd.cFileName).c_str()))
            ++removed;
    } while (FindNextFileW(find, &fd));
    FindClose(find);
    if (removed) Logf("editor audio: removed %d saved clip sounds whose clip is gone", removed);
}

// Adds the saved clip sounds not indexed yet (entries are never removed: the feeder may hold one).
void IndexEditorAudio() {
    WIN32_FIND_DATAW fd;
    HANDLE find = FindFirstFileW((g_editorDir + L"*.sk8a").c_str(), &fd);
    if (find == INVALID_HANDLE_VALUE) return;
    do {
        const std::wstring file = g_editorDir + fd.cFileName;
        if (std::any_of(g_clipIndex.begin(), g_clipIndex.end(), [&](const auto& c) { return c->file == file; })) continue;
        auto c = std::make_unique<ClipAudio>();
        c->file = file;
        if (ReadAt(file, 0, &c->h, sizeof(c->h)) && std::memcmp(c->h.magic, "SK8A", 4) == 0 && c->h.rate == kRate)
            g_clipIndex.push_back(std::move(c));
    } while (FindNextFileW(find, &fd));
    FindClose(find);
}

// Main thread, every editor frame: the playhead's position and rate, and the clip that holds it.
void TrackPlayhead() {
    static const auto* playhead = reinterpret_cast<const volatile std::uint32_t*>(
        reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr)) + kPlayhead);
    static PlayheadClock clock;
    const std::uint32_t p = *playhead;
    clock.Update(NowMs(), p);
    {
        std::lock_guard<std::mutex> lock(g_playheadLock);
        g_playhead = {clock.ms, clock.rate, clock.at};
    }
    ClipAudio* cur = g_playing.load();
    if (cur && p >= cur->h.startMs && p < cur->h.endMs) return;
    // ponytail: game times restart each launch, so two sessions' clips can overlap; the newest wins.
    ClipAudio* best = nullptr;
    for (const auto& c : g_clipIndex)
        if (p >= c->h.startMs && p < c->h.endMs && (!best || c->h.savedUnix > best->h.savedUnix)) best = c.get();
    if (best && best->pcm.empty()) {
        std::vector<std::int16_t> pcm(static_cast<std::size_t>(best->h.endMs - best->h.startMs) * kRate / 1000);
        if (!ReadAt(best->file, sizeof(ClipHeader), pcm.data(), static_cast<DWORD>(pcm.size() * sizeof(std::int16_t)))) {
            Logf("editor audio: could not read %ls", best->file.c_str());
            best = nullptr;
        } else {
            best->pcm = std::move(pcm);
        }
    }
    if (best == cur) return;
    g_playing = best;
    if (best) Logf("editor audio: playhead %u ms: playing %ls", p, best->file.c_str());
    else Logf("editor audio: playhead %u ms: no Skate sound saved for this part", p);
}

// Feeder: the playing clip at the playhead.
void EditorPlay(float* out, int frames) {
    ClipAudio* c = g_playing.load();
    Playhead ph;
    {
        std::lock_guard<std::mutex> lock(g_playheadLock);
        ph = g_playhead;
    }
    if (!c) {
        std::fill(out, out + frames, 0.0f);
        g_follower.pos = -1.0;
        return;
    }
    const double ms = ph.ms + ph.rate * (NowMs() - ph.at);
    g_follower.Fill(c->pcm.data(), static_cast<std::int64_t>(c->pcm.size()), (ms - c->h.startMs) * kRate / 1000.0,
                    ph.rate, kRate, 0.15 * kRate, out, frames);
}

void EditorTick() {
    if (g_path.load() != Path::Gta) return;
    const DWORD now = GetTickCount();
    g_scriptTickMs = GetTickCount64(); // the feeder pauses without a heartbeat
    g_pauseRequested = false;
    Slot& s = g_slots[0];
    if (!g_editor) {
        g_editor = true;
        g_recording = false;
        ScanClips(true); // a clip written in the last seconds of play
        IndexEditorAudio();
        g_probeOn = g_set.editorTone;
        g_editorPlay = true;
        Logf("gta audio: EDITOR: no script frames; Skate's clip sound%s through a GTA stream, created from the audio "
             "entity's per-frame call (%zu clips with Skate sound)",
             g_set.editorTone ? " and 440 Hz beeps" : "", g_clipIndex.size());
        if (s.live) StopSlot(0, "editor");
        s.listener = true;
        s.retryAt = 0;
    }
    TrackPlayhead();
    if (s.live && !s.soundRef) {
        Logf("gta audio: EDITOR: GTA dropped the editor stream after %.1f s", (now - s.createdAt) / 1000.0);
        s.ring.store(nullptr);
        s.live = false;
        s.retryAt = now + 500;
    }
    static const float kOrigin[3] = {};
    if (!s.live && now >= s.retryAt && !CreateSlot(0, kOrigin, nullptr)) s.retryAt = now + 2000;
}

// Runs before every wrapped virtual of the frontend entity, on any thread.
void OnEntityCall(void*, int slot) {
    const bool main = GetCurrentThreadId() == g_mainThread;
    g_calls[slot][main ? 1 : 0].fetch_add(1, std::memory_order_relaxed);
    const std::uint64_t now = GetTickCount64();
    if (now - g_lastScriptMs.load() < 1000) return;
    std::uint64_t last = g_lastCallLog.load();
    if (now - last >= 5000 && g_lastCallLog.compare_exchange_strong(last, now) && util::g_verbose)
        Logf("gta audio: no script frames: audio entity calls per s (slot:main/other):%s, tick slot %d",
             DrainCalls((now - last) / 1000.0).c_str(), g_tickSlot.load());
    if (main && slot == g_tickSlot.load()) EditorTick();
}

// Script thread, every frame: measures the tick slot over the first 5 s
// windows, then keeps the counters drained for the editor's log.
void MeasureCalls(std::uint64_t now) {
    static std::uint64_t windowStart = 0;
    static int frames = 0, windows = 0;
    g_lastCallLog = now;
    if (windows >= 3) {
        DrainCalls(1.0);
        return;
    }
    if (!windowStart) {
        windowStart = now;
        DrainCalls(1.0);
        return;
    }
    ++frames;
    if (now - windowStart < 5000) return;
    int pick = -1;
    const std::string calls = DrainCalls(frames, &pick);
    if (pick >= 0 && g_tickSlot.load() < 0) g_tickSlot = pick;
    if (util::g_verbose) Logf("gta audio: audio entity calls per frame over %d frames (slot:main/other):%s; tick slot %d", frames,
         calls.c_str(), g_tickSlot.load());
    ++windows;
    windowStart = now;
    frames = 0;
}

// ---- settings --------------------------------------------------------------
Settings LoadSettings(const std::wstring& ini) {
    Settings s;
    const std::string out = util::IniString(ini, L"AudioOutput");
    if (out == "Off" || out == "off" || out == "0") s.on = false;
    const std::string legacy = util::IniString(ini, L"Audio");
    if (!legacy.empty() && std::atof(legacy.c_str()) == 0.0) s.on = false;
    s.cache = util::IniString(ini, L"AudioCache");
    const std::string gain = util::IniString(ini, L"AudioMasterGain");
    if (!gain.empty()) s.masterGain = static_cast<float>(std::atof(gain.c_str()));
    s.tracker = util::IniString(ini, L"AudioGtaPlacement") == "Tracker";
    const std::string category = util::IniString(ini, L"AudioGtaCategory");
    if (category == "SOUND" || category == "Sound") s.category.clear();
    else if (!category.empty()) s.category = category;
    const std::string gainDb = util::IniString(ini, L"AudioGtaGainDb");
    if (!gainDb.empty()) s.gainDb = std::clamp(static_cast<float>(std::atof(gainDb.c_str())), -100.0f, 24.0f);
    const std::string ms = util::IniString(ini, L"AudioGtaBufferMs");
    if (!ms.empty()) s.bufferMs = std::clamp(std::atoi(ms.c_str()), 15, 250);
    s.editorTone = util::IniString(ini, L"AudioEditorTone") == "1";
    return s;
}

std::uint32_t g_lastState = 0xFFFFFFFF;
DWORD g_nextStatus = 0;

void PollStatus(DWORD now) {
    if (!g_status || !g_rt || now < g_nextStatus) return;
    g_nextStatus = now + 1000;
    SvAudioStatus st{};
    st.size = sizeof(st);
    if (!g_status(g_rt, &st)) return;
    if (st.state != g_lastState) {
        static const char* names[] = {"off", "loading", "ready", "error"};
        Logf("gta audio: runtime audio state %s (%u voices, %u underruns, %llu frames): %s",
             st.state < 4 ? names[st.state] : "?", st.voices, st.underruns, static_cast<unsigned long long>(st.frames),
             st.message_utf8);
        g_lastState = st.state;
    }
}

} // namespace

void SetGainDb(float db) {
    g_set.gainDb = std::clamp(db, -100.0f, 24.0f);
    const float factor = std::pow(10.0f, g_set.gainDb / 20.0f);
    for (Slot& s : g_slots) s.pcmGain.store(factor);
    Logf("gta audio: AudioGtaGainDb set to %+.1f dB in game (x%.2f)", g_set.gainDb, factor);
}

void Start(const std::wstring& iniPath, HMODULE runtimeModule, void* runtime, void (*log)(const char*)) {
    for (Slot& s : g_slots) s.limiter.Init(kRate);
    if (g_started) return;
    g_started = true;
    g_log = log;
    g_set = LoadSettings(iniPath);
    g_targetBytes = static_cast<std::uint32_t>(g_set.bufferMs * kRate / 1000) * 2;
    g_ringBytes = 32768;
    while (g_ringBytes < g_targetBytes * 4) g_ringBytes *= 2;
    Logf("gta audio: start: output %s, placement %s, buffer %d ms", g_set.on ? "GTA" : "off",
         g_set.tracker ? "Tracker" : "Position", g_set.bufferMs);
    for (int i = 0; i < kSlots; ++i) g_slots[i].listener = i == SV_AUDIO_EMITTER_SPEED;

    // Runtime audio (ABI 8, optional exports).
    g_rt = runtime;
    if (runtimeModule && runtime) {
        g_configure = reinterpret_cast<SvAudioConfigureFn>(GetProcAddress(runtimeModule, "sv_audio_configure"));
        g_setPaused = reinterpret_cast<SvAudioSetPausedFn>(GetProcAddress(runtimeModule, "sv_audio_set_paused"));
        g_emitters = reinterpret_cast<SvAudioEmittersFn>(GetProcAddress(runtimeModule, "sv_audio_emitters"));
        g_pull = reinterpret_cast<SvAudioPullFn>(GetProcAddress(runtimeModule, "sv_audio_pull"));
        g_status = reinterpret_cast<SvAudioGetStatusFn>(GetProcAddress(runtimeModule, "sv_audio_get_status"));
        if (g_set.on && g_configure && g_emitters && g_pull) {
            SvAudioConfig cfg{};
            cfg.size = sizeof(cfg);
            cfg.sample_rate = kRate;
            cfg.cache_dir_utf8 = g_set.cache.c_str();
            cfg.master_gain = g_set.masterGain;
            const std::uint32_t ok = g_set.cache.empty() ? 0u : g_configure(g_rt, &cfg);
            g_runtimeAudio = ok != 0;
            Logf("gta audio: sv_audio_configure(cache '%s', gain %.2f) -> %u%s", g_set.cache.c_str(), g_set.masterGain, ok,
                 g_set.cache.empty() ? " (AudioCache not set in SkateVLegacy.ini)" : "");
        }
    } else {
        Log("gta audio: no runtime");
    }
    g_mixerWaitFlag = gtare::MixerWaitTimedOutFlag(log);
    if (!g_set.on) {
        Log("gta audio: AudioOutput=Off");
        return;
    }
    if (!gtare::Resolve(g_re, log)) {
        Log("gta audio: GTA audio guards failed; Skate audio off (silent)");
        return;
    }
    g_path = Path::Gta;
    g_mainThread = GetCurrentThreadId();
    g_lastScriptMs = GetTickCount64();
    audiohook::Install(g_re.frontendEntity, kHookFirst, kHookLast, OnEntityCall, log);
    const float factor = std::pow(10.0f, g_set.gainDb / 20.0f);
    Logf("gta audio: AudioGtaGainDb %+.1f dB (x%.2f on the PCM fed to GTA)", g_set.gainDb, factor);
    for (Slot& s : g_slots) s.pcmGain.store(factor);
    if (g_re.categoriesOk && !g_set.category.empty()) {
        const std::string& n = g_set.category;
        const bool raw = n.size() > 2 && n[0] == '0' && (n[1] == 'x' || n[1] == 'X');
        const std::uint32_t hash = raw ? static_cast<std::uint32_t>(std::strtoul(n.c_str() + 2, nullptr, 16)) : util::Joaat(n);
        g_category = LookupCategory(hash);
        Logf("gta audio: AudioGtaCategory %s (hash %08x) -> %p%s", n.c_str(), hash, g_category,
             g_category ? "" : " (not found: sound default)");
    }
    {
        wchar_t buf[MAX_PATH]{}, docs[MAX_PATH]{};
        const DWORD n = GetEnvironmentVariableW(L"LOCALAPPDATA", buf, MAX_PATH);
        if (n && n < MAX_PATH && SUCCEEDED(SHGetFolderPathW(nullptr, CSIDL_PERSONAL, nullptr, 0, docs))) {
            g_editorDir = std::wstring(buf) + L"\\SkateV\\editor-audio\\";
            CreateDirectoryW(g_editorDir.c_str(), nullptr);
            g_clipsDir = std::wstring(docs) + L"\\Rockstar Games\\GTA V\\videos\\clips\\";
            GetSystemTimeAsFileTime(&g_sessionStart);
            g_record = new GameTimeRing(kRate, kRecordSeconds); // 17 MB, kept for the session
            PruneEditorAudio();
            Logf("gta audio: editor audio: keeping the last %d s of Skate sound for clips written to %ls", kRecordSeconds,
                 g_clipsDir.c_str());
        }
    }
    g_scriptTickMs = GetTickCount64();
    g_pauseRequested = false;
    g_paused = false;
    g_run = true;
    g_feeder = CreateThread(nullptr, 0, FeederMain, nullptr, 0, nullptr);
    if (g_feeder) SetThreadPriority(g_feeder, THREAD_PRIORITY_TIME_CRITICAL);
    Logf("gta audio: output path GTA, feeder thread %lu, runtime audio %s", g_feeder ? GetThreadId(g_feeder) : 0ul,
         g_runtimeAudio ? "yes" : "no");
}

void Tick(bool skating) {
    if (!g_started) return;
    // GTA clears it when its audio (re)initialises; see MixerWaitTimedOutFlag.
    if (g_mixerWaitFlag) *g_mixerWaitFlag = 1;
    g_scriptTickMs = GetTickCount64();
    g_lastScriptMs = g_scriptTickMs.load();
    const DWORD now = GetTickCount();
    if (g_path.load() == Path::Gta) MeasureCalls(g_lastScriptMs.load());
    g_gameMs = static_cast<std::uint32_t>(Call<int>(gta::GET_GAME_TIMER));
    g_gameAt = GetTickCount64();
    static DWORD nextScan = 0;
    if (now >= nextScan) nextScan = now + 2000, ScanClips(false);
    if (g_editor) {
        // Back from the editor: its streams were GTA's to drop (not counted as drops).
        g_editor = false;
        g_probeOn = false;
        g_editorPlay = false;
        g_playing = nullptr;
        for (int i = 0; i < kSlots; ++i)
            if (g_slots[i].live) StopSlot(i, "editor left");
        Log("gta audio: EDITOR: script frames back; normal streams resume");
    }
    g_recording = true;
    if (Call<BOOL>(gta::NETWORK_IS_SESSION_STARTED)) {
        g_pauseRequested = true;
        // Story Mode only: never create sounds in a network session.
        if (g_path.load() == Path::Gta) StopAllGta("network session");
        return;
    }
    g_pauseRequested = Call<BOOL>(gta::IS_PAUSE_MENU_ACTIVE) != 0;
    PollStatus(now);
    if (g_path.load() != Path::Gta) return;

    int pedHandle = 0;
    void* ped = PlayerPedAddress(pedHandle);
    const Vector3 pp = Call<Vector3>(gta::GET_ENTITY_COORDS, pedHandle, 1);
    float pos[kSlots][3];
    bool want[kSlots];
    for (int i = 0; i < kSlots; ++i) {
        Slot& s = g_slots[i];
        bool have = false;
        const SvAudioEmitter info = Info(i, &have);
        const bool haveInfo = have && info.frame > 0;
        pos[i][0] = haveInfo ? info.position.x : pp.x;
        pos[i][1] = haveInfo ? info.position.y : pp.y;
        pos[i][2] = haveInfo ? info.position.z : pp.z;
        if (have) s.listener = (info.flags & SV_AUDIO_EMITTER_LISTENER) != 0 || (info.flags == 0 && i == SV_AUDIO_EMITTER_SPEED);
        const bool sounding = g_runtimeAudio && (info.flags & SV_AUDIO_EMITTER_ACTIVE) &&
                              ((info.flags & SV_AUDIO_EMITTER_SOUNDING) || info.queued_frames > 0);
        if (sounding) s.lastSounding = now;
        want[i] = (skating && g_runtimeAudio) || (s.lastSounding && now - s.lastSounding < kLingerMs);
    }
    for (int i = 1; i < kSlots; ++i)
        if (g_slots[i].merged.load() && want[i]) want[0] = true;
    for (int i = 0; i < kSlots && g_path.load() == Path::Gta; ++i) {
        Slot& s = g_slots[i];
        if (s.live && s.soundRef == nullptr) {
            ++g_drops;
            Logf("gta audio: GTA dropped the %s stream after %.1f s (drop %d)", kSlotName[i],
                 (now - s.createdAt) / 1000.0, g_drops);
            s.ring.store(nullptr);
            s.live = false;
            s.retryAt = now + 500;
            if (g_drops >= kMaxDrops) {
                Disable("GTA keeps dropping the stream sounds");
                break;
            }
        }
        if (s.merged.load()) continue;
        if (want[i]) s.lastWanted = now;
        if (want[i] && !s.live && now >= s.retryAt) {
            if (!CreateSlot(i, pos[i], ped)) {
                if (g_path.load() != Path::Gta) break;
                ++s.failures;
                s.retryAt = now + 1000u * static_cast<DWORD>(s.failures);
                if (i == SV_AUDIO_EMITTER_BOARD && s.failures >= 3) {
                    Disable("the board stream could not be created 3 times");
                    break;
                }
                if (i != SV_AUDIO_EMITTER_BOARD && s.failures >= 2) {
                    s.merged = true;
                    Logf("gta audio: %s stream unavailable; mixing it into the board stream", kSlotName[i]);
                }
            }
        } else if (!want[i] && s.live && now - s.lastWanted > kLingerMs) {
            StopSlot(i, "quiet");
        }
        if (s.live && s.soundRef && !s.listener) {
            alignas(16) float p16[4] = {pos[i][0], pos[i][1], pos[i][2], 0.0f};
            RoomState room;
            if (UpdateGuarded(s.soundRef, s.group, p16, ped, g_set.tracker, &room) != 0) {
                Logf("gta audio: %s: EXCEPTION during %s", kSlotName[i], g_step);
                Disable("exception updating a GTA stream");
                break;
            }
            static bool s_inside[kSlots] = {};
            if (s.group && room.inside != s_inside[i])
                Logf("gta audio: %s environment group %s an interior at %.1f %.1f %.1f", kSlotName[i],
                     room.inside ? "entered" : "left", pos[i][0], pos[i][1], pos[i][2]);
            if (room.reset)
                Logf("gta audio: %s environment group outside: room reverb sends %.3f %.3f %.3f reset to 0",
                     kSlotName[i], room.sends[0], room.sends[1], room.sends[2]);
            s_inside[i] = room.inside;
        }
    }
}

void Stop() {
    if (!g_started) return;
    audiohook::Uninstall();
    g_run = false;
    if (g_feeder) {
        WaitForSingleObject(g_feeder, 500);
        CloseHandle(g_feeder);
        g_feeder = nullptr;
    }
    for (Slot& s : g_slots) s.ring.store(nullptr);
    g_started = false;
}

} // namespace gtaaudio

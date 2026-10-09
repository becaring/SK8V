#pragma once
#include <cstdint>

// GTA V Legacy 1.0.3889.0 audio engine entry points used by the GTA audio
// bridge (gta_audio.cpp). Every address was found by static analysis of the
// decrypted in-memory image (local/re/audio_bridge.py re-finds and
// cross-checks each one; evidence/2026-10-01/gta-audio-bridge.md). The game's
// own Bink movie audio (image +0xE0208C) is the precedent for every call: it
// plays an external stream sound ("BINK_MONO_SOUND"/"BINK_STEREO_SOUND") on
// the frontend audio entity, optionally positional with an environment group.
//
// Resolve() checks each signature at its expected RVA (falling back to a
// unique match in .text), then cross-checks the call targets inside the Bink
// function against the resolved functions. Nothing is called unless every
// required check passes.
namespace gtare {

struct Audio {
    // Functions (GTA x64 calling convention; see gta_audio_re.cpp).
    void(__fastcall* initParamsCtor)(void* params) = nullptr;
    void(__fastcall* createSoundByName)(void* entity, const char* name, void** reference, const void* params) = nullptr;
    // The name variant hashes the name and tail-jumps here (guarded below).
    void(__fastcall* createSoundByHash)(void* entity, std::uint32_t hash, void** reference, const void* params) = nullptr;
    bool(__fastcall* initStreamPlayer)(void* sound, void* ring, int channels, int rate) = nullptr;
    void(__fastcall* prepareAndPlay)(void* sound, void* waveSlot, bool allowLoad, int timeLimitMs, int flag) = nullptr;
    void(__fastcall* stopAndForget)(void* sound, bool flag) = nullptr;
    void*(__fastcall* envCreate)(const char* debugName) = nullptr;
    void(__fastcall* envInit)(void* group, void* entity, float a, int b, int c, float d, int e) = nullptr;
    void(__fastcall* envSetPosition)(void* group, const float* vec16) = nullptr;
    void(__fastcall* envSetInteriorFromEntity)(void* group, void* entity) = nullptr;
    void(__fastcall* requestedSetPosition)(void* settings, const float* vec16) = nullptr;
    void*(__fastcall* entityTracker)(void* entity) = nullptr;
    void*(__fastcall* getCategoryPtr)(void* manager, std::uint32_t hash) = nullptr; // optional
    // Globals.
    void* frontendEntity = nullptr;            // audEntity the Bink sounds are created on
    const std::uint8_t* initParamsBucket = nullptr; // byte copied to params +0x9A
    const std::uintptr_t* settingsPoolBase = nullptr;
    const std::uint32_t* settingsStride = nullptr;
    const std::uint32_t* settingsWriteIndex = nullptr;
    void* categoryManager = nullptr;           // optional
    // Bink's environment-group Init arguments read from the image (20.0, 0.5).
    float envArgDistance = 20.0f, envArgScale = 0.5f;
    bool categoriesOk = false;
    // Optional: the interior-settings layout below was verified (signatures +
    // call chain from SetInteriorLocationFromEntity), so the bridge may reset
    // a group's room reverb sends when its ped is back outside.
    bool envRoomLayoutOk = false;
};

// Logs every address and guard. Returns false when any required item failed.
bool Resolve(Audio& out, void (*log)(const char*));
// Same resolver against a complete owned mapped-image dump, for offline guards.
// It resolves pointers only and never calls game functions.
bool ResolveImage(Audio& out, void* image, void (*log)(const char*));

// GTA's audio update writes each sound's settings into one of four slots and,
// per sound, sleeps 1 ms while the audio engine still reads the slot it is
// about to write (+0x1305E4C). It decides from a snapshot taken once per update,
// so when the update runs two frames inside one engine beat (an uneven game
// frame) every sound sleeps: 100+ sounds stalled the game 100-250 ms at a time
// while skating (evidence/2026-10-08/audio-update-stall.md). The returned byte
// is the flag GTA sets after one of those waits times out, from then on skipping
// them; null when the site is not found.
std::uint8_t* MixerWaitTimedOutFlag(void (*log)(const char*));

// audSound -> its audRequestedSettings (null when the sound has none), the
// computation GTA inlines everywhere (sound +0x80 slot, +0x62 bucket).
void* RequestedSettings(const Audio& a, void* sound);

// Audio offsets inside GTA structures used directly (guarded by the signatures).
constexpr int kParamsSize = 0xC0;   // audSoundInitParams is 0xA0 used bytes; padded
constexpr int kParamsPan = 0x3A;    // s16, -1 = positional; Bink sets 0 for screen movies
constexpr int kParamsTracker = 0x48;
constexpr int kParamsCategory = 0x58;
constexpr int kParamsEnvGroup = 0x60;
constexpr int kParamsBucket = 0x9A;
constexpr int kEntityTypeOffset = 0x28; // CEntity type byte; 4 = ped
constexpr int kEntityAudioId = 0x08;    // audEntity id (u16), 0xFFFF = not registered
// naEnvironmentGroup interior fields (SetInteriorSettings, image +0x4A8164):
// room reverb sends (3 floats), interior and room pointers, interior flag.
// GTA's update consumes the sends when Init's u16 argument is 0 (as Bink's and
// ours are) and nothing resets them when the location becomes invalid.
constexpr int kEnvRoomReverb = 0xB4;
constexpr int kEnvInterior = 0xF8;
constexpr int kEnvRoom = 0x100;
constexpr int kEnvFlags = 0x4D;
constexpr std::uint8_t kEnvFlagInterior = 0x04;

} // namespace gtare

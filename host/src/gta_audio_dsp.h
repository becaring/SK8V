#pragma once
// Pure helpers of the GTA audio bridge (gta_audio.cpp), kept free of GTA so host/tests/gta_audio_tests.cpp can exercise them:
//  - the layout and writer of GTA's audReferencedRingBuffer,
//  - float -> int16 conversion with a gain ramp, the limiter,
//  - the probe tone (Rockstar Editor check), the game-time ring and the playhead follower (editor playback).
#include <windows.h>
#include <cstddef>
#include <cstdint>
#include <mutex>
#include <vector>

namespace gtaaudio {

// GTA 1.0.3889.0 rage::audReferencedRingBuffer (0x58 bytes). Layout from the
// game's own construction (Bink, image +0x126041A), writer (PushAudio
// +0x3EDABC) and reader (stream player +0x12E620C / +0x12E9638); see
// evidence/2026-10-01/gta-audio-bridge.md. int16 PCM, interleaved.
struct GtaRingBuffer {
    std::uint8_t* data;          // +0x00
    std::uint32_t size;          // +0x08 bytes
    std::uint32_t unused0C;      // +0x0C (left uninitialised by the game)
    std::uint32_t readOffset;    // +0x10 advanced by GTA's stream player
    std::uint32_t writeOffset;   // +0x14 advanced by the writer
    std::int32_t available;      // +0x18 bytes written and not yet read (negative after an underrun)
    std::uint32_t unused1C;      // +0x1C
    CRITICAL_SECTION lock;       // +0x20 (taken by both sides when useLock)
    std::uint8_t unused48;       // +0x48
    std::uint8_t useLock;        // +0x49
    std::uint8_t pad4A[6];
    volatile LONG references;    // +0x50 the last Release frees data and the ring (game allocator)
    std::uint32_t unused54;      // +0x54
};
static_assert(sizeof(CRITICAL_SECTION) == 40);
static_assert(offsetof(GtaRingBuffer, size) == 0x08);
static_assert(offsetof(GtaRingBuffer, readOffset) == 0x10);
static_assert(offsetof(GtaRingBuffer, writeOffset) == 0x14);
static_assert(offsetof(GtaRingBuffer, available) == 0x18);
static_assert(offsetof(GtaRingBuffer, lock) == 0x20);
static_assert(offsetof(GtaRingBuffer, useLock) == 0x49);
static_assert(offsetof(GtaRingBuffer, references) == 0x50);
static_assert(sizeof(GtaRingBuffer) == 0x58);

// A ring the host owns for good: VirtualAlloc'd (never the game's allocator)
// and holding one reference of its own that is never released, so GTA's
// Release can never free it. Starts with `prefillBytes` of silence queued.
// Returns null when out of memory. Never freed (a game thread may still read
// it after the sound is gone).
GtaRingBuffer* NewRing(std::uint32_t sizeBytes, std::uint32_t prefillBytes);
// Playback ring: satisfy the native stream player's one-time startup gate.
GtaRingBuffer* NewStreamRing(std::uint32_t sizeBytes, std::uint32_t targetBytes);

struct PushResult {
    std::uint32_t written = 0;  // bytes of `src` written
    std::uint32_t resynced = 0; // silence bytes written first to catch up after an underrun
};
// Writes up to `bytes` (whole only if it fits; never overwrites unread data).
// Like GTA's PushAudio, plus: after an underrun (available < 0) the write
// position first catches up with the reader by writing silence; and the
// already-consumed region after the new write position is zeroed, so a later
// underrun replays silence rather than stale audio.
PushResult RingPush(GtaRingBuffer* ring, const std::int16_t* src, std::uint32_t bytes);
// Bytes queued (may be negative after an underrun). Takes the lock.
std::int32_t RingAvailable(GtaRingBuffer* ring);
// Silence unread and stale PCM without moving GTA's independent reader cursor.
void RingSilence(GtaRingBuffer* ring);
// Script callbacks can stop entirely in GTA's pause menu. Monotonic host time.
bool AudioShouldPause(std::uint64_t nowMs, std::uint64_t scriptTickMs, bool requested);

// float [-1, 1] -> int16 with a linear gain ramp from g0 to g1 across n; clamps.
void FloatToPcm16(const float* in, std::int16_t* out, std::size_t n, float g0, float g1);

// Probe tone: 440 Hz beeps (0.4 s on, 0.2 s off) at `amplitude`, phase kept in `t`.
void ProbeTone(float* out, std::size_t n, double& t, int rate, float amplitude);

// Skate's sound by game time, for the Rockstar Editor: the feeder adds every
// stream's PCM at the game time it plays (sample index = GET_GAME_TIMER ms *
// rate / 1000); a saved clip's range is cut out of it. Holds the last
// `seconds`; anything older, or never written, reads as silence.
class GameTimeRing {
public:
    GameTimeRing(int rate, int seconds);
    void Add(std::int64_t at, const float* pcm, int n);
    // [at, at + n) into out; returns whether any of it is not silence.
    bool Read(std::int64_t at, std::int16_t* out, std::int64_t n);

private:
    std::int64_t Block(std::int64_t at) const { return at / block_; }
    int block_;
    std::vector<std::int16_t> samples_;
    std::vector<std::int64_t> tags_; // absolute block index each ring block holds
    std::mutex lock_;
};

// Plays a clip's samples at the Rockstar Editor's playhead. `target` is the
// clip sample the playhead is at now, `rate` playhead seconds per real second
// (0 paused, < 1 slow motion). Follows the rate and eases out drift over a
// second; jumps when more than `jump` samples off (a scrub); silent while the
// playhead holds still or is outside the clip.
// The editor's playhead (recorded game time, ms), read every frame. It moves
// in steps some frames apart, so its speed is measured across its changes in
// the last 300 ms: per frame, a step read as up to 6x and was clamped, which
// read 1.2x playback as 0.5x (the clip sound fell behind, jumped, repeated).
// Still for 150 ms: paused. A step back or over a second ahead: a scrub.
// The playhead now is ms + rate * (now - at).
struct PlayheadClock {
    double ms = 0.0, rate = 0.0, at = 0.0;
    void Update(double now, std::uint32_t p);

private:
    struct Change {
        double t;
        std::uint32_t p;
    };
    Change changes[64]{};
    int count = 0; // oldest first
};

struct PlayheadFollower {
    double pos = -1.0;
    void Fill(const std::int16_t* clip, std::int64_t length, double target, double rate, int sampleRate, double jump,
              float* out, int n);
};

// Peak limiter for the PCM fed to GTA: applies `gain`, and wherever a
// sample would exceed `ceiling` the gain drops at once to meet it, recovering
// with a `releaseSeconds` time constant. Hard clipping is what it replaces
// (the stream's peaks already reach full scale before GTA's ~18 dB loss).
struct Limiter {
    float envelope = 1.0f; // current extra gain factor (<= 1)
    float ceiling = 0.891f; // -1 dBFS
    float release = 0.0f;   // per-sample recovery coefficient
    void Init(double sampleRate, double releaseSeconds = 0.1, float ceilingLinear = 0.891f);
    void Process(float* samples, std::size_t n, float gain);
};

} // namespace gtaaudio

#include "gta_audio_dsp.h"

#include <algorithm>
#include <cmath>
#include <cstring>

namespace gtaaudio {
namespace {

void CopyIn(GtaRingBuffer* r, const std::uint8_t* src, std::uint32_t bytes) {
    // Write at writeOffset, wrapping at size (as PushAudio does).
    const std::uint32_t first = std::min(bytes, r->size - r->writeOffset);
    if (src) {
        std::memcpy(r->data + r->writeOffset, src, first);
        std::memcpy(r->data, src + first, bytes - first);
    } else {
        std::memset(r->data + r->writeOffset, 0, first);
        std::memset(r->data, 0, bytes - first);
    }
    r->writeOffset = (r->writeOffset + bytes) % r->size;
    r->available += static_cast<std::int32_t>(bytes);
}

void ZeroAhead(GtaRingBuffer* r) {
    // Everything after the write position up to the reader has been played.
    const std::int32_t free = static_cast<std::int32_t>(r->size) - std::max(r->available, 0);
    if (free <= 0) return;
    const std::uint32_t n = static_cast<std::uint32_t>(free);
    const std::uint32_t first = std::min(n, r->size - r->writeOffset);
    std::memset(r->data + r->writeOffset, 0, first);
    std::memset(r->data, 0, n - first);
}

struct Lock {
    GtaRingBuffer* r;
    explicit Lock(GtaRingBuffer* ring) : r(ring) {
        if (r->useLock) EnterCriticalSection(&r->lock);
    }
    ~Lock() {
        if (r->useLock) LeaveCriticalSection(&r->lock);
    }
};

} // namespace

GtaRingBuffer* NewRing(std::uint32_t sizeBytes, std::uint32_t prefillBytes) {
    sizeBytes &= ~3u;
    if (sizeBytes < 64) return nullptr;
    const std::size_t total = sizeof(GtaRingBuffer) + 64 + sizeBytes;
    auto* mem = static_cast<std::uint8_t*>(VirtualAlloc(nullptr, total, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE));
    if (!mem) return nullptr; // VirtualAlloc memory is zeroed
    auto* r = reinterpret_cast<GtaRingBuffer*>(mem);
    r->data = mem + ((sizeof(GtaRingBuffer) + 63) & ~std::size_t{63});
    r->size = sizeBytes;
    InitializeCriticalSectionAndSpinCount(&r->lock, 1000); // as the game does
    r->useLock = 1;
    r->references = 1; // ours, never released
    prefillBytes = std::min(prefillBytes & ~1u, sizeBytes);
    r->writeOffset = prefillBytes % sizeBytes;
    r->available = static_cast<std::int32_t>(prefillBytes);
    return r;
}

GtaRingBuffer* NewStreamRing(std::uint32_t sizeBytes, std::uint32_t targetBytes) {
    // GTA 3889 +0x12EFDD2 gives the reader a view at ring+0x10.
    // Its +0x12EBE36 startup gate compares view+8 (available) against
    // half of ring.size. Below that threshold it never advances the cursor.
    // Prime once with silence; the feeder then settles to targetBytes.
    return NewRing(sizeBytes, std::max(targetBytes, (sizeBytes & ~3u) / 2));
}

PushResult RingPush(GtaRingBuffer* r, const std::int16_t* src, std::uint32_t bytes) {
    PushResult out;
    if (!r) return out;
    bytes &= ~1u;
    Lock lock(r);
    if (r->available < 0) {
        // The reader ran past the writer: catch up with silence.
        out.resynced = static_cast<std::uint32_t>(-r->available) & ~1u;
        if (out.resynced > r->size) {
            // Lost more than a whole ring: restart at the reader.
            r->writeOffset = r->readOffset;
            r->available = 0;
            out.resynced = 0;
        } else {
            CopyIn(r, nullptr, out.resynced);
            r->available = 0;
        }
    }
    const std::uint32_t free = r->size - static_cast<std::uint32_t>(r->available);
    const std::uint32_t n = std::min(bytes, free);
    if (n > 0) CopyIn(r, reinterpret_cast<const std::uint8_t*>(src), n);
    out.written = n;
    ZeroAhead(r);
    return out;
}

std::int32_t RingAvailable(GtaRingBuffer* r) {
    if (!r) return 0;
    Lock lock(r);
    return r->available;
}

void RingSilence(GtaRingBuffer* r) {
    if (!r) return;
    Lock lock(r);
    std::memset(r->data, 0, r->size);
}

bool AudioShouldPause(std::uint64_t nowMs, std::uint64_t scriptTickMs, bool requested) {
    // A concurrently refreshed heartbeat can be newer than the sampled now.
    return requested || (nowMs >= scriptTickMs && nowMs - scriptTickMs >= 250);
}

void FloatToPcm16(const float* in, std::int16_t* out, std::size_t n, float g0, float g1) {
    const float step = n > 1 ? (g1 - g0) / static_cast<float>(n) : 0.0f;
    float g = g0;
    for (std::size_t i = 0; i < n; ++i, g += step) {
        float v = in[i] * g;
        if (!(v == v)) v = 0.0f; // NaN
        v = std::clamp(v, -1.0f, 1.0f) * 32767.0f;
        out[i] = static_cast<std::int16_t>(std::lrintf(v));
    }
}

void ProbeTone(float* out, std::size_t n, double& t, int rate, float amplitude) {
    const double dt = 1.0 / rate;
    for (std::size_t i = 0; i < n; ++i, t += dt) {
        const double cycle = std::fmod(t, 0.6);
        // 10 ms fades at both ends of each beep (no clicks).
        float env = 0.0f;
        if (cycle < 0.4) env = static_cast<float>(std::min({1.0, cycle / 0.01, (0.4 - cycle) / 0.01}));
        out[i] = amplitude * env * std::sin(static_cast<float>(2.0 * 3.14159265358979 * 440.0 * t));
    }
}

void Limiter::Init(double sampleRate, double releaseSeconds, float ceilingLinear) {
    envelope = 1.0f;
    ceiling = ceilingLinear;
    release = static_cast<float>(1.0 - std::exp(-1.0 / (releaseSeconds * sampleRate)));
}

void Limiter::Process(float* x, std::size_t n, float gain) {
    for (std::size_t i = 0; i < n; ++i) {
        float y = x[i] * gain;
        if (!(y == y)) y = 0.0f;
        envelope += (1.0f - envelope) * release;
        const float a = std::fabs(y);
        if (a * envelope > ceiling) envelope = ceiling / a;
        x[i] = y * envelope;
    }
}

GameTimeRing::GameTimeRing(int rate, int seconds)
    : block_(std::max(1, rate / 10)), samples_(static_cast<std::size_t>(rate) * seconds),
      tags_(samples_.size() / static_cast<std::size_t>(block_), -1) {}

void GameTimeRing::Add(std::int64_t at, const float* pcm, int n) {
    const auto blocks = static_cast<std::int64_t>(tags_.size());
    std::lock_guard<std::mutex> lock(lock_);
    for (int k = 0; k < n; ++k) {
        const std::int64_t a = at + k, b = Block(a);
        if (a < 0) continue;
        std::int16_t* s = &samples_[static_cast<std::size_t>((b % blocks) * block_)];
        if (tags_[static_cast<std::size_t>(b % blocks)] != b) {
            std::fill(s, s + block_, std::int16_t{0}); // a lap ago, or never written
            tags_[static_cast<std::size_t>(b % blocks)] = b;
        }
        const long v = s[a % block_] + std::lrintf(std::clamp(pcm[k], -1.0f, 1.0f) * 32767.0f);
        s[a % block_] = static_cast<std::int16_t>(std::clamp(v, -32768L, 32767L));
    }
}

bool GameTimeRing::Read(std::int64_t at, std::int16_t* out, std::int64_t n) {
    const auto blocks = static_cast<std::int64_t>(tags_.size());
    std::lock_guard<std::mutex> lock(lock_);
    bool any = false;
    for (std::int64_t k = 0; k < n; ++k) {
        const std::int64_t a = at + k, b = Block(a);
        const bool held = a >= 0 && tags_[static_cast<std::size_t>(b % blocks)] == b;
        out[k] = held ? samples_[static_cast<std::size_t>((b % blocks) * block_ + a % block_)] : std::int16_t{0};
        any = any || out[k] != 0;
    }
    return any;
}

void PlayheadClock::Update(double now, std::uint32_t p) {
    const bool moved = count == 0 || p != changes[count - 1].p;
    if (count && (p < changes[count - 1].p || p - changes[count - 1].p > 1000)) count = 0; // a scrub
    if (moved) {
        if (count == 64) std::copy(changes + 1, changes + 64, changes), --count;
        changes[count++] = {now, p};
    }
    int first = 0;
    while (first < count - 1 && changes[first].t < now - 300.0) ++first;
    if (first) std::copy(changes + first, changes + count, changes), count -= first;
    const Change& last = changes[count - 1];
    ms = last.p, at = last.t;
    if (now - last.t > 150.0) {
        rate = 0.0; // paused
    } else if (count >= 2 && last.t - changes[0].t >= 50.0) {
        rate = std::clamp((static_cast<double>(last.p) - changes[0].p) / (last.t - changes[0].t), 0.0, 4.0);
    }
}

void PlayheadFollower::Fill(const std::int16_t* clip, std::int64_t length, double target, double rate, int sampleRate,
                            double jump, float* out, int n) {
    if (rate < 0.05 || target < 0.0 || target >= static_cast<double>(length)) {
        pos = target; // paused or outside the clip: resume from the playhead
        std::fill(out, out + n, 0.0f);
        return;
    }
    if (pos < 0.0 || std::fabs(pos - target) > jump) pos = target;
    const double step = std::clamp(rate + (target - pos) / sampleRate, 0.0, 4.0);
    for (int k = 0; k < n; ++k, pos += step) {
        const auto i = static_cast<std::int64_t>(pos);
        const double f = pos - static_cast<double>(i);
        const double a = i >= 0 && i < length ? clip[i] : 0, b = i + 1 >= 0 && i + 1 < length ? clip[i + 1] : 0;
        out[k] = static_cast<float>((a + (b - a) * f) / 32768.0);
    }
}

} // namespace gtaaudio

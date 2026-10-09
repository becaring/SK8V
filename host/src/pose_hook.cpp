#include "game_probe.h"
#include "host_util.h"
#include "gun_pose.h"
#include "live_clip.h"
#include "pose_hook.h"
#include "pose_math.h"

#include <windows.h>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <mutex>

namespace posehook {
namespace {

constexpr int kSlot = 65;                       // vtable offset 0x208
constexpr std::uintptr_t kSlotRva = 0x7B0270;   // CPed per-frame animation/IK pass
constexpr int kCopiedSlots = 512;
constexpr int kMaxBones = 512;

using SlotFn = bool(__fastcall*)(std::uintptr_t, bool);

// GTA's per-ped animation job chain runs on a worker thread after the slot-65
// pass and rewrites the skeleton (every frame, 11-4000 us later, from these two
// task functions, both run by the job runner +0x1345B58):
//   +0x15B9948  params +0x20 = locals: copies the animated locals (memcpy from
//               params +0x18) and composes the object matrices
//   +0x15B8680  params +0x18 = locals: object -> local rewrite
// (objects follow the locals in one allocation: objects = locals + 64 * bones.)
// Each is detoured with a 5-byte jump (one aligned 8-byte store) to a relay
// near the image; after GTA's task returns, Skate's pose is written for the
// player's skeleton only, on that worker, before the chain completes.
struct JobDetour {
    std::uintptr_t rva;
    int localsOffset;               // params offset of the locals pointer
    std::uintptr_t target = 0;
    std::uint64_t original = 0;     // first 8 bytes
    std::uintptr_t trampoline = 0;  // original prologue (5 bytes) + jump back
};
using JobFn = std::uintptr_t(__fastcall*)(std::uintptr_t, std::uintptr_t, std::uintptr_t, std::uintptr_t);
JobDetour g_jobs[2] = {{0x15B9948, 0x20}, {0x15B8680, 0x18}};
// mov rax, rsp; push rbp; push rbx; push rsi; push rdi; push r12..r15
constexpr std::uint8_t kJobPrologue[15] = {0x48, 0x8B, 0xC4, 0x55, 0x53, 0x56, 0x57, 0x41,
                                           0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57};

std::uintptr_t g_entity = 0;
std::uintptr_t g_originalTable = 0;
std::uintptr_t* g_copy = nullptr; // [0] RTTI, [1..] slots
SlotFn g_original = nullptr;
std::atomic<std::uintptr_t> g_skeleton{0};

// Triple-buffered pose: the script thread fills a buffer no reader can be
// using and then publishes its index; a reader always copies a complete pose.
// The pose is stored relative to the entity, made so on the script thread
// right after the ped was placed (PublishPose). Made relative in the anim job
// against the entity's matrix of that moment, it paired by turns this frame's
// pose with the last frame's placement or the reverse (the job runs on a
// worker, the placement on the script thread): the body sat a frame of travel
// ahead or behind, and the Rockstar Editor recorded it skipping back and forth.
// (A seqlock that gave up under contention skipped the write for that frame,
// which showed GTA's A-pose for one frame.)
struct PoseBuffer {
    float bones[kMaxBones * 16];
    int count = 0;
};
PoseBuffer g_buffers[3];
std::atomic<int> g_latest{-1};

// Hand-off fade: direction +1 in (GTA -> Skate), -1 out (Skate -> GTA).
std::atomic<int> g_fadeDir{0}, g_fadeMs{0};
std::atomic<ULONGLONG> g_fadeStart{0};
std::atomic<bool> g_freeze{false}; // FadeOut: capture Skate's pose relative to the ped once
bool g_frozen = false;             // writer state (one writer at a time per ped)
bool g_haveWritten = false;
std::atomic<bool> g_outDone{false};
using namespace posemath;

// Skate's share of the written pose now (1 outside a fade).
float SkateWeight() {
    const int dir = g_fadeDir.load(std::memory_order_acquire);
    if (!dir) return 1.0f;
    const int ms = g_fadeMs.load(std::memory_order_relaxed);
    const float t =
        ms > 0 ? static_cast<float>(GetTickCount64() - g_fadeStart.load(std::memory_order_relaxed)) / ms : 1.0f;
    const float u = t < 0.0f ? 0.0f : t > 1.0f ? 1.0f : t;
    return dir > 0 ? u : 1.0f - u;
}

// Armed on the board (gun_pose.h): the gun's share of the pose, ramped in
// and out. GTA animates the armed ped; the hook takes its upper body.
std::atomic<int> g_gunDir{-1}, g_gunMs{1};
std::atomic<ULONGLONG> g_gunStart{0};
float GunWeight() {
    const int ms = g_gunMs.load(std::memory_order_relaxed);
    float t = static_cast<float>(GetTickCount64() - g_gunStart.load(std::memory_order_relaxed)) / (ms > 0 ? ms : 1);
    t = t < 0.0f ? 0.0f : t > 1.0f ? 1.0f : t;
    return g_gunDir.load(std::memory_order_acquire) > 0 ? t : 1.0f - t;
}
gunpose::Spine g_spine; // resolved on Install
// GTA's own animated pose, copied by the job that composes it (the first
// writer of the frame) while the gun is out; the later passes of the same
// frame find our pose in place and compose from this copy.
M g_gtaPose[kMaxBones];
bool g_gtaValid = false;

// Skate's world pose -> entity-relative object matrices.
bool ToObjects(const float* pose, int count, std::uintptr_t parent, M* object) {
    M entityInv;
    if (!Inverse(Load(parent), entityInv)) return false;
    for (int i = 0; i < count; ++i) {
        M world;
        std::memcpy(&world, pose + 16 * i, sizeof(world));
        object[i] = Compose(world, entityInv);
    }
    return true;
}

// Bone i relative to its parent (the root keeps its object matrix).
M LocalOf(const M* object, const std::int16_t* parents, int i, int count) {
    M local = object[i];
    const int p = parents[i];
    M inv;
    if (i > 0 && p >= 0 && p < count && Inverse(object[p], inv)) local = Compose(object[i], inv);
    return local;
}

void ClipBone(int i, const M& local) {
    const Q q = FromRows(local);
    const float qa[4] = {q.x, q.y, q.z, q.w};
    liveclip::WriteBone(i, local.r[3], qa);
}

// After GTA's pass: object matrices (and matching locals) from Skate's pose.
// `fresh`: the locals are GTA's animated ones just copied by +0x15B9948.
void Apply(std::uintptr_t s, bool fresh = false) {
    const std::uintptr_t data = *reinterpret_cast<const std::uintptr_t*>(s + 0x00);
    const std::uintptr_t parent = *reinterpret_cast<const std::uintptr_t*>(s + 0x08);
    const std::uintptr_t locals = *reinterpret_cast<const std::uintptr_t*>(s + 0x10);
    const std::uintptr_t objects = *reinterpret_cast<const std::uintptr_t*>(s + 0x18);
    const int count = *reinterpret_cast<const int*>(s + 0x20);
    if (!data || !parent || !locals || !objects || count <= 0 || count > kMaxBones) return;
    const auto* parents = *reinterpret_cast<const std::int16_t* const*>(data + 0x38);
    if (!parents) return;

    const int latest = g_latest.load(std::memory_order_acquire);
    if (latest < 0) return;
    const PoseBuffer& buffer = g_buffers[latest];
    if (buffer.count != count) return; // a pose published for this skeleton only

    // The pass and its two anim tasks write one after another for this ped,
    // so the fade state below is never written concurrently.
    static thread_local M object[kMaxBones];
    static M frozen[kMaxBones], written[kMaxBones];
    const float weight = SkateWeight();
    const bool out = g_fadeDir.load(std::memory_order_relaxed) < 0;
    if (out) {
        if (g_freeze.exchange(false)) g_frozen = false;
        if (weight <= 0.0f) {
            g_outDone = true;
            return; // GTA's pose alone
        }
    }
    if (out && g_frozen) {
        for (int i = 0; i < count; ++i) object[i] = frozen[i];
    } else {
        std::memcpy(object, buffer.bones, sizeof(M) * count);
        if (out) {
            for (int i = 0; i < count; ++i) frozen[i] = object[i];
            g_frozen = true;
        }
    }
    if (weight < 1.0f) {
        // A later writer of the same frame finds our blend already in place:
        // blending it again would compound the weight.
        bool ours = g_haveWritten;
        for (int i = 0; ours && i < count; ++i) {
            const M cur = Load(objects + 64 * i);
            for (int r = 0; ours && r < 4; ++r)
                for (int k = 0; ours && k < 3; ++k) ours = std::fabs(cur.r[r][k] - written[i].r[r][k]) < 1e-5f;
        }
        if (ours) return;
        for (int i = 0; i < count; ++i) object[i] = Blend(Load(objects + 64 * i), object[i], weight);
    }
    const float gun = GunWeight();
    if (gun <= 0.0f) {
        g_gtaValid = false;
    } else {
        if (fresh) {
            for (int i = 0; i < count; ++i) g_gtaPose[i] = Load(objects + 64 * i);
            g_gtaValid = true;
        }
        if (g_gtaValid && g_spine.Valid()) {
            static M composed[kMaxBones];
            gunpose::Compose(object, g_gtaPose, parents, count, g_spine, composed);
            for (int i = 0; i < count; ++i) object[i] = gun >= 1.0f ? composed[i] : Blend(object[i], composed[i], gun);
        }
    }
    for (int i = 0; i < count; ++i) written[i] = object[i];
    g_haveWritten = true;
    // LiveClip: the same locals into the clip GTA plays (live_clip.h).
    const bool clip = liveclip::BeginWrite();
    const bool compare = clip && fresh && liveclip::Comparing() && !g_frozen;
    for (int i = 0; i < count; ++i) {
        if (compare) {
            const M gta = Load(locals + 64 * i);
            const Q q = FromRows(gta);
            const float qa[4] = {q.x, q.y, q.z, q.w};
            liveclip::Compare(i, gta.r[3], qa);
        }
        StoreXyz(objects + 64 * i, object[i]);
        const M local = LocalOf(object, parents, i, count);
        StoreXyz(locals + 64 * i, local);
        if (clip) ClipBone(i, local);
    }
    if (compare) liveclip::EndCompare();
}

template <int Index>
std::uintptr_t __fastcall JobWrap(std::uintptr_t params, std::uintptr_t b, std::uintptr_t c, std::uintptr_t d) {
    JobDetour& job = g_jobs[Index];
    const std::uintptr_t result = reinterpret_cast<JobFn>(job.trampoline)(params, b, c, d);
    const std::uintptr_t s = g_skeleton.load(std::memory_order_acquire);
    if (s && params) {
        const std::uintptr_t locals = *reinterpret_cast<const std::uintptr_t*>(s + 0x10);
        if (locals && *reinterpret_cast<const std::uintptr_t*>(params + job.localsOffset) == locals) Apply(s, Index == 0);
    }
    return result;
}

void PatchQword(std::uintptr_t at, std::uint64_t value) {
    DWORD old = 0;
    VirtualProtect(reinterpret_cast<void*>(at), 8, PAGE_EXECUTE_READWRITE, &old);
    InterlockedExchange64(reinterpret_cast<volatile LONG64*>(at), static_cast<LONG64>(value));
    VirtualProtect(reinterpret_cast<void*>(at), 8, old, &old);
    FlushInstructionCache(GetCurrentProcess(), reinterpret_cast<void*>(at), 8);
}

bool InstallJobs(void (*log)(const char*)) {
    if (g_jobs[0].target) return true;
    const auto base = reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr));
    char line[256];
    for (const JobDetour& job : g_jobs) {
        const std::uintptr_t t = base + job.rva;
        if ((t & 7) != 0 || std::memcmp(reinterpret_cast<const void*>(t), kJobPrologue, sizeof(kJobPrologue)) != 0) {
            std::snprintf(line, sizeof(line), "pose hook: anim task +0x%llx prologue differs; job hooks not installed",
                          static_cast<unsigned long long>(job.rva));
            log(line);
            return false;
        }
    }
    static std::uint8_t* mem = nullptr; // reused on reinstall; never freed
    if (!mem) mem = static_cast<std::uint8_t*>(util::AllocNear(base, 4096));
    if (!mem) {
        log("pose hook: no memory near the game image; job hooks not installed");
        return false;
    }
    for (const JobDetour& job : g_jobs) {
        const auto distance = reinterpret_cast<std::intptr_t>(mem) - static_cast<std::intptr_t>(base + job.rva + 5);
        if (distance < INT32_MIN + 4096 || distance > INT32_MAX - 4096) {
            log("pose hook: relay memory out of jump range; job hooks not installed");
            return false;
        }
    }
    void* const wraps[2] ={reinterpret_cast<void*>(&JobWrap<0>), reinterpret_cast<void*>(&JobWrap<1>)};
    for (int i = 0; i < 2; ++i) {
        JobDetour& job = g_jobs[i];
        const std::uintptr_t t = base + job.rva;
        std::uint8_t* relay = mem + 64 * i;      // jmp [rip+0]; dq wrap
        std::uint8_t* tramp = mem + 64 * i + 32; // 5 prologue bytes; jmp [rip+0]; dq t+5
        relay[0] = 0xFF;
        relay[1] = 0x25;
        std::memset(relay + 2, 0, 4);
        std::memcpy(relay + 6, &wraps[i], 8);
        std::memcpy(tramp, kJobPrologue, 5); // mov rax, rsp; push rbp; push rbx
        tramp[5] = 0xFF;
        tramp[6] = 0x25;
        std::memset(tramp + 7, 0, 4);
        const std::uintptr_t back = t + 5;
        std::memcpy(tramp + 11, &back, 8);
        job.trampoline = reinterpret_cast<std::uintptr_t>(tramp);
        job.original = *reinterpret_cast<const std::uint64_t*>(t);
    }
    FlushInstructionCache(GetCurrentProcess(), mem, 4096);
    for (int i = 0; i < 2; ++i) {
        JobDetour& job = g_jobs[i];
        const std::uintptr_t t = base + job.rva;
        std::uint8_t patch[8];
        std::memcpy(patch, &job.original, 8);
        const auto rel = static_cast<std::int32_t>(reinterpret_cast<std::intptr_t>(mem + 64 * i) -
                                                   static_cast<std::intptr_t>(t + 5));
        patch[0] = 0xE9;
        std::memcpy(patch + 1, &rel, 4);
        std::uint64_t value;
        std::memcpy(&value, patch, 8);
        job.target = t;
        PatchQword(t, value);
    }
    log("pose hook: anim job tasks +0x15B9948 and +0x15B8680 detoured; Skate's pose follows them on the worker");
    return true;
}

void UninstallJobs(void (*log)(const char*)) {
    if (!g_jobs[0].target) return;
    for (JobDetour& job : g_jobs) {
        PatchQword(job.target, job.original);
        job.target = 0;
    }
    // Relays and trampolines stay allocated: a worker may still be inside one.
    log("pose hook: anim job tasks restored");
}

bool __fastcall Slot(std::uintptr_t self, bool flag) {
    const bool result = g_original(self, flag);
    if (self == g_entity) {
        const std::uintptr_t s = g_skeleton.load(std::memory_order_relaxed);
        if (s) Apply(s);
    }
    return result;
}

} // namespace

void SetGun(bool on, int ms) {
    const int dir = on ? 1 : -1;
    if (g_gunDir.load(std::memory_order_relaxed) == dir) return;
    // Start from the current weight so a quick release does not jump.
    const float now = GunWeight();
    const float from = on ? now : 1.0f - now;
    g_gunMs.store(ms > 0 ? ms : 1, std::memory_order_relaxed);
    g_gunStart.store(GetTickCount64() - static_cast<ULONGLONG>(from * (ms > 0 ? ms : 1)), std::memory_order_relaxed);
    g_gunDir.store(dir, std::memory_order_release);
}

bool Install(std::uintptr_t entity, const probe::SkeletonInfo& skeleton, void (*log)(const char*)) {
    char line[256];
    if (!entity || !skeleton.skeleton) return false;
    g_skeleton = skeleton.skeleton;
    {
        gunpose::Spine sp;
        const std::uintptr_t data = *reinterpret_cast<const std::uintptr_t*>(skeleton.skeleton);
        const auto* parents = data ? *reinterpret_cast<const std::int16_t* const*>(data + 0x38) : nullptr;
        for (const int tag : {57597, 23553, 24816, 24817, 24818}) { // Spine_Root, Spine0..3
            const int i = probe::BoneIndexByTag(skeleton, static_cast<std::uint16_t>(tag));
            if (i >= 0) sp.bone[sp.count++] = i;
        }
        if (parents && sp.count && sp.bone[0] < skeleton.count) sp.base = parents[sp.bone[0]];
        g_spine = sp;
    }
    if (g_entity == entity) return true;
    if (g_entity) Uninstall(log);
    const auto base = reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr));
    const std::uintptr_t table = *reinterpret_cast<const std::uintptr_t*>(entity);
    const std::uintptr_t slot = reinterpret_cast<const std::uintptr_t*>(table)[kSlot];
    if (slot != base + kSlotRva) {
        std::snprintf(line, sizeof(line), "pose hook: ped vtable slot %d is image +0x%llx, expected +0x%llx; not installed",
                      kSlot, static_cast<unsigned long long>(slot - base), static_cast<unsigned long long>(kSlotRva));
        log(line);
        return false;
    }
    auto* copy = static_cast<std::uintptr_t*>(
        VirtualAlloc(nullptr, sizeof(std::uintptr_t) * (kCopiedSlots + 1), MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE));
    if (!copy) return false;
    std::memcpy(copy, reinterpret_cast<const void*>(table - sizeof(std::uintptr_t)),
                sizeof(std::uintptr_t) * (kCopiedSlots + 1)); // RTTI locator + slots
    // A pose left from an earlier session belongs to another ped (or another
    // place): writing it moves the mesh away from the entity (a Director Mode
    // actor rendered nowhere). Until this session publishes, GTA's own pose stands.
    g_latest.store(-1, std::memory_order_release);
    g_original = reinterpret_cast<SlotFn>(slot);
    copy[1 + kSlot] = reinterpret_cast<std::uintptr_t>(&Slot);
    g_copy = copy;
    g_originalTable = table;
    g_entity = entity;
    reinterpret_cast<std::atomic<std::uintptr_t>*>(entity)->store(reinterpret_cast<std::uintptr_t>(copy + 1));
    log("pose hook: player ped's animation pass (vtable slot 65) wrapped; Skate's pose follows it");
    InstallJobs(log);
    return true;
}

void Uninstall(void (*log)(const char*)) {
    g_skeleton = 0;
    g_fadeDir = 0;
    g_gunDir = -1, g_gunStart = 0, g_gunMs = 1; // no gun share left for the next session
    g_haveWritten = false;
    UninstallJobs(log);
    if (!g_entity) return;
    auto* const slot0 = reinterpret_cast<std::atomic<std::uintptr_t>*>(g_entity);
    if (slot0->load() == reinterpret_cast<std::uintptr_t>(g_copy + 1)) slot0->store(g_originalTable);
    // The copied table stays allocated: a game thread may still be using it.
    g_entity = 0;
    log("pose hook: original ped virtual table restored");
}

// LiveClip, script thread: this pose into the clip before GTA evaluates it
// later in the frame. With only the job hook writing, GTA's animated pose was
// the previous frame's and the replay recorder sampled either one (the body
// snapped between the two in Rockstar Editor clips). With both equal it cannot.
// Not during fades or with the gun out: those blend with GTA's own pose in the job.
void WriteClipEarly(const M* object, const std::int16_t* parents, int count) {
    if (SkateWeight() < 1.0f || GunWeight() > 0.0f || !liveclip::BeginWrite()) return;
    for (int i = 0; i < count; ++i) ClipBone(i, LocalOf(object, parents, i, count));
}

// Script thread, after the ped was placed this frame: the pose relative to
// the entity as placed (see PoseBuffer).
void PublishPose(const float* bones, int count) {
    count = std::min(count, kMaxBones);
    const std::uintptr_t s = g_skeleton.load(std::memory_order_acquire);
    if (!s) return;
    const std::uintptr_t data = *reinterpret_cast<const std::uintptr_t*>(s + 0x00);
    const std::uintptr_t parent = *reinterpret_cast<const std::uintptr_t*>(s + 0x08);
    if (!data || !parent || *reinterpret_cast<const int*>(s + 0x20) != count) return;
    const auto* parents = *reinterpret_cast<const std::int16_t* const*>(data + 0x38);
    // The buffer after the published one: readers only ever hold the
    // published one (a read takes microseconds; publishes are a frame apart).
    const int latest = g_latest.load(std::memory_order_relaxed);
    const int next = (latest + 1) % 3;
    M* object = reinterpret_cast<M*>(g_buffers[next].bones);
    if (!parents || !ToObjects(bones, count, parent, object)) return;
    WriteClipEarly(object, parents, count);
    g_buffers[next].count = count;
    g_latest.store(next, std::memory_order_release);
}

void FadeOut(int ms) {
    g_outDone = false;
    g_fadeMs = ms;
    g_fadeStart = GetTickCount64();
    g_freeze = true;
    g_fadeDir.store(-1, std::memory_order_release);
}

void FadeIn(int ms) {
    g_outDone = false;
    g_fadeMs = ms;
    g_fadeStart = GetTickCount64();
    g_fadeDir.store(1, std::memory_order_release);
}

bool FadeOutDone() { return g_fadeDir.load() < 0 && (g_outDone.load() || SkateWeight() <= 0.0f); }

} // namespace posehook

#pragma once
#include "ped_collider_builder.h"
#include <cstdint>
#include <cstddef>

// The live GTA entity skeleton and fragment, read through one bounded,
// fault-tolerant memory reader (ReadLive).
namespace probe {

// Live entity skeleton (crSkeleton) on this build.
struct SkeletonInfo {
    std::uintptr_t skeleton = 0;
    std::uintptr_t parentMtx = 0;  // +0x08: one matrix (entity world)
    std::uintptr_t objectMtx = 0;  // +0x10: parent-relative LOCAL matrices, 0x40 each (historic name)
    std::uintptr_t globalMtx = 0;  // +0x18: entity-relative OBJECT matrices, 0x40 each (historic name)
    std::uintptr_t bones = 0;      // crSkeletonData bones, 80 bytes each
    int count = 0;
};
// Entity handle -> skeleton. `why` names the failing step when this returns false.
bool ResolvePedSkeleton(int ped, SkeletonInfo& out, const char*& why);
int BoneIndexByTag(const SkeletonInfo& s, std::uint16_t tag);
// The ped's live fragment type (fragInst +0x78): its ragdoll compound and
// skeleton are what any ped model actually collides with. Fails closed.
bool PedFragmentType(int ped, std::uintptr_t& type, const char*& why);
// Bounded, fault-tolerant read of live process memory (ReadProcessMemory).
bool ReadLive(std::uintptr_t address, void* out, std::size_t size);
// The entity's fragInst (its phInst), 0 on failure. Script thread.
std::uintptr_t FragInstOf(std::uintptr_t entity);

// Live memory for the collider builder.
class LiveMemory : public pedcollision::builder::Memory {
public:
    bool Read(std::uintptr_t a, void* out, std::size_t n) const override { return ReadLive(a, out, n); }
};

} // namespace probe

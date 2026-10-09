#pragma once
// Posed body colliders for ANY ped: built at runtime from the ragdoll compound
// the ped's own fragment type carries (capsules/boxes, bone tags, bind
// transforms), by patching the verified collider templates written by
// tools/ped_collider_templates.py. The template tool proves the patch map
// complete against the rage-built exports; this module replays it. Pure: no
// GTA calls, so the HOST test reads the owned source fragment through the
// same Memory interface the game uses for live memory.
#include <array>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <string>
#include <vector>

namespace pedcollision::builder {

struct Layout {
    std::uint32_t name = 0, composite = 0, child = 0, transforms = 0, boxes = 0;
};
struct KindTemplate {
    std::array<std::uint8_t, 16> header{};
    std::vector<std::uint8_t> sys;
    Layout layout{};
};
struct Templates {
    KindTemplate capsule, box;
    std::array<std::uint8_t, 16> ytypHeader{};
    std::vector<std::uint8_t> ytyp;
};
// <dir>/templates.txt, capsule.sys, box.sys, ytyp.sys.
bool Load(const std::filesystem::path& dir, Templates& out, std::string& why);

// Primitive kinds of the authored compound (phBound type byte).
constexpr std::uint8_t kCapsule = 1, kBox = 3;

struct Child {
    std::uint8_t kind = 0;
    // The authored bound (vtable included; only documented fields are used).
    std::array<std::uint8_t, 128> raw{};
    // bind @ inverse bone bind (RAGE row-vector), as the exporter composes it.
    double relative[16]{};
    std::uint16_t boneTag = 0;
};

class Memory {
public:
    virtual ~Memory() = default;
    virtual bool Read(std::uintptr_t address, void* out, std::size_t size) const = 0;
};

// The ragdoll compound of a fragment type (`type` = fragType address): every
// child must be a capsule or box with a zero capsule tail and a bone tag the
// fragment's own skeleton resolves. Fails closed with `why`.
bool ReadCompound(const Memory& memory, std::uintptr_t type, std::vector<Child>& out, std::string& why);

// 14 characters, the template name length: "skv_" + 8 hex content hash + "_c"/"_b".
std::string ModelName(const Templates& t, const Child& child);

// What GTA's drawable store reads from a placed collider drawable (Legacy3889,
// decrypted image; evidence/2026-10-03/ped-collider-shader-group.md). The
// store's only placed-listener (+0x9445c4) reads the shader group (+0x10) and
// its shader count (+0x18) without a null check, so the group must exist; it
// must be empty (no dictionary, shaders or +0x20 array; blocks size 4 = the
// 0x40 header, the retail encoding) so the listener touches nothing more. LOD
// models, skeleton, joints, lights and +0xC0 stay null (placement skips them);
// the embedded composite bound (+0xC8) and the name are in the section.
bool CheckDrawable(const std::vector<std::uint8_t>& sys, std::string& why);

// Decompressed system sections for one child (BuildYdr output passes
// CheckDrawable or the build fails).
bool BuildYdr(const Templates& t, const Child& child, const std::string& name, std::vector<std::uint8_t>& sys);
bool BuildYtyp(const Templates& t, const Child& child, const std::string& name, std::vector<std::uint8_t>& sys);

// RSC7 file: the template header and the system section as raw deflate in
// stored blocks (valid deflate; RSC7 readers inflate it).
std::vector<std::uint8_t> Rsc7File(const std::array<std::uint8_t, 16>& header, const std::vector<std::uint8_t>& sys);

} // namespace pedcollision::builder

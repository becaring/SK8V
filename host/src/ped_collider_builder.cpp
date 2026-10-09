#include "ped_collider_builder.h"
#include "host_util.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <sstream>

namespace pedcollision::builder {
namespace {

// Child bound fields copied verbatim (offset, size) - the exporter's set.
struct Field { std::uint32_t offset, size; };
constexpr Field kChildFields[] = {{0x14, 4}, {0x20, 12}, {0x2c, 4}, {0x30, 12}, {0x3c, 4}, {0x40, 12}, {0x4c, 1},
                                  {0x4d, 1}, {0x4e, 1}, {0x4f, 1}, {0x50, 12}, {0x5c, 1}, {0x5d, 1}, {0x60, 12}, {0x6c, 4}};
// Composite fields the exporter takes from the child; the rest are computed.
constexpr Field kOuterCopied[] = {{0x2c, 4}, {0x3c, 4}, {0x4c, 1}, {0x4d, 1}, {0x4e, 1}, {0x4f, 1},
                                  {0x5c, 1}, {0x5d, 1}, {0x60, 12}, {0x6c, 4}};
constexpr std::uint32_t kYtypBbMin = 0x230, kYtypBbMax = 0x240, kYtypBsCentre = 0x250, kYtypBsRadius = 0x260;
constexpr std::uint32_t kYtypHashes[] = {0x268, 0x278, 0x280, 0x2C8};

bool ReadFile(const std::filesystem::path& p, std::vector<std::uint8_t>& out) {
    std::ifstream in(p, std::ios::binary);
    if (!in) return false;
    out.assign(std::istreambuf_iterator<char>(in), {});
    return true;
}

bool Hex16(const std::string& hex, std::array<std::uint8_t, 16>& out) {
    if (hex.size() != 32) return false;
    auto nibble = [](char c) {
        return c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : c >= 'A' && c <= 'F' ? c - 'A' + 10 : -1;
    };
    for (std::size_t i = 0; i < 16; ++i) {
        const int hi = nibble(hex[2 * i]), lo = nibble(hex[2 * i + 1]);
        if (hi < 0 || lo < 0) return false;
        out[i] = static_cast<std::uint8_t>(hi << 4 | lo);
    }
    return true;
}

void PutFloats(std::vector<std::uint8_t>& d, std::uint32_t at, const float* v, int n) {
    std::memcpy(d.data() + at, v, sizeof(float) * n);
}

float F(const std::array<std::uint8_t, 128>& raw, std::uint32_t at) {
    float v;
    std::memcpy(&v, raw.data() + at, 4);
    return v;
}

// Enclosing bounds of the child box under the bone-relative transform, in
// double like the exporter's numpy, rounded once to float.
struct Bounds { float low[3], high[3], center[3], radius; };
Bounds Enclose(const Child& c) {
    const double lo[3] = {F(c.raw, 0x30), F(c.raw, 0x34), F(c.raw, 0x38)};
    const double hi[3] = {F(c.raw, 0x20), F(c.raw, 0x24), F(c.raw, 0x28)};
    double mn[3] = {INFINITY, INFINITY, INFINITY}, mx[3] = {-INFINITY, -INFINITY, -INFINITY};
    for (int corner = 0; corner < 8; ++corner) {
        // itertools.product order is irrelevant to min/max.
        const double p[3] = {(corner & 4) ? hi[0] : lo[0], (corner & 2) ? hi[1] : lo[1], (corner & 1) ? hi[2] : lo[2]};
        for (int col = 0; col < 3; ++col) {
            const double v = p[0] * c.relative[0 * 4 + col] + p[1] * c.relative[1 * 4 + col] +
                             p[2] * c.relative[2 * 4 + col] + c.relative[3 * 4 + col];
            mn[col] = (std::min)(mn[col], v);
            mx[col] = (std::max)(mx[col], v);
        }
    }
    Bounds b{};
    double r2 = 0.0;
    for (int i = 0; i < 3; ++i) {
        const double center = (mn[i] + mx[i]) * 0.5;
        b.low[i] = static_cast<float>(mn[i]);
        b.high[i] = static_cast<float>(mx[i]);
        b.center[i] = static_cast<float>(center);
        r2 += (mx[i] - center) * (mx[i] - center);
    }
    b.radius = static_cast<float>(std::sqrt(r2));
    return b;
}

const KindTemplate* KindOf(const Templates& t, const Child& c) {
    if (c.kind == kCapsule) return &t.capsule;
    if (c.kind == kBox) return &t.box;
    return nullptr;
}

template <class T>
bool Get(const Memory& m, std::uintptr_t at, T& out) {
    return m.Read(at, &out, sizeof(T));
}

constexpr std::uint64_t kSystemBase = 0x50000000;

template <class T>
T At(const std::vector<std::uint8_t>& d, std::size_t at) {
    T v;
    std::memcpy(&v, d.data() + at, sizeof(T));
    return v;
}

// Pointer field at `at`: null, or `size` bytes inside the system section.
bool Pointer(const std::vector<std::uint8_t>& d, std::size_t at, std::size_t size, std::size_t& offset, bool& null) {
    const auto v = At<std::uint64_t>(d, at);
    null = v == 0;
    offset = 0;
    if (null) return true;
    if (v < kSystemBase || v - kSystemBase > d.size() || d.size() - (v - kSystemBase) < size) return false;
    offset = static_cast<std::size_t>(v - kSystemBase);
    return true;
}

} // namespace

bool CheckDrawable(const std::vector<std::uint8_t>& d, std::string& why) {
    why = "collider drawable shorter than its header";
    if (d.size() < 0xD0) return false;
    std::size_t sg = 0, name = 0, bound = 0;
    bool null = false;
    why = "collider drawable has no shader group (the drawable store's placed-listener dereferences it)";
    if (!Pointer(d, 0x10, 0x40, sg, null) || null) return false;
    why = "collider shader group is not the empty retail layout";
    if (At<std::uint32_t>(d, sg + 0x04) != 1 || At<std::uint64_t>(d, sg + 0x08) || At<std::uint64_t>(d, sg + 0x10) ||
        At<std::uint16_t>(d, sg + 0x18) || At<std::uint16_t>(d, sg + 0x1A) || At<std::uint32_t>(d, sg + 0x1C) ||
        At<std::uint64_t>(d, sg + 0x20) || At<std::uint64_t>(d, sg + 0x28) || At<std::uint32_t>(d, sg + 0x30) != 0x40 / 16 ||
        At<std::uint32_t>(d, sg + 0x34) || At<std::uint64_t>(d, sg + 0x38)) return false;
    why = "collider drawable carries render data";
    for (std::size_t at : {0x18u, 0x50u, 0x58u, 0x60u, 0x68u, 0x90u, 0xA0u, 0xB0u, 0xC0u})
        if (At<std::uint64_t>(d, at)) return false;
    if (At<std::uint16_t>(d, 0xB8)) return false;
    why = "collider drawable name outside the section";
    if (!Pointer(d, 0xA8, 1, name, null) || null || std::find(d.begin() + static_cast<std::ptrdiff_t>(name), d.end(), 0) == d.end())
        return false;
    why = "collider drawable lacks its embedded composite bound";
    if (!Pointer(d, 0xC8, 0x70, bound, null) || null || d[bound + 0x10] != 10) return false;
    why = "";
    return true;
}

bool Load(const std::filesystem::path& dir, Templates& out, std::string& why) {
    out = {};
    std::ifstream index(dir / "templates.txt");
    std::string line;
    why = "collider templates: missing templates.txt (tools/ped_collider_templates.py)";
    if (!index || !std::getline(index, line) || line != "SKATEV_COLLIDER_TEMPLATES\t1") return false;
    bool capsule = false, box = false, ytyp = false;
    while (std::getline(index, line)) {
        if (!line.empty() && line.back() == '\r') line.pop_back();
        std::istringstream fields(line);
        std::string kind, hex;
        std::getline(fields, kind, '\t');
        std::getline(fields, hex, '\t');
        if (kind == "ytyp") {
            if (!Hex16(hex, out.ytypHeader) || !ReadFile(dir / "ytyp.sys", out.ytyp)) return false;
            ytyp = true;
            continue;
        }
        KindTemplate* k = kind == "capsule" ? &out.capsule : kind == "box" ? &out.box : nullptr;
        if (!k) return false;
        std::string n[5];
        for (auto& v : n) std::getline(fields, v, '\t');
        try {
            k->layout = {static_cast<std::uint32_t>(std::stoul(n[0])), static_cast<std::uint32_t>(std::stoul(n[1])),
                         static_cast<std::uint32_t>(std::stoul(n[2])), static_cast<std::uint32_t>(std::stoul(n[3])),
                         static_cast<std::uint32_t>(std::stoul(n[4]))};
        } catch (...) {
            return false;
        }
        if (!Hex16(hex, k->header) || !ReadFile(dir / (kind + ".sys"), k->sys)) return false;
        const Layout& l = k->layout;
        why = "collider templates: layout outside the template";
        for (std::uint32_t at : {l.name + 14u, l.composite + 0x70u, l.child + 0x70u, l.transforms + 0x40u, l.boxes + 0x20u})
            if (at > k->sys.size()) return false;
        // Templates exported before the empty shader group crash GTA's
        // drawable store; refuse them rather than generate such colliders.
        std::string drawable;
        if (!CheckDrawable(k->sys, drawable)) {
            why = "collider templates: " + kind + " template unusable (" + drawable +
                  "); rerun tools/prepare-ped-colliders.py and tools/ped_collider_templates.py";
            return false;
        }
        (kind == "capsule" ? capsule : box) = true;
    }
    why = "collider templates: incomplete set";
    if (!(capsule && box && ytyp) || out.ytyp.size() < kYtypHashes[3] + 4) return false;
    why = "";
    return true;
}

bool ReadCompound(const Memory& m, std::uintptr_t type, std::vector<Child>& out, std::string& why) {
    out.clear();
    std::uintptr_t group = 0, lod = 0, composite = 0, bounds = 0, transforms = 0, fragments = 0;
    std::uint8_t compositeKind = 0, lodCount = 0;
    std::uint16_t count = 0;
    why = "fragment ragdoll compound unavailable";
    if (!Get(m, type + 0xf0, group) || !group || !Get(m, group + 0x10, lod) || !lod ||
        !Get(m, lod + 0xe8, composite) || !composite || !Get(m, composite + 0x10, compositeKind)) return false;
    why = "fragment physics bound is not a composite";
    if (compositeKind != 10) return false;
    why = "fragment compound children disagree";
    if (!Get(m, composite + 0xa0, count) || !Get(m, lod + 0x11d, lodCount) || count == 0 || count > 64 ||
        count != lodCount || !Get(m, composite + 0x70, bounds) || !Get(m, composite + 0x78, transforms) ||
        !Get(m, lod + 0xd0, fragments) || !bounds || !transforms || !fragments) return false;
    // The fragment's own skeleton: bone tags and inverse binds.
    std::uintptr_t drawable = 0, skeleton = 0, bones = 0, inverse = 0;
    std::uint16_t boneCount = 0;
    why = "fragment skeleton unavailable";
    if (!Get(m, type + 0x30, drawable) || !drawable || !Get(m, drawable + 0x18, skeleton) || !skeleton ||
        !Get(m, skeleton + 0x5e, boneCount) || !boneCount || !Get(m, skeleton + 0x20, bones) ||
        !Get(m, skeleton + 0x28, inverse) || !bones || !inverse) return false;
    auto matrix = [&](std::uintptr_t at, double (&v)[16]) {
        float f[16];
        if (!m.Read(at, f, sizeof(f))) return false;
        for (int i = 0; i < 16; ++i) v[i] = f[i];
        // RAGE packs metadata in column 3 of transform arrays.
        v[3] = v[7] = v[11] = 0.0;
        v[15] = 1.0;
        for (double x : v)
            if (!std::isfinite(x)) return false;
        return true;
    };
    for (std::uint16_t i = 0; i < count; ++i) {
        Child c{};
        std::uintptr_t bound = 0, fragment = 0;
        why = "fragment child bound unreadable";
        if (!Get(m, bounds + 8 * i, bound) || !bound || !m.Read(bound, c.raw.data(), 112)) return false;
        c.kind = c.raw[0x10];
        why = "fragment child primitive is not a capsule or box";
        if (c.kind != kCapsule && c.kind != kBox) return false;
        if (c.kind == kCapsule) {
            if (!m.Read(bound + 112, c.raw.data() + 112, 16)) return false;
            why = "capsule tail data cannot be represented";
            for (int k = 112; k < 128; ++k)
                if (c.raw[k]) return false;
        }
        why = "fragment child bone tag unreadable";
        if (!Get(m, fragments + 8 * i, fragment) || !fragment || !Get(m, fragment + 0x12, c.boneTag)) return false;
        int boneIndex = -1;
        for (std::uint16_t b = 0; b < boneCount && boneIndex < 0; ++b) {
            std::uint16_t tag = 0;
            if (!Get(m, bones + 80ull * b + 0x44, tag)) return false;
            if (tag == c.boneTag) boneIndex = b;
        }
        why = "fragment child bone tag absent from its skeleton";
        if (boneIndex < 0) return false;
        double bind[16], inv[16];
        why = "fragment child transform invalid";
        if (!matrix(transforms + 64ull * i, bind) || !matrix(inverse + 64ull * boneIndex, inv)) return false;
        for (int r = 0; r < 4; ++r)
            for (int col = 0; col < 4; ++col) {
                double s = 0.0;
                for (int k = 0; k < 4; ++k) s += bind[r * 4 + k] * inv[k * 4 + col];
                c.relative[r * 4 + col] = s;
            }
        out.push_back(c);
    }
    why = "";
    return true;
}

std::string ModelName(std::uint32_t pedModel, std::size_t index, const Child& c) {
    // FNV-1a over what the limb is, never over its geometry: the compound is
    // read live and its floats differ between take-outs, so content names
    // minted 21 new names per take-out. Each name holds GTA streaming slots
    // for the process; the pool ran dry (crash at GTA5+0x16AAF5B, 2026-10-07,
    // and on the second take-out with Open Interiors installed, 2026-10-09).
    const char kind = c.kind == kCapsule ? 'c' : 'b';
    const std::uint32_t key[] = {pedModel, static_cast<std::uint32_t>(index), c.boneTag, static_cast<std::uint32_t>(kind)};
    std::uint32_t h = 2166136261u;
    for (const auto* b = reinterpret_cast<const std::uint8_t*>(key); b != reinterpret_cast<const std::uint8_t*>(key + 4); ++b)
        h = (h ^ *b) * 16777619u;
    char name[16];
    std::snprintf(name, sizeof(name), "skv_%08x_%c", h, kind);
    return name;
}

bool BuildYdr(const Templates& t, const Child& c, const std::string& name, std::vector<std::uint8_t>& sys) {
    const KindTemplate* k = KindOf(t, c);
    if (!k || name.size() != 14) return false;
    const Layout& l = k->layout;
    sys = k->sys;
    const char* old = reinterpret_cast<const char*>(sys.data() + l.name);
    if (strnlen(old, 15) != 14) return false;
    const Bounds b = Enclose(c);
    // Drawable bounds.
    PutFloats(sys, 0x20, b.center, 3);
    PutFloats(sys, 0x2c, &b.radius, 1);
    PutFloats(sys, 0x30, b.low, 3);
    PutFloats(sys, 0x40, b.high, 3);
    // Composite: child fields, then computed bounds.
    for (const Field& f : kOuterCopied) std::memcpy(sys.data() + l.composite + f.offset, c.raw.data() + f.offset, f.size);
    PutFloats(sys, l.composite + 0x14, &b.radius, 1);
    PutFloats(sys, l.composite + 0x20, b.high, 3);
    PutFloats(sys, l.composite + 0x30, b.low, 3);
    PutFloats(sys, l.composite + 0x40, b.center, 3);
    PutFloats(sys, l.composite + 0x50, b.center, 3);
    // The authored child.
    for (const Field& f : kChildFields) std::memcpy(sys.data() + l.child + f.offset, c.raw.data() + f.offset, f.size);
    // Transform rows (xyz; w lanes keep the template's words).
    for (int row = 0; row < 4; ++row) {
        const float v[3] = {static_cast<float>(c.relative[row * 4]), static_cast<float>(c.relative[row * 4 + 1]),
                            static_cast<float>(c.relative[row * 4 + 2])};
        PutFloats(sys, l.transforms + 16 * row, v, 3);
    }
    // Per-child box: the child's own extents, margin in max.w.
    std::memcpy(sys.data() + l.boxes, c.raw.data() + 0x30, 12);
    std::memcpy(sys.data() + l.boxes + 16, c.raw.data() + 0x20, 12);
    std::memcpy(sys.data() + l.boxes + 28, c.raw.data() + 0x2c, 4);
    std::memcpy(sys.data() + l.name, name.data(), 14);
    std::string why;
    return CheckDrawable(sys, why);
}

bool BuildYtyp(const Templates& t, const Child& c, const std::string& name, std::vector<std::uint8_t>& sys) {
    if (t.ytyp.empty() || name.size() != 14) return false;
    sys = t.ytyp;
    const Bounds b = Enclose(c);
    PutFloats(sys, kYtypBbMin, b.low, 3);
    PutFloats(sys, kYtypBbMax, b.high, 3);
    PutFloats(sys, kYtypBsCentre, b.center, 3);
    PutFloats(sys, kYtypBsRadius, &b.radius, 1);
    const std::uint32_t h = util::Joaat(name);
    for (std::uint32_t at : kYtypHashes) std::memcpy(sys.data() + at, &h, 4);
    return true;
}

std::vector<std::uint8_t> Rsc7File(const std::array<std::uint8_t, 16>& header, const std::vector<std::uint8_t>& sys) {
    std::vector<std::uint8_t> out(header.begin(), header.end());
    std::size_t at = 0;
    do {
        const std::size_t n = (std::min)(sys.size() - at, std::size_t{65535});
        const bool last = at + n == sys.size();
        out.push_back(last ? 1 : 0); // BFINAL, BTYPE 00 (stored); byte aligned
        out.push_back(static_cast<std::uint8_t>(n & 0xff));
        out.push_back(static_cast<std::uint8_t>(n >> 8));
        out.push_back(static_cast<std::uint8_t>(~n & 0xff));
        out.push_back(static_cast<std::uint8_t>((~n >> 8) & 0xff));
        out.insert(out.end(), sys.begin() + static_cast<std::ptrdiff_t>(at), sys.begin() + static_cast<std::ptrdiff_t>(at + n));
        at += n;
    } while (at < sys.size());
    return out;
}

} // namespace pedcollision::builder

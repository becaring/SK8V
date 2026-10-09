#pragma once
// GTA peds as Skate sees them, with hitboxes like the player's: every part of a ped's own ragdoll compound
// (capsules and boxes of its live fragment type, with their authored masses),
// posed by the ped's live skeleton each frame, sent to the runtime with
// sv_set_ped_parts. Read-only: GTA memory is only read (checked reads).
#include "skatev_runtime.h"

#include <cmath>
#include <cstdint>
#include <vector>

namespace pedbodies {

// Appends the posed parts of `ped` (GTA world space) tagged `tag`; false and
// nothing appended when its compound or skeleton cannot be read.
bool Collect(int ped, std::uint32_t tag, std::vector<SvPedPart>& out);

// The ragdoll component of `ped`'s part nearest to `point` among `parts`
// (the last frame sent), or -1.
int NearestComponent(const std::vector<SvPedPart>& parts, std::uint32_t tag, SvVec3 point);

// Angular velocity a momentum J struck at `point` gives the ped `tag` as one
// rigid body: w = I^-1 (r x J) about the parts' centre of mass, I from each
// part as a solid box (capsules as their bounding box) plus its offset.
// Capped at 20 rad/s (a shin hit at 5 m/s turns ~15). Zero without parts.
inline SvVec3 LaunchTurn(const std::vector<SvPedPart>& parts, std::uint32_t tag, SvVec3 point, SvVec3 J) {
    float mass = 0.0f;
    SvVec3 com{};
    for (const SvPedPart& p : parts) {
        if (p.tag != tag || !(p.mass_kg > 0.0f)) continue;
        mass += p.mass_kg;
        com = {com.x + p.centre.x * p.mass_kg, com.y + p.centre.y * p.mass_kg, com.z + p.centre.z * p.mass_kg};
    }
    if (!(mass > 0.0f)) return {};
    com = {com.x / mass, com.y / mass, com.z / mass};
    float I[3][3]{};
    for (const SvPedPart& p : parts) {
        if (p.tag != tag || !(p.mass_kg > 0.0f)) continue;
        const float m = p.mass_kg;
        float h[3] = {p.half_extents.x, p.half_extents.y, p.half_extents.z};
        if (p.kind == 1) h[1] += p.radius; // capsule: segment plus its caps
        const float d[3] = {(h[1] * h[1] + h[2] * h[2]) * m / 3.0f, (h[0] * h[0] + h[2] * h[2]) * m / 3.0f,
                            (h[0] * h[0] + h[1] * h[1]) * m / 3.0f};
        const SvVec3 ax[3] = {p.right, p.forward, p.up};
        const float r[3] = {p.centre.x - com.x, p.centre.y - com.y, p.centre.z - com.z};
        const float rr = r[0] * r[0] + r[1] * r[1] + r[2] * r[2];
        for (int i = 0; i < 3; ++i)
            for (int j = 0; j < 3; ++j) {
                float own = 0.0f;
                for (int k = 0; k < 3; ++k) {
                    const float a[3] = {ax[k].x, ax[k].y, ax[k].z};
                    own += d[k] * a[i] * a[j];
                }
                I[i][j] += own + m * ((i == j ? rr : 0.0f) - r[i] * r[j]);
            }
    }
    const float r[3] = {point.x - com.x, point.y - com.y, point.z - com.z};
    const float L[3] = {r[1] * J.z - r[2] * J.y, r[2] * J.x - r[0] * J.z, r[0] * J.y - r[1] * J.x};
    const float det = I[0][0] * (I[1][1] * I[2][2] - I[1][2] * I[2][1]) - I[0][1] * (I[1][0] * I[2][2] - I[1][2] * I[2][0]) +
                      I[0][2] * (I[1][0] * I[2][1] - I[1][1] * I[2][0]);
    if (!(std::fabs(det) > 1e-9f)) return {};
    // Cramer's rule for I w = L.
    auto col = [&](int c) {
        float M[3][3];
        for (int i = 0; i < 3; ++i)
            for (int j = 0; j < 3; ++j) M[i][j] = j == c ? L[i] : I[i][j];
        return (M[0][0] * (M[1][1] * M[2][2] - M[1][2] * M[2][1]) - M[0][1] * (M[1][0] * M[2][2] - M[1][2] * M[2][0]) +
                M[0][2] * (M[1][0] * M[2][1] - M[1][1] * M[2][0])) / det;
    };
    SvVec3 w{col(0), col(1), col(2)};
    const float n = std::sqrt(w.x * w.x + w.y * w.y + w.z * w.z);
    if (n > 20.0f) w = {w.x * 20.0f / n, w.y * 20.0f / n, w.z * 20.0f / n};
    return w;
}

} // namespace pedbodies

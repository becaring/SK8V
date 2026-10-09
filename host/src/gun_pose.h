#pragma once
// Gun pose: GTA animates the armed, aiming ped (its own task tree, aim and
// weapon clips) and Skate keeps the lower half. The hook calls this with
// Skate's pose and GTA's animated pose, both in the ped's object space.
//
// The hips and legs (everything outside the spine chain) are Skate's. The
// chain, Spine_Root..Spine3, turns from Skate's orientation to GTA's in equal
// steps, so the twist between a sideways skate stance and an aim down the
// camera is spread over the whole torso; the last chain bone and everything
// above it (arms, head, the hand holding the gun) is GTA's, rigid on it.
#include "pose_math.h"

#include <cstdint>

namespace gunpose {

using posemath::M;

struct Spine {
    int base = -1;                 // the parent of the first chain bone
    int count = 0;                 // chain bones found, root to chest (Spine_Root..Spine3)
    int bone[5] = {-1, -1, -1, -1, -1};
    bool Valid() const { return base >= 0 && count >= 2; }
};

// Rotation between two bones in degrees (0 when they face alike).
inline float AngleDeg(const M& a, const M& b) {
    float dot = 0.0f;
    for (int r = 0; r < 3; ++r)
        for (int c = 0; c < 3; ++c) dot += a.r[r][c] * b.r[r][c];
    const float cosine = (dot - 1.0f) * 0.5f;
    return std::acos(cosine > 1.0f ? 1.0f : cosine < -1.0f ? -1.0f : cosine) * 57.29578f;
}

// `out` is Skate's pose with the upper body replaced; `parents` precede
// children. `out` may not alias the inputs. Leaves Skate's pose untouched when
// the chain or a matrix is unusable.
inline void Compose(const M* skate, const M* gta, const std::int16_t* parents, int count, const Spine& sp, M* out) {
    for (int i = 0; i < count; ++i) out[i] = skate[i];
    if (!sp.Valid() || sp.base >= count) return;
    for (int k = 0; k < sp.count; ++k)
        if (sp.bone[k] < 0 || sp.bone[k] >= count) return;

    const int n = sp.count;
    M final[5], delta[5];
    M prevFinal = skate[sp.base], prevGta = gta[sp.base];
    for (int k = 0; k < n; ++k) {
        const int b = sp.bone[k];
        M inv;
        if (!posemath::Inverse(gta[b], inv)) return;
        M f = posemath::Blend(skate[b], gta[b], static_cast<float>(k + 1) / static_cast<float>(n));
        // Where GTA has this bone against its parent, in the parent's axes,
        // laid out on the parent's final axes.
        const float d[3] = {gta[b].r[3][0] - prevGta.r[3][0], gta[b].r[3][1] - prevGta.r[3][1],
                            gta[b].r[3][2] - prevGta.r[3][2]};
        float local[3];
        for (int r = 0; r < 3; ++r) local[r] = d[0] * prevGta.r[r][0] + d[1] * prevGta.r[r][1] + d[2] * prevGta.r[r][2];
        for (int c = 0; c < 3; ++c)
            f.r[3][c] = prevFinal.r[3][c] + local[0] * prevFinal.r[0][c] + local[1] * prevFinal.r[1][c] +
                        local[2] * prevFinal.r[2][c];
        final[k] = f;
        delta[k] = posemath::Compose(inv, f);
        prevFinal = f, prevGta = gta[b];
    }
    // Bones under the chain follow the nearest chain bone above them.
    int anchor[512];
    for (int i = 0; i < count && i < 512; ++i) {
        int a = -1;
        for (int k = 0; k < n; ++k)
            if (i == sp.bone[k]) a = k;
        const int p = parents[i];
        const bool chain = a >= 0;
        if (!chain && p >= 0 && p < i) a = anchor[p];
        anchor[i] = a;
        if (chain) out[i] = final[a];
        else if (a >= 0) out[i] = posemath::Compose(gta[i], delta[a]);
    }
}

} // namespace gunpose

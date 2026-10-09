#pragma once
#include "pose_math.h"
namespace boardnative::detail {
using posemath::M;
// Native board bone parents (SKATEBOARD_ROOT, TRUCK_FRONT, TRUCK_BACK, wheels).
inline constexpr int Parents[7] = {-1, 0, 0, 1, 2, 1, 2};
// World bone matrices -> entity-relative object matrices and parent-relative locals.
inline bool Convert(const M (&world)[7], const M& entity, M (&objects)[7], M (&locals)[7]) {
    using namespace posemath;
    M inverse;
    if (!Finite(entity) || !Inverse(entity, inverse)) return false;
    for (int i = 0; i < 7; ++i) {
        if (!Finite(world[i])) return false;
        objects[i] = Compose(world[i], inverse);
        if (!Finite(objects[i])) return false;
    }
    for (int i = 0; i < 7; ++i) {
        locals[i] = objects[i];
        if (Parents[i] >= 0) {
            if (!Inverse(objects[Parents[i]], inverse)) return false;
            locals[i] = Compose(objects[i], inverse);
        }
        if (!Finite(locals[i])) return false;
    }
    return true;
}
} // namespace boardnative::detail

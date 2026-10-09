#pragma once
// Bone matrix maths shared by the pose hook and the gun pose (gun_pose.h).
// Matrices are RAGE bone rows (X, Y, Z axes, translation; row vectors).
#include <cmath>
#include <cstring>
#include <cstdint>

namespace posemath {

struct M {
    float r[4][4]; // rows: X axis, Y axis, Z axis, translation
};

// a then b (row-vector convention).
inline M Compose(const M& a, const M& b) {
    M o{};
    for (int row = 0; row < 4; ++row) {
        for (int c = 0; c < 3; ++c) {
            float v = a.r[row][0] * b.r[0][c] + a.r[row][1] * b.r[1][c] + a.r[row][2] * b.r[2][c];
            if (row == 3) v += b.r[3][c];
            o.r[row][c] = v;
        }
        o.r[row][3] = a.r[row][3];
    }
    return o;
}

inline bool Inverse(const M& m, M& out) {
    const float(*a)[4] = m.r;
    const float det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) -
                      a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) +
                      a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if (!std::isfinite(det) || std::fabs(det) < 1e-12f) return false;
    const float id = 1.0f / det;
    M o{};
    o.r[0][0] = (a[1][1] * a[2][2] - a[1][2] * a[2][1]) * id;
    o.r[0][1] = (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * id;
    o.r[0][2] = (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * id;
    o.r[1][0] = (a[1][2] * a[2][0] - a[1][0] * a[2][2]) * id;
    o.r[1][1] = (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * id;
    o.r[1][2] = (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * id;
    o.r[2][0] = (a[1][0] * a[2][1] - a[1][1] * a[2][0]) * id;
    o.r[2][1] = (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * id;
    o.r[2][2] = (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * id;
    for (int c = 0; c < 3; ++c) o.r[3][c] = -(a[3][0] * o.r[0][c] + a[3][1] * o.r[1][c] + a[3][2] * o.r[2][c]);
    for (int row = 0; row < 4; ++row) o.r[row][3] = m.r[row][3];
    out = o;
    return true;
}

inline M Identity() {
    M m{};
    m.r[0][0] = m.r[1][1] = m.r[2][2] = m.r[3][3] = 1.0f;
    return m;
}

inline bool Finite(const M& m) {
    for (const auto& row : m.r)
        for (float x : row)
            if (!std::isfinite(x)) return false;
    return true;
}

inline M Load(std::uintptr_t p) {
    M m;
    std::memcpy(&m, reinterpret_cast<const void*>(p), sizeof(m));
    return m;
}

inline void StoreXyz(std::uintptr_t p, const M& m) {
    auto* d = reinterpret_cast<float*>(p);
    for (int row = 0; row < 4; ++row)
        for (int c = 0; c < 3; ++c) d[row * 4 + c] = m.r[row][c];
}

struct Q {
    float x, y, z, w;
};

// Rows are the axes (row-vector convention).
inline Q FromRows(const M& m) {
    const float m00 = m.r[0][0], m11 = m.r[1][1], m22 = m.r[2][2];
    Q q;
    const float tr = m00 + m11 + m22;
    if (tr > 0.0f) {
        const float s = std::sqrt(tr + 1.0f) * 2.0f;
        q = {(m.r[1][2] - m.r[2][1]) / s, (m.r[2][0] - m.r[0][2]) / s, (m.r[0][1] - m.r[1][0]) / s, 0.25f * s};
    } else if (m00 > m11 && m00 > m22) {
        const float s = std::sqrt(1.0f + m00 - m11 - m22) * 2.0f;
        q = {0.25f * s, (m.r[1][0] + m.r[0][1]) / s, (m.r[2][0] + m.r[0][2]) / s, (m.r[1][2] - m.r[2][1]) / s};
    } else if (m11 > m22) {
        const float s = std::sqrt(1.0f + m11 - m00 - m22) * 2.0f;
        q = {(m.r[1][0] + m.r[0][1]) / s, 0.25f * s, (m.r[2][1] + m.r[1][2]) / s, (m.r[2][0] - m.r[0][2]) / s};
    } else {
        const float s = std::sqrt(1.0f + m22 - m00 - m11) * 2.0f;
        q = {(m.r[2][0] + m.r[0][2]) / s, (m.r[2][1] + m.r[1][2]) / s, 0.25f * s, (m.r[0][1] - m.r[1][0]) / s};
    }
    return q;
}

// FromRows for a rigid rotation only: unit, orthogonal, right-handed axes
// (within 0.002), normalised result; false otherwise.
inline bool RigidQuaternion(const M& m, Q& q) {
    if (!Finite(m)) return false;
    for (int a = 0; a < 3; ++a) {
        const float* x = m.r[a];
        if (std::fabs(x[0] * x[0] + x[1] * x[1] + x[2] * x[2] - 1.0f) > 0.002f) return false;
        for (int b = a + 1; b < 3; ++b)
            if (std::fabs(x[0] * m.r[b][0] + x[1] * m.r[b][1] + x[2] * m.r[b][2]) > 0.002f) return false;
    }
    const float(*a)[4] = m.r;
    const float det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) +
                      a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if (std::fabs(det - 1.0f) > 0.002f) return false;
    q = FromRows(m);
    const float n = std::sqrt(q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w);
    if (!std::isfinite(n) || n < 1e-4f) return false;
    q = {q.x / n, q.y / n, q.z / n, q.w / n};
    return true;
}

inline void ToRows(const Q& q, M& m) {
    const float x = q.x, y = q.y, z = q.z, w = q.w;
    m.r[0][0] = 1 - 2 * (y * y + z * z);
    m.r[0][1] = 2 * (x * y + z * w);
    m.r[0][2] = 2 * (x * z - y * w);
    m.r[1][0] = 2 * (x * y - z * w);
    m.r[1][1] = 1 - 2 * (x * x + z * z);
    m.r[1][2] = 2 * (y * z + x * w);
    m.r[2][0] = 2 * (x * z + y * w);
    m.r[2][1] = 2 * (y * z - x * w);
    m.r[2][2] = 1 - 2 * (x * x + y * y);
}

// `a` at t = 0, `b` at t = 1 (bones are rigid: unit axes).
inline M Blend(const M& a, const M& b, float t) {
    const Q qa = FromRows(a);
    Q qb = FromRows(b);
    float d = qa.x * qb.x + qa.y * qb.y + qa.z * qb.z + qa.w * qb.w;
    if (d < 0.0f) qb = {-qb.x, -qb.y, -qb.z, -qb.w}, d = -d;
    Q q;
    if (d > 0.9995f) {
        q = {qa.x + (qb.x - qa.x) * t, qa.y + (qb.y - qa.y) * t, qa.z + (qb.z - qa.z) * t, qa.w + (qb.w - qa.w) * t};
    } else {
        const float th = std::acos(d), s = std::sin(th);
        const float wa = std::sin((1 - t) * th) / s, wb = std::sin(t * th) / s;
        q = {qa.x * wa + qb.x * wb, qa.y * wa + qb.y * wb, qa.z * wa + qb.z * wb, qa.w * wa + qb.w * wb};
    }
    const float l = std::sqrt(q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w);
    q = {q.x / l, q.y / l, q.z / l, q.w / l};
    M o = a;
    ToRows(q, o);
    for (int c = 0; c < 3; ++c) o.r[3][c] = a.r[3][c] + (b.r[3][c] - a.r[3][c]) * t;
    return o;
}

} // namespace posemath

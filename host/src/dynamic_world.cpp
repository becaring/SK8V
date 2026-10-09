#include "dynamic_world.h"
#include "prop_collision.h"
#include <windows.h>
#include <main.h>
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <fstream>
#include <filesystem>
#include <iomanip>
#include <sstream>
#include "natives.h"
#include "ped_bodies.h"
#include "ped_speech.h"
#include "pose_math.h"

namespace {
using gta::Call;

constexpr std::size_t kMaxBodies = 128;
constexpr int kPoolScratch = 1024;
PropCollisionIndex g_props;

float Dist2(const Vector3& a, SvVec3 b) {
    const float dx = a.x - b.x, dy = a.y - b.y, dz = a.z - b.z;
    return dx * dx + dy * dy + dz * dz;
}

// Rotation whose local x, y, z axes are right, forward, up (unit).
SvQuat QuatFromAxes(const Vector3& right, const Vector3& forward, const Vector3& up) {
    posemath::M m{};
    const Vector3* axes[3] = {&right, &forward, &up};
    for (int a = 0; a < 3; ++a) m.r[a][0] = axes[a]->x, m.r[a][1] = axes[a]->y, m.r[a][2] = axes[a]->z;
    const posemath::Q q = posemath::FromRows(m);
    return {q.x, q.y, q.z, q.w};
}
}

std::size_t DynamicWorld::LoadPropTemplates(const std::string& worldCache) {
    g_props = {};
    std::filesystem::path stem(worldCache);
    stem.replace_extension();
    std::ifstream models(stem.string() + ".prop-models.txt");
    g_props.ReadModels(models);
    // A stale manifest alone must not advertise a missing local template.
    std::vector<std::uint32_t> missing;
    for (const auto hash : g_props.Models()) {
        std::ostringstream name; name << std::hex << std::setfill('0') << std::setw(8) << hash << ".svwc";
        std::error_code error;
        if (!std::filesystem::is_regular_file(std::filesystem::path(stem.string() + ".prop-models") / name.str(), error))
            missing.push_back(hash);
    }
    for (const auto hash : missing) g_props.RemoveModel(hash);
    return g_props.Models().size();
}

bool DynamicWorld::AddBody(int entity, EntityKind kind) {
    if (bodies_.size() >= kMaxBodies) return false;
    if (!Call<BOOL>(gta::DOES_ENTITY_EXIST, entity) || !Call<BOOL>(gta::IS_ENTITY_VISIBLE, entity)) return false;
    if (kind == EntityKind::Object && Call<BOOL>(gta::IS_ENTITY_ATTACHED, entity)) return false;
    const Hash model = Call<Hash>(gta::GET_ENTITY_MODEL, entity);
    Vector3 lo{}, hi{};
    Call<void>(gta::GET_MODEL_DIMENSIONS, model, &lo, &hi);
    // The pinned alloc8or DB defines outputs forward, right, up, position.
    // Preserve each basis column's scale for exact local collision templates.
    Vector3 fwd{}, right{}, up{}, pos{};
    Call<void>(gta::GET_ENTITY_MATRIX, entity, &fwd, &right, &up, &pos);
    auto length = [](const Vector3& v) { return std::sqrt(v.x*v.x + v.y*v.y + v.z*v.z); };
    const float sx = length(right), sy = length(fwd), sz = length(up);
    if (!(std::isfinite(sx) && std::isfinite(sy) && std::isfinite(sz) && sx > 1e-5f && sy > 1e-5f && sz > 1e-5f)) return false;
    Vector3 nr{}, nf{}, nu{};
    nr.x=right.x/sx; nr.y=right.y/sx; nr.z=right.z/sx;
    nf.x=fwd.x/sy; nf.y=fwd.y/sy; nf.z=fwd.z/sy;
    nu.x=up.x/sz; nu.y=up.y/sz; nu.z=up.z/sz;
    const float cx = (lo.x + hi.x) * 0.5f, cy = (lo.y + hi.y) * 0.5f, cz = (lo.z + hi.z) * 0.5f;
    SvBox b{};
    b.tag = Tag(kind, entity);
    b.center = {pos.x + right.x * cx + fwd.x * cy + up.x * cz, pos.y + right.y * cx + fwd.y * cy + up.y * cz,
                pos.z + right.z * cx + fwd.z * cy + up.z * cz};
    b.rotation = QuatFromAxes(nr, nf, nu);
    b.half_extents = {(hi.x - lo.x) * 0.5f * sx, (hi.y - lo.y) * 0.5f * sy, (hi.z - lo.z) * 0.5f * sz};
    if (kind == EntityKind::Ped) {
        // Model bounds include arm span; a ped body is narrower.
        b.half_extents.x = (std::min)(b.half_extents.x, 0.3f);
        b.half_extents.y = (std::min)(b.half_extents.y, 0.3f);
    }
    if (!g_props.HasTemplate(model) &&
        !(b.half_extents.x > 0.02f && b.half_extents.y > 0.02f && b.half_extents.z > 0.02f)) return false;
    SvDynamicBody body{};
    body.size = sizeof(body);
    body.model_hash = model;
    body.position = {pos.x, pos.y, pos.z};
    body.right = {right.x, right.y, right.z};
    body.forward = {fwd.x, fwd.y, fwd.z};
    body.up = {up.x, up.y, up.z};
    body.fallback = b;
    // Contact exchange: GTA's own velocities (moving contact body).
    const Vector3 v = Call<Vector3>(gta::GET_ENTITY_VELOCITY, entity);
    const Vector3 w = Call<Vector3>(gta::GET_ENTITY_ROTATION_VELOCITY, entity);
    body.linear_velocity = {v.x, v.y, v.z};
    body.angular_velocity = {w.x, w.y, w.z};
    if (kind == EntityKind::Vehicle) {
        // Skitch grab line: the rear bumper bone. Bikes and models without
        // bumper_r get no line.
        const int bone = Call<int>(gta::GET_ENTITY_BONE_INDEX_BY_NAME, entity, "bumper_r");
        if (bone >= 0) {
            const Vector3 g = Call<Vector3>(gta::GET_WORLD_POSITION_OF_ENTITY_BONE, entity, bone);
            if (std::isfinite(g.x) && std::isfinite(g.y) && std::isfinite(g.z)) {
                body.grab_point = {g.x, g.y, g.z};
                body.grab_flags = 1;
            }
        }
    }
    bool isDoor = false, unlocked = false;
    if (kind == EntityKind::Object) {
        // Open unlocked doors, but retain their moving exact collision when a template exists.
        Hash door = 0;
        isDoor = Call<BOOL>(gta::DOOR_SYSTEM_FIND_EXISTING_DOOR, pos.x, pos.y, pos.z, model, &door) && door;
        unlocked = isDoor && PropCollisionIndex::DoorMayOpen(Call<int>(gta::DOOR_SYSTEM_GET_DOOR_STATE, door));
        if (unlocked) {
            doors_.push_back({door, {pos.x, pos.y, pos.z}, {fwd.x, fwd.y, fwd.z}});
            ++stats_.doors;
        }
    }
    const auto route = g_props.Select(body, kind == EntityKind::Object, isDoor, unlocked);
    const bool exact = route == PropCollisionIndex::Route::Exact;
    if (kind == EntityKind::Object && !isDoor && !exact) {
        // Preserve the existing litter interaction only when no exact bound
        // is available. A known template never gets replaced by its box.
        const float largest = (std::max)({b.half_extents.x, b.half_extents.y, b.half_extents.z});
        if (largest < 0.3f || b.half_extents.z < 0.05f) {
            debris_.push_back({entity, {b.center.x, b.center.y, b.center.z}});
            ++stats_.debris;
            return false;
        }
    }
    if (route == PropCollisionIndex::Route::None) { ++stats_.missingBounds; return false; }
    body.flags = PropCollisionIndex::AllowsBox(route) ? 1u : 0u; // Exact gaps must NEVER be filled by the fallback box.
    bodies_.push_back(body);
    if (exact) ++stats_.exactBounds;
    else if (kind == EntityKind::Object) ++stats_.missingBounds;
    return true;
}

void DynamicWorld::Gather(int self, SvVec3 center, float radius, int presentationEntity) {
    bodies_.clear();
    parts_.clear();
    debris_.clear();
    doors_.clear();
    const std::uint32_t exchanged = stats_.exchanged, pedImpulses = stats_.pedImpulses;
    stats_ = {};
    stats_.exchanged = exchanged; // cumulative
    stats_.pedImpulses = pedImpulses;
    scratch_.resize(kPoolScratch);
    const float r2 = radius * radius;
    auto& nearby = nearby_;
    nearby.clear();
    auto collect = [&](int (*pool)(int*, int), EntityKind kind, float kindRadius2) {
        const int n = pool(scratch_.data(), kPoolScratch);
        for (int i = 0; i < n; ++i) {
            const int e = scratch_[i];
            if (e == self || e == presentationEntity) continue;
            const Vector3 p = Call<Vector3>(gta::GET_ENTITY_COORDS, e, 0);
            const float d2 = Dist2(p, center);
            if (d2 <= kindRadius2) nearby.push_back({d2, e, kind});
        }
    };
    collect(worldGetAllVehicles, EntityKind::Vehicle, r2);
    collect(worldGetAllPeds, EntityKind::Ped, r2 * 0.5f);
    collect(worldGetAllObjects, EntityKind::Object, r2 * 0.5f);
    std::sort(nearby.begin(), nearby.end(), [](const Candidate& a, const Candidate& b) { return a.d2 < b.d2; });
    for (const Candidate& c : nearby) {
        // A ped Skate can read as its own ragdoll parts gets no box.
        if (c.kind == EntityKind::Ped && pedParts_ && Call<BOOL>(gta::IS_ENTITY_VISIBLE, c.entity) &&
            pedbodies::Collect(c.entity, Tag(EntityKind::Ped, c.entity), parts_)) {
            ++stats_.peds;
            ++stats_.pedParts;
            continue;
        }
        if (!AddBody(c.entity, c.kind)) continue;
        switch (c.kind) {
        case EntityKind::Vehicle: ++stats_.vehicles; break;
        case EntityKind::Ped: ++stats_.peds; break;
        case EntityKind::Object: ++stats_.objects; break;
        }
    }
}

void DynamicWorld::KickDebris(SvVec3 board, SvVec3 v) {
    const float speed = std::sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
    if (speed < 1.5f) return;
    const unsigned long now = GetTickCount();
    for (const Debris& d : debris_) {
        const float dx = d.center.x - board.x, dy = d.center.y - board.y, dz = d.center.z - board.z;
        if (dx * dx + dy * dy > 0.8f * 0.8f || std::fabs(dz) > 0.8f) continue;
        const std::uint32_t tag = Tag(EntityKind::Object, d.entity);
        auto it = cooldown_.find(tag);
        if (it != cooldown_.end() && now - it->second < 1500) continue;
        cooldown_[tag] = now;
        if (!Call<BOOL>(gta::DOES_ENTITY_EXIST, d.entity)) continue;
        Call<void>(gta::APPLY_FORCE_TO_ENTITY, d.entity, 1, v.x * 0.6f, v.y * 0.6f, 1.0f, 0.0f, 0.0f, 0.0f, 0, 0, 1, 1, 0,
                   1);
    }
}

void DynamicWorld::OpenDoors(SvVec3 skater) {
    for (const Door& d : doors_) {
        const float dx = skater.x - d.hinge.x, dy = skater.y - d.hinge.y, dz = skater.z - d.hinge.z;
        if (dx * dx + dy * dy > 2.2f * 2.2f || std::fabs(dz) > 2.5f) continue;
        // Swing away from the skater: the side of the door's forward axis the
        // skater is on decides the sign.
        const float side = dx * d.forward.x + dy * d.forward.y;
        Call<void>(gta::DOOR_SYSTEM_SET_DOOR_STATE, d.hash, 0, 0, 1);
        Call<void>(gta::DOOR_SYSTEM_SET_OPEN_RATIO, d.hash, side > 0.0f ? -1.0f : 1.0f, 0, 1);
    }
}

void DynamicWorld::React(const SvDynamicHit* hits, std::uint32_t count, SvVec3 v) {
    const unsigned long now = GetTickCount();
    const float speed = std::sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
    for (std::uint32_t i = 0; i < count; ++i) {
        const std::uint32_t tag = hits[i].tag;
        auto it = cooldown_.find(tag);
        if (it != cooldown_.end() && now - it->second < 1500) continue;
        cooldown_[tag] = now;
        ++stats_.hits;
        const auto kind = static_cast<EntityKind>((tag >> 28) & 0x7u);
        const int entity = static_cast<int>(tag & 0x0FFFFFFFu);
        // Vehicles, objects and (with their parts) peds receive Skate's
        // solved impulses instead.
        if (exchange_ && (kind != EntityKind::Ped || pedParts_)) continue;
        if (!Call<BOOL>(gta::DOES_ENTITY_EXIST, entity) || speed < 1.5f) continue;
        // GTA-side consequence only; Skate already resolved the board's contact.
        const float k = kind == EntityKind::Ped ? 2.5f : 1.2f;
        if (kind == EntityKind::Ped && !Call<BOOL>(gta::IS_PED_RAGDOLL, entity)) {
            Call<BOOL>(gta::SET_PED_TO_RAGDOLL, entity, 1500, 2500, 0, 0, 0, 0);
            Knocked(entity, 0.0f);
        }
        if (kind != EntityKind::Vehicle) {
            Call<void>(gta::APPLY_FORCE_TO_ENTITY, entity, 1, v.x * k, v.y * k, 1.0f, 0.0f, 0.0f, 0.0f, 0, 0, 1, 1, 0, 1);
        }
    }
    if (cooldown_.size() > 512) cooldown_.clear();
}

namespace crime {
bool IsCop(int ped) {
    const int type = Call<int>(gta::GET_PED_TYPE, ped);
    return type == 6 || type == 27 || type == 29; // PED_TYPE_COP, SWAT, ARMY
}
void Report(int crime) {
    Call<void>(gta::REPORT_CRIME, Call<Player>(gta::PLAYER_ID), crime, Call<int>(gta::GET_WANTED_LEVEL_THRESHOLD, 1));
}
} // namespace crime

void DynamicWorld::Knocked(int ped, float impulse) {
    for (const Knock& k : knocked_) if (k.ped == ped) return;
    // Knocking a ped down with the board is an assault (once per knock), but
    // only with a cop there to see it: REPORT_CRIME with the 1-star threshold
    // skips GTA's witness logic and gave stars on an empty street.
    // ponytail: 40 m box, no line of sight; add a LOS check if cops behind walls still report.
    const Vector3 at = Call<Vector3>(gta::GET_ENTITY_COORDS, ped, 1);
    if (crime::IsCop(ped))
        crime::Report(crime::kAssaultCop);
    else if (Call<BOOL>(gta::IS_COP_PED_IN_AREA_3D, at.x - 40.0f, at.y - 40.0f, at.z - 20.0f, at.x + 40.0f, at.y + 40.0f, at.z + 20.0f))
        crime::Report(crime::kAssault);
    if (pedPain_ >= 0) pedspeech::Hurt(ped, std::clamp(impulse / 20.0f, 10.0f, 50.0f), false);
    if (knocked_.size() < 16) knocked_.push_back({ped, GetTickCount(), false});
}

void DynamicWorld::SpeakGetUps(void (*log)(const char*)) {
    const unsigned long now = GetTickCount();
    for (auto it = knocked_.begin(); it != knocked_.end();) {
        const int ped = it->ped;
        const unsigned long age = now - it->at;
        if (!Call<BOOL>(gta::DOES_ENTITY_EXIST, ped) || Call<BOOL>(gta::IS_PED_DEAD_OR_DYING, ped, 1) || age > 15000) {
            it = knocked_.erase(it);
        } else if (!it->up && age > 1000 && !Call<BOOL>(gta::IS_PED_RAGDOLL, ped) && !Call<BOOL>(gta::IS_PED_GETTING_UP, ped)) {
            pedspeech::Pain(ped, 13 /*AUD_DAMAGE_REASON_POST_FALL_GRUNT*/, 0.0f);
            it->up = true, it->at = now;
            ++it;
        } else if (it->up && age > 1200) {
            const std::string said = pedGetUp_.empty() ? std::string() : pedspeech::Say(ped, pedGetUp_);
            if (log && pedLogs_ < 40) {
                ++pedLogs_;
                char line[200];
                std::snprintf(line, sizeof(line), "ped up: entity %d: %s", ped,
                              said.empty() ? "no listed context in this voice" : said.c_str());
                log(line);
            }
            it = knocked_.erase(it);
        } else {
            ++it;
        }
    }
}

void DynamicWorld::PushPed(std::uint32_t tag, const PedPush& p, void (*log)(const char*)) {
    const int component = pedbodies::NearestComponent(parts_, tag, p.point); // logged: where it was struck
    const Vector3 before = Call<Vector3>(gta::GET_ENTITY_VELOCITY, p.entity);
    // The whole ragdoll takes the velocity change Skate's solve gave the ped
    // (impulse / the ped's mass in that solve): a push at the struck part
    // alone barely moves the body.
    Knocked(p.entity, std::sqrt(p.impulse.x * p.impulse.x + p.impulse.y * p.impulse.y + p.impulse.z * p.impulse.z));
    SvVec3 launched{}; // the launch's momentum as applied (launch mode)
    if (p.mass > 0.0f) {
        float dx = p.impulse.x / p.mass, dy = p.impulse.y / p.mass, dz = p.impulse.z / p.mass;
        if (launch_) {
            // Launch mode: scaled, and lifted in proportion to its speed over
            // the ground.
            dx *= launchScale_, dy *= launchScale_, dz *= launchScale_;
            dz += launchLift_ * std::sqrt(dx * dx + dy * dy);
            launched = {dx * p.mass, dy * p.mass, dz * p.mass};
        }
        Call<void>(gta::SET_ENTITY_VELOCITY, p.entity, before.x + dx, before.y + dy, before.z + dz);
    }
    // The turn: Skate's solve gave the posed body w = I^-1 (r x J) about its
    // centre of mass. Each ragdoll part takes its own share of that rigid
    // motion, momentum m_k (w x r_k) at the part (a rigid velocity field the
    // joints already satisfy; the parts' momenta sum to zero, so the launch
    // velocity above is unchanged).
    const float k = launch_ ? launchScale_ : 1.0f;
    SvVec3 w{p.spin.x * k, p.spin.y * k, p.spin.z * k};
    // Launch mode reports no turn (patch 0021 pushes along the contact
    // normal only), so the whole ragdoll would fly off at one velocity. The
    // launch struck the body at a point: it turns as I^-1 (r x J) about the
    // centre of mass too.
    if (launch_ && !(w.x * w.x + w.y * w.y + w.z * w.z > 1e-8f)) {
        w = pedbodies::LaunchTurn(parts_, tag, p.point, launched);
        w = {w.x * launchSpin_, w.y * launchSpin_, w.z * launchSpin_};
    }
    if (w.x * w.x + w.y * w.y + w.z * w.z > 1e-8f) {
        float mass = 0.0f;
        SvVec3 com{};
        for (const SvPedPart& part : parts_) {
            if (part.tag != tag || !(part.mass_kg > 0.0f)) continue;
            mass += part.mass_kg;
            com = {com.x + part.centre.x * part.mass_kg, com.y + part.centre.y * part.mass_kg, com.z + part.centre.z * part.mass_kg};
        }
        if (mass > 0.0f) {
            com = {com.x / mass, com.y / mass, com.z / mass};
            for (const SvPedPart& part : parts_) {
                if (part.tag != tag || !(part.mass_kg > 0.0f)) continue;
                const float rx = part.centre.x - com.x, ry = part.centre.y - com.y, rz = part.centre.z - com.z;
                const float m = part.mass_kg;
                // Force type 1 (APPLY_TYPE_IMPULSE), world direction, at the
                // part (no offset). Not 4 (APPLY_TYPE_TORQUE): a one-frame
                // torque barely registers.
                Call<void>(gta::APPLY_FORCE_TO_ENTITY, p.entity, 1, m * (w.y * rz - w.z * ry), m * (w.z * rx - w.x * rz),
                           m * (w.x * ry - w.y * rx), 0.0f, 0.0f, 0.0f, static_cast<int>(part.component), 0, 1, 0, 0, 1);
            }
        }
    }
    if (log && pedLogs_ < 40) {
        ++pedLogs_;
        char line[240];
        std::snprintf(line, sizeof(line), "ped push: tag %08x %.0f N s at component %d after %u frames waiting for the ragdoll",
                      tag, std::sqrt(p.impulse.x * p.impulse.x + p.impulse.y * p.impulse.y + p.impulse.z * p.impulse.z),
                      component, p.frames);
        log(line);
    }
}

void DynamicWorld::ApplyImpulses(const SvDynamicImpulse* impulses, std::uint32_t count, void (*log)(const char*)) {
    auto logf = [&](const char* fmt, auto... args) {
        if (!log) return;
        char buf[400];
        std::snprintf(buf, sizeof(buf), fmt, args...);
        log(buf);
    };
    // Pushes held for peds GTA has now switched to their ragdoll.
    for (auto it = pedPushes_.begin(); it != pedPushes_.end();) {
        PedPush& p = it->second;
        if (!Call<BOOL>(gta::DOES_ENTITY_EXIST, p.entity)) {
            it = pedPushes_.erase(it);
        } else if (Call<BOOL>(gta::IS_PED_RAGDOLL, p.entity)) {
            PushPed(it->first, p, log);
            it = pedPushes_.erase(it);
        } else if (++p.frames > 30) {
            if (pedLogs_ < 20) {
                ++pedLogs_;
                logf("ped push: tag %08x never entered its ragdoll (can ragdoll %d); %.0f N s dropped", it->first,
                     static_cast<int>(Call<BOOL>(gta::CAN_PED_RAGDOLL, p.entity)),
                     std::sqrt(p.impulse.x * p.impulse.x + p.impulse.y * p.impulse.y + p.impulse.z * p.impulse.z));
            }
            it = pedPushes_.erase(it);
        } else {
            ++it;
        }
    }
    // Per vehicle this frame: summed impulse, and the strongest contact's
    // local offset (for the damage after the loop).
    struct Hit { int entity; SvVec3 sum; float strongest; float ox, oy, oz; };
    Hit hits[16];
    std::uint32_t hitCount = 0;
    for (std::uint32_t i = 0; i < count; ++i) {
        const SvDynamicImpulse& j = impulses[i];
        const auto kind = static_cast<EntityKind>((j.tag >> 28) & 0x7u);
        const int entity = static_cast<int>(j.tag & 0x0FFFFFFFu);
        if (!Call<BOOL>(gta::DOES_ENTITY_EXIST, entity)) continue;
        if (!(std::isfinite(j.impulse.x) && std::isfinite(j.impulse.y) && std::isfinite(j.impulse.z))) continue;
        if (kind == EntityKind::Ped) {
            if (!pedParts_) continue;
            // GTA's own rule (pedbounds.xml UsesRagdollReactionIfShoved): a
            // shoved ped reacts as a ragdoll. GTA switches the ped over on a
            // later frame and a push on the animated ped is lost, so the push
            // is held until the ragdoll is live and then goes to the struck part.
            const float magnitude = std::sqrt(j.impulse.x * j.impulse.x + j.impulse.y * j.impulse.y + j.impulse.z * j.impulse.z);
            auto& p = pedPushes_[j.tag];
            if (!p.entity) p = {entity, {0, 0, 0}, j.point, 0.0f, j.mass_kg, j.tick, 0, {0, 0, 0}};
            // Skate moves a ped it pushed itself until GTA has taken the push
            // over, so every push it reports is real: they add up.
            p.impulse = {p.impulse.x + j.impulse.x, p.impulse.y + j.impulse.y, p.impulse.z + j.impulse.z};
            const SvVec3& w = j.angular_velocity_change;
            if (std::isfinite(w.x) && std::isfinite(w.y) && std::isfinite(w.z)) p.spin = {p.spin.x + w.x, p.spin.y + w.y, p.spin.z + w.z};
            if (magnitude > p.strongest) p.strongest = magnitude, p.point = j.point;
            ++stats_.exchanged;
            ++stats_.pedImpulses;
            if (Call<BOOL>(gta::IS_PED_RAGDOLL, entity)) {
                PushPed(j.tag, p, log);
                pedPushes_.erase(j.tag);
            } else if (Call<BOOL>(gta::CAN_PED_RAGDOLL, entity)) {
                Call<BOOL>(gta::SET_PED_TO_RAGDOLL, entity, 1500, 2500, 0, 0, 0, 0);
            }
            continue;
        }
        // Contact point relative to the entity origin, in its local frame.
        Vector3 fwd{}, right{}, up{}, pos{};
        Call<void>(gta::GET_ENTITY_MATRIX, entity, &fwd, &right, &up, &pos);
        const float rx = j.point.x - pos.x, ry = j.point.y - pos.y, rz = j.point.z - pos.z;
        auto unitDot = [&](const Vector3& a) {
            const float l = std::sqrt(a.x * a.x + a.y * a.y + a.z * a.z);
            return l > 1e-5f ? (rx * a.x + ry * a.y + rz * a.z) / l : 0.0f;
        };
        const float ox = unitDot(right), oy = unitDot(fwd), oz = unitDot(up);
        // Force type 1 (APPLY_TYPE_IMPULSE); world direction; offset in the
        // entity's local frame; not mass-scaled. Not 4 (APPLY_TYPE_TORQUE): a
        // car took 0.136 of an expected 1.875 m/s. A real hit (not resting/rolling contact) also plays GTA's own
        // impact sound for the vehicle or prop (bPlayAudio).
        const float strength = std::sqrt(j.impulse.x * j.impulse.x + j.impulse.y * j.impulse.y + j.impulse.z * j.impulse.z);
        Call<void>(gta::APPLY_FORCE_TO_ENTITY, entity, 1, j.impulse.x, j.impulse.y, j.impulse.z, ox, oy, oz, 0,
                   0 /*bLocalForce*/, 1 /*bLocalOffset*/, 0 /*bScaleByMass*/, strength >= 150.0f ? 1 : 0 /*bPlayAudio*/,
                   1 /*bScaleByTimeWarp*/);
        ++stats_.exchanged;
        if (kind == EntityKind::Vehicle && damage_) {
            Hit* h = nullptr;
            for (std::uint32_t k = 0; k < hitCount; ++k) if (hits[k].entity == entity) h = &hits[k];
            if (!h && hitCount < 16) h = &hits[hitCount++], *h = {entity, {0, 0, 0}, 0.0f, 0, 0, 0};
            if (h) {
                h->sum = {h->sum.x + j.impulse.x, h->sum.y + j.impulse.y, h->sum.z + j.impulse.z};
                const float m = std::sqrt(j.impulse.x * j.impulse.x + j.impulse.y * j.impulse.y + j.impulse.z * j.impulse.z);
                if (m > h->strongest) h->strongest = m, h->ox = ox, h->oy = oy, h->oz = oz;
            }
        }
    }
    for (std::uint32_t k = 0; k < hitCount; ++k) {
        const Hit& h = hits[k];
        const float total = std::sqrt(h.sum.x * h.sum.x + h.sum.y * h.sum.y + h.sum.z * h.sum.z);
        if (total < damageMin_) continue;
        const float before = Call<float>(gta::GET_VEHICLE_BODY_HEALTH, h.entity);
        Call<void>(gta::SET_VEHICLE_DAMAGE, h.entity, h.ox, h.oy, h.oz, total * damageScale_, damageRadius_, 1);
        if (damageLogs_ < 40) {
            ++damageLogs_;
            logf("vehicle damage: entity %d impulse %.0f N s at local (%.2f, %.2f, %.2f): damage %.0f radius %.2f, body health %.0f -> %.0f",
                 h.entity, total, h.ox, h.oy, h.oz, total * damageScale_, damageRadius_, before,
                 Call<float>(gta::GET_VEHICLE_BODY_HEALTH, h.entity));
        }
    }
}

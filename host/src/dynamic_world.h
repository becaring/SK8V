#pragma once
// Moving GTA entities around the skater, handed to the Skate runtime as
// exact collision templates (or their bounded fallback box), and GTA-side
// reactions when the board touches one. Static Los Santos never comes here.
#include "skatev_runtime.h"
#include <cstdint>
#include <string>
#include <unordered_map>
#include <vector>

enum class EntityKind : std::uint32_t { Vehicle = 0, Ped = 1, Object = 2 };

struct DynamicWorldStats {
    std::uint32_t vehicles = 0, peds = 0, objects = 0, hits = 0, debris = 0, doors = 0;
    std::uint32_t exactBounds = 0, missingBounds = 0, exchanged = 0;
    std::uint32_t pedParts = 0, pedImpulses = 0; // peds sent as ragdoll parts; impulses applied to peds
};

// GTA's own crime system for what the held skater does (board hits and
// shots from the board are crimes like any other). REPORT_CRIME ids (nativedb): 11 assault on a civilian, 12 on an
// officer, 13 assault with a deadly weapon, 14 officer shot, 28 shots fired.
// Reports pass the 1-star threshold, so they skip GTA's witness logic: callers
// decide whether anyone saw it (Knocked needs a cop nearby).
namespace crime {
constexpr int kAssault = 11, kAssaultCop = 12, kShootPed = 13, kShootCop = 14, kShotsFired = 28;
bool IsCop(int ped);
void Report(int crime);
} // namespace crime

class DynamicWorld {
public:
    // Collects the bodies (and ped parts) near `center`, excluding the skater.
    void Gather(int self, SvVec3 center, float radius, int presentationEntity = 0);
    const std::vector<SvDynamicBody>& Bodies() const { return bodies_; }
    // GTA-side reactions to board contacts (ragdoll peds, push props).
    void React(const SvDynamicHit* hits, std::uint32_t count, SvVec3 boardVelocity);
    // Litter (cans, bottles, cups) gets no box; the board knocks it aside.
    void KickDebris(SvVec3 board, SvVec3 boardVelocity);
    // Unlocked doors swing away; exact templates remain physically present.
    void OpenDoors(SvVec3 skater);
    // Contact exchange: apply Skate's solved impulses to the GTA entities.
    void ApplyImpulses(const SvDynamicImpulse* impulses, std::uint32_t count, void (*log)(const char*));
    // Peds keep the hit reaction; vehicles/objects use the exchange.
    void SetExchange(bool on) { exchange_ = on; }
    // Peds join the exchange with their own ragdoll parts (no box).
    void SetPedParts(bool on) { pedParts_ = on; }
    // Launch mode (PedLaunch=1): Skate's launch velocity is scaled and gets
    // lift (vertical speed per m/s of horizontal launch speed); `spin` scales
    // the tumble the launch gives about the struck point.
    void SetPedLaunch(bool on, float scale, float lift, float spin) {
        launch_ = on, launchScale_ = scale, launchLift_ = lift, launchSpin_ = spin;
    }
    // Knocked-over peds: pain grunt when struck, a line once back up.
    void SetPedVoices(int pain, const std::string& getUp) { pedPain_ = pain, pedGetUp_ = getUp; }
    // Once a frame: knocked-over peds back on their feet say their line.
    void SpeakGetUps(void (*log)(const char*));
    // Vehicle damage from Skate's impulses (VehicleDamage*): a vehicle whose
    // summed impulse this frame reaches `min` N s takes SET_VEHICLE_DAMAGE of
    // impulse * scale at the strongest contact, dent radius `radius`.
    void SetVehicleDamage(bool on, float scale, float min, float radius) {
        damage_ = on, damageScale_ = scale, damageMin_ = min, damageRadius_ = radius;
    }
    const std::vector<SvPedPart>& PedParts() const { return parts_; }
    const DynamicWorldStats& Stats() const { return stats_; }
    // Load exact template index and baked INSTANCE transforms beside cache.
    // Legacy model-only .props.txt is deliberately not used for suppression.
    static std::size_t LoadPropTemplates(const std::string& worldCache);

    static std::uint32_t Tag(EntityKind kind, int handle) {
        return (static_cast<std::uint32_t>(kind) << 28) | (static_cast<std::uint32_t>(handle) & 0x0FFFFFFFu);
    }

private:
    bool AddBody(int entity, EntityKind kind);
    struct Debris { int entity; SvVec3 center; };
    struct Candidate { float d2; int entity; EntityKind kind; };
    std::vector<Candidate> nearby_;
    std::vector<SvDynamicBody> bodies_;
    std::vector<Debris> debris_;
    struct Door { std::uint32_t hash; SvVec3 hinge; SvVec3 forward; };
    std::vector<Door> doors_;
    std::vector<int> scratch_;
    std::unordered_map<std::uint32_t, unsigned long> cooldown_;
    bool exchange_ = false;
    bool pedParts_ = false;
    bool launch_ = false;
    float launchScale_ = 1.0f, launchLift_ = 0.0f, launchSpin_ = 1.0f;
    int pedPain_ = 0;
    std::string pedGetUp_;
    // Peds knocked over: once up, GTA's post-fall grunt, then a line.
    struct Knock { int ped; unsigned long at; bool up; };
    std::vector<Knock> knocked_;
    void Knocked(int ped, float impulse);
    bool damage_ = true;
    float damageScale_ = 0.5f, damageMin_ = 200.0f, damageRadius_ = 150.0f;
    std::uint32_t damageLogs_ = 0;
    std::vector<SvPedPart> parts_;
    // Ped pushes waiting for GTA to switch the ped to its ragdoll (a push on
    // the animated ped is lost): summed per ped, applied once it ragdolls.
    struct PedPush { int entity; SvVec3 impulse; SvVec3 point; float strongest; float mass; std::uint64_t tick; unsigned frames; SvVec3 spin; };
    std::unordered_map<std::uint32_t, PedPush> pedPushes_;
    unsigned pedLogs_ = 0;
    void PushPed(std::uint32_t tag, const PedPush& push, void (*log)(const char*));
    DynamicWorldStats stats_;
};

#pragma once
#include "skatev_runtime.h"
#include <windows.h>
#include <string>

class RuntimeBridge {
public:
    RuntimeBridge() = default;
    ~RuntimeBridge() = default;
    RuntimeBridge(const RuntimeBridge&) = delete;
    RuntimeBridge& operator=(const RuntimeBridge&) = delete;

    // Returns false with `error` set; the DLL stays loaded once it was.
    // Every export below is required.
    bool Load(const wchar_t* dllPath, const std::string& dataRoot, const std::string& worldCache,
              const std::string& logPath, std::uint32_t skaterTriangles, std::uint32_t presentationFlags,
              std::string& error);
    // Releases the runtime handle. The DLL is never unloaded: its worker
    // thread exits on its own once the handle's job channel closes.
    void Release();
    bool Deactivate();
    bool Step(const SvInput& input);
    bool Output(SvOutput& out) const;
    bool Score(SvScoreState& out) const;
    std::string StatusText() const;
    std::uint32_t SetDynamicBodies(const SvDynamicBody* bodies, std::uint32_t count);
    // What Skate's audio engine is playing (text lines).
    std::string AudioDebugText() const;
    // Posed ragdoll parts of nearby peds.
    std::uint32_t SetPedParts(const SvPedPart* parts, std::uint32_t count);
    // Contact exchange: impulses on host entities to apply.
    std::uint32_t DynamicImpulses(SvDynamicImpulse* out, std::uint32_t capacity) const;
    std::uint32_t DynamicHits(SvDynamicHit* out, std::uint32_t capacity) const;
    std::uint32_t SkaterMesh(SvColorTri* out, std::uint32_t capacity) const;
    std::string TrickHistory() const;
    bool SetCharacter(const SvCharacter* character);
    bool ConfigureQuirk(const SvQuirkConfig& config);
    bool TriggerQuirk(std::uint32_t quirk);
    bool QuirkState(SvQuirkState& out) const;
    // Ped presentation: bone world matrices (16 floats each); returns bones written.
    std::uint32_t CharacterPose(float* out, std::uint32_t capacityBones) const;
    bool BoardPose(SvBoardPose& out) const;
    bool IsLoaded() const { return runtime_ != nullptr; }
    void* Handle() const { return runtime_; } // sv_create handle (ABI 8 audio/HUD lookups)
    bool LifecycleTrack(const SvLifecycleTrack& track);
    bool LifecycleEnter(const SvSpawn& spawn);
    bool LifecycleState(SvLifecycleState& out) const;
    bool MapStatesSet(const std::uint32_t* hashes, std::uint32_t count);
    bool SetHallOfMeat(bool enabled);
    bool SetBailLimit(float seconds);
    bool SetAirLimit(float seconds);
    bool SetLipRule(bool enabled);
    bool SetVerboseLog(bool enabled);
    bool SetSkitchStandoff(float metres);
    int SkitchVehicle() const;
    bool SetPhysicsLevel(std::uintptr_t table, std::uintptr_t imageLo, std::uintptr_t imageHi);
    bool PlayLine(const std::string& utf8Path);
    std::uint32_t ApiVersion() const { return apiVersion_; }

private:
    HMODULE module_ = nullptr;
    void* runtime_ = nullptr;
    std::uint32_t apiVersion_ = 0;
    SvDestroyFn destroy_ = nullptr;
    SvDeactivateFn deactivate_ = nullptr;
    SvStepFn step_ = nullptr;
    SvGetOutputFn getOutput_ = nullptr;
    SvGetScoreStateFn getScore_ = nullptr;
    SvGetStatusTextFn getStatusText_ = nullptr;
    SvSetDynamicBodiesFn setDynamicBodies_ = nullptr;
    SvGetDynamicImpulsesFn getDynamicImpulses_ = nullptr;
    SvSetPedPartsFn setPedParts_ = nullptr;
    std::uint32_t(__cdecl* audioDebugText_)(void*, std::uint8_t*, std::uint32_t) = nullptr;
    SvGetDynamicHitsFn getDynamicHits_ = nullptr;
    SvGetSkaterMeshFn getSkaterMesh_ = nullptr;
    SvGetTrickHistoryFn getTrickHistory_ = nullptr;
    SvSetCharacterFn setCharacter_ = nullptr;
    SvConfigureQuirkFn configureQuirk_ = nullptr;
    SvTriggerQuirkFn triggerQuirk_ = nullptr;
    SvGetQuirkStateFn getQuirkState_ = nullptr;
    SvGetCharacterPoseFn getCharacterPose_ = nullptr;
    SvGetBoardPoseFn getBoardPose_ = nullptr;
    SvLifecycleTrackFn lifecycleTrack_ = nullptr;
    SvLifecycleEnterFn lifecycleEnter_ = nullptr;
    SvLifecycleGetStateFn lifecycleGetState_ = nullptr;
    SvMapStatesSetFn mapStatesSet_ = nullptr;
    SvSetHallOfMeatFn setHallOfMeat_ = nullptr;
    SvSetBailLimitFn setBailLimit_ = nullptr;
    SvSetAirLimitFn setAirLimit_ = nullptr;
    SvSetLipRuleFn setLipRule_ = nullptr;
    SvSetVerboseLogFn setVerboseLog_ = nullptr;
    SvSetSkitchStandoffFn setSkitchStandoff_ = nullptr;
    SvSkitchVehicleFn skitchVehicle_ = nullptr;
    SvSetPhysicsLevelFn setPhysicsLevel_ = nullptr;
    SvPlayLineFn playLine_ = nullptr;
};

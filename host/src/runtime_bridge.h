#pragma once
#include "skatev_runtime.h"
#include <windows.h>
#include <string>
#include <vector>

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
    // Keyboard and mouse (optional exports): false when the runtime has none.
    bool KeyboardConfigure(const std::string& utf8KeysFile, std::uint32_t& problems);
    bool KeyboardPad(float mouseX, float mouseY, float dt, bool active, SvPad& out);
    std::string KeyboardKey(const char* action);     // the key bound, "" when none
    bool KeyboardBind(const char* action, int vk);    // rebinds and saves SkateVKeys.txt
    bool KeyboardLayout(const char* name);            // "standard" or "skate": every key at once
    std::string KeyboardHints(bool onFoot); // "key\twhat" lines; empty without the export
    bool SetDifficulty(int index);
    bool SetCameraType(int type);  // 0 Low, 1 High
    bool SetGestures(const int (&up_down_left_right)[4]);  // catalog indices; all < 0: off
    bool SetPosture(int profile);                // 0 none, 1 stiff, 2 slouch, 3 buff
    bool SetRidingStyle(int style);              // 0 none, 1 Loose, 2 Gonzo, 3 Aggressive
    bool SetEquipment(float truck, float wheel); // 0..1 each
    bool SetTrainer(const float (&tuning)[11], bool holdFakie); // false: runtime has none
    bool SetHippieJump(float scale); // false: runtime has none
    // The board shop (optional exports; false / empty when the runtime has none).
    bool HasShop() const { return shopStart_ && shopText_; }
    bool ShopStart(const std::string& assets, const std::string& garages);
    bool ShopCharacter(const std::string& who);
    bool ShopOpen(int tier);
    void ShopClose();
    SvShopResult ShopInput(int input, long long cash);
    std::string ShopText(int which);
    bool ShopRestPose(const float (&at)[16], SvBoardPose& out);
    bool ShopRide(const char* state, float speed, float dt); // true: a part wore through
    // The boards on show in a store: up to `max` entity matrices (16 floats each) and their models.
    int ShopDisplay(int tier, const float (&interior)[3], const float (&eye)[3], const float* clerk, unsigned day,
                    SvProbeFn probe, void* ctx, float* out, int max, std::vector<std::string>& models);
    bool SetVerboseLog(bool enabled);
    bool SetSkitchStandoff(float metres);
    int SkitchVehicle() const;
    bool GroundAt(const SvVec3& point, float depth, SvGround& out) const;
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
    SvKeyboardConfigureFn keyboardConfigure_ = nullptr;
    SvKeyboardPadFn keyboardPad_ = nullptr;
    SvKeyboardHintsFn keyboardHints_ = nullptr;
    SvKeyboardKeyFn keyboardKey_ = nullptr;
    SvKeyboardLayoutFn keyboardLayout_ = nullptr;
    SvKeyboardBindFn keyboardBind_ = nullptr;
    SvSetDifficultyFn setDifficulty_ = nullptr;
    SvSetCameraTypeFn setCameraType_ = nullptr;
    SvSetGesturesFn setGestures_ = nullptr;
    SvSetPostureFn setPosture_ = nullptr;
    SvSetRidingStyleFn setRidingStyle_ = nullptr;
    SvSetEquipmentFn setEquipment_ = nullptr;
    SvSetTrainerFn setTrainer_ = nullptr;
    SvSetHippieJumpFn setHippieJump_ = nullptr;
    SvShopStartFn shopStart_ = nullptr;
    SvShopCharacterFn shopCharacter_ = nullptr;
    SvShopOpenFn shopOpen_ = nullptr;
    SvShopCloseFn shopClose_ = nullptr;
    SvShopInputFn shopInput_ = nullptr;
    SvShopTextFn shopText_ = nullptr;
    SvShopRestPoseFn shopRestPose_ = nullptr;
    SvShopDisplayFn shopDisplay_ = nullptr;
    SvShopRideFn shopRide_ = nullptr;
    SvSetVerboseLogFn setVerboseLog_ = nullptr;
    SvSetSkitchStandoffFn setSkitchStandoff_ = nullptr;
    SvSkitchVehicleFn skitchVehicle_ = nullptr;
    SvGroundAtFn groundAt_ = nullptr;
    SvSetPhysicsLevelFn setPhysicsLevel_ = nullptr;
    SvPlayLineFn playLine_ = nullptr;
};

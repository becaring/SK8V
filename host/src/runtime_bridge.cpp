#include "runtime_bridge.h"
#include <type_traits>
#include <cstring>

bool RuntimeBridge::Load(const wchar_t* dllPath, const std::string& dataRoot, const std::string& worldCache,
                         const std::string& logPath, std::uint32_t skaterTriangles, std::uint32_t presentationFlags, std::string& error) {
    if (!module_) module_ = LoadLibraryW(dllPath);
    if (!module_) {
        error = "LoadLibrary(SkateVRuntime.dll) failed, error " + std::to_string(GetLastError());
        return false;
    }
    std::string missing;
    auto sym = [&](auto& fn, const char* name) {
        fn = reinterpret_cast<std::remove_reference_t<decltype(fn)>>(GetProcAddress(module_, name));
        if (!fn) missing += missing.empty() ? name : std::string(", ") + name;
    };
    SvApiVersionFn api = nullptr;
    SvCreateFn create = nullptr;
    sym(api, "sv_api_version");
    sym(create, "sv_create");
    sym(destroy_, "sv_destroy");
    sym(deactivate_, "sv_deactivate");
    sym(step_, "sv_step");
    sym(getOutput_, "sv_get_output");
    sym(getScore_, "sv_get_score_state");
    sym(getStatusText_, "sv_get_status_text");
    sym(setDynamicBodies_, "sv_set_dynamic_bodies");
    sym(getDynamicImpulses_, "sv_get_dynamic_impulses");
    sym(setPedParts_, "sv_set_ped_parts");
    sym(audioDebugText_, "sv_audio_debug_text");
    sym(getDynamicHits_, "sv_get_dynamic_hits");
    sym(getSkaterMesh_, "sv_get_skater_mesh");
    sym(getTrickHistory_, "sv_get_trick_history");
    sym(setCharacter_, "sv_set_character");
    sym(configureQuirk_, "sv_configure_quirk");
    sym(triggerQuirk_, "sv_trigger_quirk");
    sym(getQuirkState_, "sv_get_quirk_state");
    sym(getCharacterPose_, "sv_get_character_pose");
    sym(getBoardPose_, "sv_get_board_pose");
    sym(lifecycleTrack_, "sv_lifecycle_track");
    sym(lifecycleEnter_, "sv_lifecycle_enter");
    sym(lifecycleGetState_, "sv_lifecycle_get_state");
    sym(mapStatesSet_, "sv_map_states_set");
    sym(setHallOfMeat_, "sv_set_hall_of_meat");
    sym(setBailLimit_, "sv_set_bail_limit");
    sym(setAirLimit_, "sv_set_air_limit");
    sym(setLipRule_, "sv_set_lip_rule");
    sym(setDifficulty_, "sv_set_difficulty");
    sym(setCameraType_, "sv_set_camera_type");
    sym(setGestures_, "sv_set_gestures");
    sym(setPosture_, "sv_set_posture");
    sym(setRidingStyle_, "sv_set_riding_style");
    sym(setEquipment_, "sv_set_equipment");
    // Optional: trainer tuning.
    setTrainer_ = reinterpret_cast<SvSetTrainerFn>(GetProcAddress(module_, "sv_set_trainer"));
    setHippieJump_ = reinterpret_cast<SvSetHippieJumpFn>(GetProcAddress(module_, "sv_set_hippie_jump"));
    // Optional: the board shop.
    auto optional = [&](auto& fn, const char* name) {
        fn = reinterpret_cast<std::remove_reference_t<decltype(fn)>>(GetProcAddress(module_, name));
    };
    optional(shopStart_, "sv_shop_start");
    optional(shopCharacter_, "sv_shop_character");
    optional(shopOpen_, "sv_shop_open");
    optional(shopClose_, "sv_shop_close");
    optional(shopInput_, "sv_shop_input");
    optional(shopText_, "sv_shop_text");
    optional(shopRestPose_, "sv_shop_rest_pose");
    optional(shopDisplay_, "sv_shop_display");
    optional(shopRide_, "sv_shop_ride");
    sym(setVerboseLog_, "sv_set_verbose_log");
    sym(setSkitchStandoff_, "sv_set_skitch_standoff");
    sym(skitchVehicle_, "sv_skitch_vehicle");
    sym(groundAt_, "sv_ground_at");
    sym(setPhysicsLevel_, "sv_set_physics_level");
    sym(playLine_, "sv_play_line");
    // Optional: keyboard and mouse.
    keyboardConfigure_ = reinterpret_cast<SvKeyboardConfigureFn>(GetProcAddress(module_, "sv_keyboard_configure"));
    keyboardPad_ = reinterpret_cast<SvKeyboardPadFn>(GetProcAddress(module_, "sv_keyboard_pad"));
    keyboardHints_ = reinterpret_cast<SvKeyboardHintsFn>(GetProcAddress(module_, "sv_keyboard_hints"));
    keyboardKey_ = reinterpret_cast<SvKeyboardKeyFn>(GetProcAddress(module_, "sv_keyboard_key"));
    keyboardLayout_ = reinterpret_cast<SvKeyboardLayoutFn>(GetProcAddress(module_, "sv_keyboard_layout"));
    keyboardBind_ = reinterpret_cast<SvKeyboardBindFn>(GetProcAddress(module_, "sv_keyboard_bind"));
    if (!missing.empty()) {
        error = "SkateVRuntime.dll is missing required ABI exports: " + missing;
        return false;
    }
    apiVersion_ = api();
    if (apiVersion_ != SKATEV_ABI_VERSION) {
        error = "runtime ABI " + std::to_string(apiVersion_) + ", host expects " + std::to_string(SKATEV_ABI_VERSION);
        return false;
    }

    SvCreateInfo info{};
    info.size = sizeof(info);
    info.abi_version = SKATEV_ABI_VERSION;
    info.data_root_utf8 = dataRoot.c_str();
    info.world_cache_utf8 = worldCache.c_str();
    info.log_path_utf8 = logPath.empty() ? nullptr : logPath.c_str();
    info.skater_triangle_budget = skaterTriangles;
    info.presentation_flags = presentationFlags;
    runtime_ = create(&info);
    if (!runtime_) {
        error = "sv_create rejected the configuration (check DataRoot / WorldCache)";
        return false;
    }
    return true;
}

void RuntimeBridge::Release() {
    if (runtime_) destroy_(runtime_);
    runtime_ = nullptr;
}

bool RuntimeBridge::BoardPose(SvBoardPose& out) const {
    out = {};
    out.size = sizeof(out);
    return runtime_ && getBoardPose_(runtime_, &out) != 0;
}

std::uint32_t RuntimeBridge::SetDynamicBodies(const SvDynamicBody* bodies, std::uint32_t count) {
    return runtime_ ? setDynamicBodies_(runtime_, bodies, count) : 0;
}

std::string RuntimeBridge::AudioDebugText() const {
    if (!runtime_) return {};
    char buf[4096];
    const std::uint32_t n = audioDebugText_(runtime_, reinterpret_cast<std::uint8_t*>(buf), sizeof(buf));
    return std::string(buf, n);
}

std::uint32_t RuntimeBridge::SetPedParts(const SvPedPart* parts, std::uint32_t count) {
    return runtime_ ? setPedParts_(runtime_, parts, count) : 0;
}

std::uint32_t RuntimeBridge::DynamicImpulses(SvDynamicImpulse* out, std::uint32_t capacity) const {
    if (!runtime_ || capacity == 0) return 0;
    for (std::uint32_t i = 0; i < capacity; ++i) out[i] = SvDynamicImpulse{static_cast<std::uint32_t>(sizeof(SvDynamicImpulse))};
    return getDynamicImpulses_(runtime_, out, capacity);
}

bool RuntimeBridge::Deactivate() {
    return runtime_ && deactivate_(runtime_) != 0;
}

bool RuntimeBridge::Step(const SvInput& input) {
    return runtime_ && step_(runtime_, &input) != 0;
}

bool RuntimeBridge::Output(SvOutput& out) const {
    out = {};
    out.size = sizeof(out);
    return runtime_ && getOutput_(runtime_, &out) != 0;
}

bool RuntimeBridge::Score(SvScoreState& out) const {
    out = {};
    out.size = sizeof(out);
    return runtime_ && getScore_(runtime_, &out) != 0;
}

std::string RuntimeBridge::StatusText() const {
    if (!runtime_) return "runtime not loaded";
    char buf[256]{};
    getStatusText_(runtime_, buf, sizeof(buf));
    return buf;
}

std::uint32_t RuntimeBridge::DynamicHits(SvDynamicHit* out, std::uint32_t capacity) const {
    return runtime_ ? getDynamicHits_(runtime_, out, capacity) : 0;
}

std::uint32_t RuntimeBridge::SkaterMesh(SvColorTri* out, std::uint32_t capacity) const {
    return runtime_ ? getSkaterMesh_(runtime_, out, capacity) : 0;
}

std::string RuntimeBridge::TrickHistory() const {
    if (!runtime_) return {};
    char buf[512]{};
    getTrickHistory_(runtime_, buf, sizeof(buf));
    return buf;
}

std::uint32_t RuntimeBridge::CharacterPose(float* out, std::uint32_t capacityBones) const {
    return runtime_ ? getCharacterPose_(runtime_, out, capacityBones) : 0;
}

bool RuntimeBridge::SetCharacter(const SvCharacter* character) {
    return runtime_ && setCharacter_(runtime_, character) != 0;
}

bool RuntimeBridge::ConfigureQuirk(const SvQuirkConfig& config) {
    return runtime_ && configureQuirk_(runtime_, &config) != 0;
}

bool RuntimeBridge::TriggerQuirk(std::uint32_t quirk) {
    return runtime_ && triggerQuirk_(runtime_, quirk) != 0;
}

bool RuntimeBridge::QuirkState(SvQuirkState& out) const {
    out = {};
    out.size = sizeof(out);
    return runtime_ && getQuirkState_(runtime_, &out) != 0;
}

bool RuntimeBridge::LifecycleTrack(const SvLifecycleTrack& track) {
    return runtime_ && lifecycleTrack_(runtime_, &track) != 0;
}

bool RuntimeBridge::LifecycleEnter(const SvSpawn& spawn) {
    return runtime_ && lifecycleEnter_(runtime_, &spawn) != 0;
}

bool RuntimeBridge::LifecycleState(SvLifecycleState& out) const {
    out = {};
    out.size = sizeof(out);
    return runtime_ && lifecycleGetState_(runtime_, &out) != 0;
}

bool RuntimeBridge::MapStatesSet(const std::uint32_t* hashes, std::uint32_t count) {
    return runtime_ && mapStatesSet_(runtime_, hashes, count) != 0;
}

bool RuntimeBridge::SetHallOfMeat(bool enabled) {
    return runtime_ && setHallOfMeat_(runtime_, enabled ? 1u : 0u) != 0;
}

bool RuntimeBridge::SetBailLimit(float seconds) {
    return runtime_ && setBailLimit_(runtime_, seconds) != 0;
}

bool RuntimeBridge::SetAirLimit(float seconds) {
    return runtime_ && setAirLimit_(runtime_, seconds) != 0;
}

bool RuntimeBridge::KeyboardConfigure(const std::string& utf8KeysFile, std::uint32_t& problems) {
    if (!runtime_ || !keyboardConfigure_) return false;
    problems = keyboardConfigure_(runtime_, utf8KeysFile.c_str());
    return true;
}

std::string RuntimeBridge::KeyboardKey(const char* action) {
    if (!runtime_ || !keyboardKey_) return {};
    char text[64]{};
    keyboardKey_(runtime_, action, text, sizeof(text));
    return text;
}

bool RuntimeBridge::KeyboardLayout(const char* name) {
    return runtime_ && keyboardLayout_ && keyboardLayout_(runtime_, name) != 0;
}

bool RuntimeBridge::KeyboardBind(const char* action, int vk) {
    return runtime_ && keyboardBind_ && keyboardBind_(runtime_, action, static_cast<std::uint32_t>(vk)) != 0;
}

std::string RuntimeBridge::KeyboardHints(bool onFoot) {
    if (!runtime_ || !keyboardHints_) return {};
    const std::uint32_t need = keyboardHints_(runtime_, onFoot ? 1u : 0u, nullptr, 0);
    std::string text(need, '\0');
    keyboardHints_(runtime_, onFoot ? 1u : 0u, text.data(), need);
    text.resize(need ? need - 1 : 0);
    return text;
}

bool RuntimeBridge::KeyboardPad(float mouseX, float mouseY, float dt, bool active, SvPad& out) {
    return runtime_ && keyboardPad_ && keyboardPad_(runtime_, mouseX, mouseY, dt, active ? 1u : 0u, &out) != 0;
}

bool RuntimeBridge::SetLipRule(bool enabled) {
    return runtime_ && setLipRule_(runtime_, enabled ? 1u : 0u) != 0;
}

// The engine takes all four directions or none: a direction left empty gets
// gesture 0 here and the ASI keeps its d-pad press from Skate (BoardControls).
bool RuntimeBridge::SetGestures(const int (&g)[4]) {
    if (!runtime_ || !setGestures_) return false;
    const bool any = g[0] >= 0 || g[1] >= 0 || g[2] >= 0 || g[3] >= 0;
    const auto at = [&](int i) { return static_cast<std::uint32_t>(!any ? 37 : g[i] < 0 ? 0 : g[i]); };
    return setGestures_(runtime_, at(0), at(1), at(2), at(3)) != 0;
}

bool RuntimeBridge::SetPosture(int profile) {
    return runtime_ && setPosture_ && profile >= 0 && setPosture_(runtime_, static_cast<std::uint32_t>(profile)) != 0;
}

bool RuntimeBridge::SetRidingStyle(int style) {
    return runtime_ && setRidingStyle_ && style >= 0 && setRidingStyle_(runtime_, static_cast<std::uint32_t>(style)) != 0;
}

bool RuntimeBridge::SetHippieJump(float scale) {
    return runtime_ && setHippieJump_ && setHippieJump_(runtime_, scale) != 0;
}

bool RuntimeBridge::SetTrainer(const float (&tuning)[11], bool holdFakie) {
    return runtime_ && setTrainer_ && setTrainer_(runtime_, tuning, holdFakie ? 1u : 0u) != 0;
bool RuntimeBridge::ShopStart(const std::string& assets, const std::string& garages) {
    return runtime_ && shopStart_ && shopStart_(runtime_, assets.c_str(), garages.c_str()) != 0;
}

bool RuntimeBridge::ShopCharacter(const std::string& who) {
    return runtime_ && shopCharacter_ && shopCharacter_(runtime_, who.c_str()) != 0;
}

bool RuntimeBridge::ShopOpen(int tier) {
    return runtime_ && shopOpen_ && shopOpen_(runtime_, static_cast<std::uint32_t>(tier)) != 0;
}

void RuntimeBridge::ShopClose() {
    if (runtime_ && shopClose_) shopClose_(runtime_);
}

SvShopResult RuntimeBridge::ShopInput(int input, long long cash) {
    SvShopResult out{1, 0};
    if (runtime_ && shopInput_) shopInput_(runtime_, static_cast<std::uint32_t>(input), cash, &out);
    return out;
}

std::string RuntimeBridge::ShopText(int which) {
    if (!runtime_ || !shopText_) return {};
    std::string text(4096, '\0');
    std::uint32_t need = shopText_(runtime_, static_cast<std::uint32_t>(which), text.data(), static_cast<std::uint32_t>(text.size()));
    if (need > text.size()) {
        text.assign(need, '\0');
        need = shopText_(runtime_, static_cast<std::uint32_t>(which), text.data(), static_cast<std::uint32_t>(text.size()));
    }
    text.resize(need ? need - 1 : 0);
    return text;
}

int RuntimeBridge::ShopDisplay(int tier, const float (&interior)[3], const float (&eye)[3], const float* clerk, unsigned day,
                               SvProbeFn probe, void* ctx, float* out, int max, std::vector<std::string>& models) {
    models.clear();
    if (!runtime_ || !shopDisplay_ || max <= 0) return 0;
    std::string text(8192, '\0');
    const std::uint32_t n = shopDisplay_(runtime_, static_cast<std::uint32_t>(tier), interior, eye, clerk, day, probe, ctx, out,
                                         static_cast<std::uint32_t>(max), text.data(), static_cast<std::uint32_t>(text.size()));
    text.resize(std::strlen(text.c_str()));
    std::size_t at = 0;
    while (models.size() < n && at < text.size()) {
        const std::size_t end = text.find('\n', at);
        models.push_back(text.substr(at, end == std::string::npos ? std::string::npos : end - at));
        at = end == std::string::npos ? text.size() : end + 1;
    }
    return static_cast<int>(models.size() == n ? n : 0);
}

bool RuntimeBridge::ShopRide(const char* state, float speed, float dt) {
    return runtime_ && shopRide_ && shopRide_(runtime_, state, speed, dt) != 0;
}

bool RuntimeBridge::ShopRestPose(const float (&at)[16], SvBoardPose& out) {
    return runtime_ && shopRestPose_ && shopRestPose_(runtime_, at, &out) != 0;
}

bool RuntimeBridge::SetEquipment(float truck, float wheel) {
    return runtime_ && setEquipment_ && setEquipment_(runtime_, truck, wheel) != 0;
}

bool RuntimeBridge::SetCameraType(int type) {
    return runtime_ && setCameraType_ && type >= 0 && setCameraType_(runtime_, static_cast<std::uint32_t>(type)) != 0;
}

bool RuntimeBridge::SetDifficulty(int index) {
    return runtime_ && index >= 0 && setDifficulty_(runtime_, static_cast<std::uint32_t>(index)) != 0;
}

bool RuntimeBridge::SetVerboseLog(bool enabled) {
    return runtime_ && setVerboseLog_(runtime_, enabled ? 1u : 0u) != 0;
}

bool RuntimeBridge::SetSkitchStandoff(float metres) {
    return runtime_ && setSkitchStandoff_(runtime_, metres) != 0;
}

int RuntimeBridge::SkitchVehicle() const {
    return runtime_ ? static_cast<int>(skitchVehicle_(runtime_)) : 0;
}

bool RuntimeBridge::GroundAt(const SvVec3& point, float depth, SvGround& out) const {
    out = {};
    out.size = sizeof(out);
    return runtime_ && groundAt_(runtime_, &point, depth, &out) != 0;
}

bool RuntimeBridge::SetPhysicsLevel(std::uintptr_t table, std::uintptr_t imageLo, std::uintptr_t imageHi) {
    return runtime_ && setPhysicsLevel_(runtime_, table, imageLo, imageHi) != 0;
}

bool RuntimeBridge::PlayLine(const std::string& utf8Path) {
    return runtime_ && playLine_(runtime_, utf8Path.c_str()) != 0;
}

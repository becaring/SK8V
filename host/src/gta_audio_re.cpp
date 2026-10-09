#include "gta_audio_re.h"
#include "image_code_ranges.h"

#include <windows.h>
#include <cstdio>
#include <cstring>
#include <vector>

namespace gtare {
namespace {

// Signatures: byte-identical to local/re/audio_bridge.py (FUNCS / SITES).
struct Item {
    const char* name;
    std::uintptr_t rva;
    const char* signature;
    bool required;
};

constexpr Item kInitParamsCtor{"audSoundInitParams::ctor", 0x33B7B8,
                               "0F 57 C0 48 8D 41 10 BA 03 00 00 00 45 33 C0 0F 29 01 FF CA 44 89 00", true};
constexpr Item kCreateSoundByName{
    "audEntity::CreateSound_PersistentReference(name)", 0x131696C,
    "48 89 5C 24 08 48 89 74 24 10 57 48 83 EC 20 48 8B C2 48 8B D9 33 D2 48 8B C8 49 8B F9 49 8B F0 E8 ? ? ? ? "
    "4C 8B CF 4C 8B C6 8B D0 48 8B CB 48 8B 5C 24 30 48 8B 74 24 38 48 83 C4 20 5F E9 1C FF FF FF",
    true};
constexpr Item kCreateSoundByHash{
    "audEntity::CreateSound_PersistentReference(hash)", 0x13168CC,
    "48 8B C4 48 89 58 08 48 89 68 10 48 89 70 18 48 89 78 20 41 56 48 81 EC D0 00 00 00 4D 8B F1 49 8B F8 8B EA "
    "48 8B F1 81 FA 16 CF 8F E3 74 4F B8 FF FF 00 00 66 39 41 08 74 44 4D 85 C0 74 3F",
    true};
constexpr Item kInitStreamPlayer{
    "audExternalStreamSound::InitStreamPlayer", 0x132DB00,
    "44 0F B6 51 62 8B 89 B4 00 00 00 48 8B 05 ? ? ? ? 0F AF 0D ? ? ? ? 4D 69 D2 D0 34 00 00 "
    "49 03 8C 02 C0 34 00 00 74 12 F0 FF 42 50 48 89 11 44 89 41 08 44 89 49 0C B0 01 C3",
    true};
constexpr Item kPrepareAndPlay{
    "audSound::PrepareAndPlay", 0x132E210,
    "48 89 5C 24 08 48 89 6C 24 10 48 89 74 24 18 57 48 83 EC 20 F6 81 89 00 00 00 04 41 8B D9 41 8A E8 48 8B F2",
    true};
constexpr Item kStopAndForget{
    "audSound::StopAndForget", 0x1334F68,
    "4C 8B C9 0F B6 89 80 00 00 00 81 F9 FF 00 00 00 74 24 45 0F B6 41 62 0F AF 0D ? ? ? ? 48 8B 05 ? ? ? ? "
    "4D 69 C0 D0 34 00 00 49 03 8C 00 C0 34 00 00",
    true};
constexpr Item kEnvCreate{
    "naEnvironmentGroup::Create", 0x4709CC,
    "48 89 5C 24 08 57 48 83 EC 20 8B 0D ? ? ? ? 65 48 8B 04 25 58 00 00 00 BA ? ? 00 00 48 8B 04 C8 33 DB F6 04 02 01",
    true};
constexpr Item kEnvInit{"naEnvironmentGroup::Init", 0x48EFE8,
                        "48 89 5C 24 08 48 89 74 24 10 57 48 83 EC 30 0F 29 74 24 20 41 8B D9 48 8B F9 0F 28 F2 E8 ? ? ? ? "
                        "80 A7 18 01 00 00 FC",
                        true};
constexpr Item kEnvSetPosition{"naEnvironmentGroup::SetPosition", 0x4A87B0,
                               "0F 28 12 0F 28 C2 0F 28 CA F3 0F 11 51 70 0F C6 C2 55 0F C6 CA AA 0F C6 D2 FF F3 0F 11 51 7C",
                               true};
constexpr Item kEnvSetInterior{"naEnvironmentGroup::SetInteriorLocationFromEntity", 0x4A7E60,
                               "48 85 D2 74 27 53 48 83 EC 20 48 8B C2 48 8B D9 48 8D 54 24 38 48 8B C8 E8 ? ? ? ? 48 8B CB 8B 10 E8",
                               true};
constexpr Item kEnvSetLocation{"naEnvironmentGroup::SetInteriorLocation", 0x4A80E0,
                               "89 54 24 10 53 48 83 EC 20 80 89 18 01 00 00 80 80 61 4D FB 48 8B D9 3B 91 F4 00 00 00 74 07 "
                               "80 89 1A 01 00 00 01 89 91 F4 00 00 00 48 8D 4C 24 38 E8",
                               false};
constexpr Item kEnvSetSettings{"naEnvironmentGroup::SetInteriorSettings", 0x4A8164,
                               "48 85 D2 74 3C 4D 85 C0 74 37 41 8B 40 14 89 81 B4 00 00 00 41 8B 40 18 89 81 B8 00 00 00 "
                               "41 8B 40 1C 89 81 BC 00 00 00 80 49 4D 04 80 89 18 01 00 00 80 48 89 91 F8 00 00 00 4C 89 81 00 01 00 00 C3",
                               false};
constexpr Item kSiteEnvUpdateReverb{"naEnvironmentGroup update: reverb sends from +0xB4", 0x4CF919,
                                    "0F B6 87 14 01 00 00 F3 0F 10 15 ? ? ? ? F3 0F 10 8F B4 00 00 00", false};
constexpr Item kRequestedSetPosition{"audRequestedSettings::SetPosition", 0x13123E8,
                                     "48 83 EC 28 8B 05 ? ? ? ? 0F 28 02 4C 8B C9 48 FF C0 48 03 C0 0F 29 04 C1", true};
constexpr Item kEntityTracker{"CEntity -> audio tracker", 0x9341B4,
                              "8A 41 28 3C 04 75 08 48 8D 81 F8 10 00 00 C3 3C 03 75 08 48 8D 81 A0 09 00 00 C3 3C 05 75 08 "
                              "48 8D 81 28 03 00 00 C3",
                              true};
constexpr Item kGetCategoryPtr{"audCategoryManager::GetCategoryPtr", 0x130A2C0,
                               "48 89 5C 24 08 44 8B 89 84 00 00 00 45 33 C0 44 8B DA 45 8B D0 85 D2 75 04 33 C0", false};
// Sites inside GTA's Bink movie audio start (+0xE0208C) and others.
constexpr Item kSiteBinkCreate{
    "Bink movie sound start (frontend entity, CreateSound/InitStreamPlayer/PrepareAndPlay calls)", 0xE021E3,
    "4C 8D 4C 24 50 4C 8D 43 08 48 8D 0D ? ? ? ? 48 0F 44 D0 E8 ? ? ? ? 48 8B 5B 08 48 85 DB 74 39 "
    "44 8B 4E 14 44 8B 46 10 48 8B 16 48 8B CB E8 ? ? ? ? 45 33 C9 45 33 C0 33 D2 48 8B CB 89 6C 24 20 E8",
    true};
constexpr Item kSiteBinkBucket{"Bink movie sound start (allocation bucket byte)", 0xE020DC,
                               "8A 0D ? ? ? ? 88 8C 24 EA 00 00 00 48 8B 4B 10 48 85 C9", true};
constexpr Item kSiteBinkEnv{
    "Bink movie sound start (environment group path)", 0xE02100,
    "33 D2 E8 ? ? ? ? 48 8B F8 48 85 C0 0F 85 ? ? ? ? 48 8D 0D ? ? ? ? E8 ? ? ? ? 48 8B F8 48 85 C0 74 73 "
    "F3 0F 10 05 ? ? ? ? F3 0F 10 15 ? ? ? ? C7 44 24 30 E8 03 00 00 45 33 C9 33 D2 48 8B C8 "
    "F3 0F 11 44 24 28 C7 44 24 20 A0 0F 00 00 E8",
    true};
constexpr Item kSiteBinkTracker{"Bink movie sound start (entity tracker)", 0xE0219A,
                                "48 8B 4B 10 E8 ? ? ? ? 48 89 BC 24 B0 00 00 00 48 89 84 24 98 00 00 00", true};
constexpr Item kSiteCategory{"category manager site", 0x48F9F5, "48 8D 1D ? ? ? ? 48 8B CB BA EA 75 96 D5 E8", false};

constexpr Item kSiteMixerWait{
    "audio update: wait for the engine to leave the next settings slot", 0x1305E86,
    "80 3D ? ? ? ? 00 75 4A E8 ? ? ? ? 44 8B F0 E8 ? ? ? ? 8B 15 ? ? ? ? 41 03 D6 3B C2 73 19 B9 01 00 00 00 "
    "E8 ? ? ? ? 8B 3D ? ? ? ? E8 ? ? ? ? 3B F7 74 DA 0F B6 05 ? ? ? ? 3B F7 B9 01 00 00 00 0F 44 C1 88 05",
    false};

struct Image {
    std::uint8_t* base = nullptr;
    std::vector<CodeRange> code;
};

Image GameImage(void* base) {
    Image img;
    img.base = static_cast<std::uint8_t*>(base);
    const auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(img.base);
    const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS64*>(img.base + dos->e_lfanew);
    img.code = CodeRanges(IMAGE_FIRST_SECTION(nt), nt->FileHeader.NumberOfSections,
                          nt->OptionalHeader.SizeOfImage);
    return img;
}

struct Sig {
    std::vector<std::uint8_t> bytes;
    std::vector<bool> any;
};

Sig Parse(const char* text) {
    Sig s;
    for (const char* p = text; *p;) {
        while (*p == ' ') ++p;
        if (!*p) break;
        if (*p == '?') {
            s.bytes.push_back(0);
            s.any.push_back(true);
            while (*p == '?') ++p;
        } else {
            s.bytes.push_back(static_cast<std::uint8_t>(std::strtoul(p, nullptr, 16)));
            s.any.push_back(false);
            p += 2;
        }
    }
    return s;
}

bool MatchRaw(const std::uint8_t* at, const Sig& s) {
    for (std::size_t i = 0; i < s.bytes.size(); ++i)
        if (!s.any[i] && at[i] != s.bytes[i]) return false;
    return true;
}

// SEH-guarded (no C++ objects with destructors in this frame).
bool Match(const std::uint8_t* at, const Sig* s) {
    __try {
        return MatchRaw(at, *s);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

std::uint8_t* Locate(const Image& img, const Item& item, void (*log)(const char*)) {
    char line[320];
    const Sig sig = Parse(item.signature);
    std::uint8_t* const expected = img.base + item.rva;
    bool executable = false;
    for (const auto& r:img.code) executable |= item.rva >= r.begin && item.rva < r.end && sig.bytes.size() <= r.end-item.rva;
    if (executable && Match(expected, &sig)) {
        std::snprintf(line, sizeof(line), "gta audio: OK  %-60s +0x%07llx (%p)", item.name,
                      static_cast<unsigned long long>(item.rva), expected);
        log(line);
        return expected;
    }
    // Not at the expected place: accept only one match across all code ranges.
    std::uint8_t* found = nullptr;
    int hits = 0;
    for (const auto& r : img.code) {
      for (std::uint8_t* p = img.base+r.begin; p + sig.bytes.size() <= img.base+r.end && hits < 2; ++p) {
        if (Match(p, &sig)) {
            found = p;
            ++hits;
        }
      }
    }
    if (hits == 1) {
        std::snprintf(line, sizeof(line), "gta audio: MOVED %-58s expected +0x%07llx, unique match at +0x%07llx", item.name,
                      static_cast<unsigned long long>(item.rva), static_cast<unsigned long long>(found - img.base));
        log(line);
        return found;
    }
    std::snprintf(line, sizeof(line), "gta audio: %s %-55s signature %s at +0x%07llx (%d matches in executable sections)",
                  item.required ? "FAIL" : "skip", item.name, hits ? "ambiguous" : "not found",
                  static_cast<unsigned long long>(item.rva), hits);
    log(line);
    return nullptr;
}

template <class T>
T RipTarget(const std::uint8_t* dispAt) {
    std::int32_t d;
    std::memcpy(&d, dispAt, 4);
    return reinterpret_cast<T>(const_cast<std::uint8_t*>(dispAt + 4 + d));
}

std::uint8_t* CallTarget(const std::uint8_t* e8At) {
    if (*e8At != 0xE8 && *e8At != 0xE9) return nullptr;
    return RipTarget<std::uint8_t*>(e8At + 1);
}

} // namespace

bool Resolve(Audio& a, void (*log)(const char*)) {
    return ResolveImage(a, GetModuleHandleW(nullptr), log);
}

bool ResolveImage(Audio& a, void* image, void (*log)(const char*)) {
    char line[320];
    const Image img = GameImage(image);
    std::snprintf(line, sizeof(line), "gta audio: resolving GTA audio engine (image %p, %zu executable sections)", img.base,img.code.size());
    log(line);
    if (img.code.empty()) {
        log("gta audio: FAIL no executable sections");
        return false;
    }
    bool ok = true;
    auto need = [&](const Item& item) {
        std::uint8_t* p = Locate(img, item, log);
        if (!p && item.required) ok = false;
        return p;
    };
    std::uint8_t* ctor = need(kInitParamsCtor);
    std::uint8_t* create = need(kCreateSoundByName);
    std::uint8_t* createHash = need(kCreateSoundByHash);
    std::uint8_t* initStream = need(kInitStreamPlayer);
    std::uint8_t* play = need(kPrepareAndPlay);
    std::uint8_t* stop = need(kStopAndForget);
    std::uint8_t* envCreate = need(kEnvCreate);
    std::uint8_t* envInit = need(kEnvInit);
    std::uint8_t* envPos = need(kEnvSetPosition);
    std::uint8_t* envInterior = need(kEnvSetInterior);
    std::uint8_t* envLocation = need(kEnvSetLocation);
    std::uint8_t* envSettings = need(kEnvSetSettings);
    std::uint8_t* siteEnvReverb = need(kSiteEnvUpdateReverb);
    std::uint8_t* reqPos = need(kRequestedSetPosition);
    std::uint8_t* tracker = need(kEntityTracker);
    std::uint8_t* getCategory = need(kGetCategoryPtr);
    std::uint8_t* siteCreate = need(kSiteBinkCreate);
    std::uint8_t* siteBucket = need(kSiteBinkBucket);
    std::uint8_t* siteEnv = need(kSiteBinkEnv);
    std::uint8_t* siteTracker = need(kSiteBinkTracker);
    std::uint8_t* siteCategory = need(kSiteCategory);
    if (!ok) {
        log("gta audio: guard failed; GTA audio path disabled");
        return false;
    }

    // Cross-checks: the game's own Bink code calls exactly these functions.
    struct Check {
        const char* what;
        const std::uint8_t* got;
        const std::uint8_t* want;
    } checks[] = {
        {"Bink calls CreateSound(name)", CallTarget(siteCreate + 0x14), create},
        {"Bink calls InitStreamPlayer", CallTarget(siteCreate + 0x30), initStream},
        {"Bink calls PrepareAndPlay", CallTarget(siteCreate + 0x44), play},
        {"Bink calls naEnvironmentGroup::Create", CallTarget(siteEnv + 0x1A), envCreate},
        {"Bink calls naEnvironmentGroup::Init", CallTarget(siteEnv + 0x55), envInit},
        {"Bink calls the entity tracker getter", CallTarget(siteTracker + 0x04), tracker},
        {"CreateSound(name) tail-jumps to CreateSound(hash)", CallTarget(create + 0x3F), createHash},
        {"StopAndForget uses InitStreamPlayer's settings stride", RipTarget<std::uint8_t*>(stop + 0x1A),
         RipTarget<std::uint8_t*>(initStream + 0x15)},
        {"StopAndForget uses InitStreamPlayer's settings pool", RipTarget<std::uint8_t*>(stop + 0x21),
         RipTarget<std::uint8_t*>(initStream + 0x0E)},
    };
    for (const Check& c : checks) {
        const bool same = c.got && c.got == c.want;
        std::snprintf(line, sizeof(line), "gta audio: %s %s (+0x%llx)", same ? "OK  " : "FAIL", c.what,
                      static_cast<unsigned long long>(c.got ? c.got - img.base : 0));
        log(line);
        ok &= same;
    }
    // The Bink site really is the movie sound start: its sound name strings.
    // (lea rax, "BINK_MONO_SOUND"; lea rdx, "BINK_STEREO_SOUND" at site -0x0E.)
    const Sig leaPair = Parse("48 8D 05 ? ? ? ? 48 8D 15");
    const bool monoOk = Match(siteCreate - 0x0E, &leaPair) &&
                        std::memcmp(RipTarget<const char*>(siteCreate - 0x0E + 3), "BINK_MONO_SOUND", 16) == 0;
    std::snprintf(line, sizeof(line), "gta audio: %s Bink site names \"BINK_MONO_SOUND\"", monoOk ? "OK  " : "FAIL");
    log(line);
    ok &= monoOk;
    if (!ok) {
        log("gta audio: cross-check failed; GTA audio path disabled");
        return false;
    }

    a.initParamsCtor = reinterpret_cast<decltype(a.initParamsCtor)>(ctor);
    a.createSoundByName = reinterpret_cast<decltype(a.createSoundByName)>(create);
    a.createSoundByHash = reinterpret_cast<decltype(a.createSoundByHash)>(createHash);
    a.initStreamPlayer = reinterpret_cast<decltype(a.initStreamPlayer)>(initStream);
    a.prepareAndPlay = reinterpret_cast<decltype(a.prepareAndPlay)>(play);
    a.stopAndForget = reinterpret_cast<decltype(a.stopAndForget)>(stop);
    a.envCreate = reinterpret_cast<decltype(a.envCreate)>(envCreate);
    a.envInit = reinterpret_cast<decltype(a.envInit)>(envInit);
    a.envSetPosition = reinterpret_cast<decltype(a.envSetPosition)>(envPos);
    a.envSetInteriorFromEntity = reinterpret_cast<decltype(a.envSetInteriorFromEntity)>(envInterior);
    a.requestedSetPosition = reinterpret_cast<decltype(a.requestedSetPosition)>(reqPos);
    a.entityTracker = reinterpret_cast<decltype(a.entityTracker)>(tracker);
    a.frontendEntity = RipTarget<void*>(siteCreate + 0x0C);
    a.initParamsBucket = RipTarget<const std::uint8_t*>(siteBucket + 0x02);
    a.settingsPoolBase = RipTarget<const std::uintptr_t*>(initStream + 0x0E);
    a.settingsStride = RipTarget<const std::uint32_t*>(initStream + 0x15);
    a.settingsWriteIndex = RipTarget<const std::uint32_t*>(reqPos + 0x06);
    a.envArgScale = *RipTarget<const float*>(siteEnv + 0x2B);
    a.envArgDistance = *RipTarget<const float*>(siteEnv + 0x33);
    if (getCategory && siteCategory && CallTarget(siteCategory + 0x0F) == getCategory) {
        a.getCategoryPtr = reinterpret_cast<decltype(a.getCategoryPtr)>(getCategory);
        a.categoryManager = RipTarget<void*>(siteCategory + 0x03);
        a.categoriesOk = true;
    }
    // SetInteriorLocationFromEntity -> SetInteriorLocation -> SetInteriorSettings,
    // and the update reads the sends those write.
    a.envRoomLayoutOk = envLocation && envSettings && siteEnvReverb && CallTarget(envInterior + 0x22) == envLocation &&
                        CallTarget(envLocation + 0x77) == envSettings;
    std::snprintf(line, sizeof(line), "gta audio: %s environment group room settings layout (reset when back outside)",
                  a.envRoomLayoutOk ? "OK  " : "skip");
    log(line);
    const auto rva = [&](const void* p) {
        return static_cast<unsigned long long>(static_cast<const std::uint8_t*>(p) - img.base);
    };
    std::snprintf(line, sizeof(line),
                  "gta audio: globals: frontend entity +0x%llx (id %04x, vtable +0x%llx), params bucket +0x%llx = %u, "
                  "settings pool +0x%llx, stride +0x%llx = %u, write index +0x%llx = %u, category manager %s+0x%llx",
                  rva(a.frontendEntity), *reinterpret_cast<const std::uint16_t*>(static_cast<std::uint8_t*>(a.frontendEntity) + kEntityAudioId),
                  rva(*static_cast<void**>(a.frontendEntity)), rva(a.initParamsBucket), *a.initParamsBucket,
                  rva(a.settingsPoolBase), rva(a.settingsStride), *a.settingsStride, rva(a.settingsWriteIndex),
                  *a.settingsWriteIndex, a.categoriesOk ? "" : "(unavailable) ",
                  a.categoryManager ? rva(a.categoryManager) : 0ull);
    log(line);
    std::snprintf(line, sizeof(line), "gta audio: environment group Init args from the image: distance %.2f, scale %.2f",
                  a.envArgDistance, a.envArgScale);
    log(line);
    return true;
}

std::uint8_t* MixerWaitTimedOutFlag(void (*log)(const char*)) {
    const Image img = GameImage(GetModuleHandleW(nullptr));
    std::uint8_t* site = Locate(img, kSiteMixerWait, log);
    if (!site) return nullptr;
    // cmp byte [flag], 0 (7 bytes, imm8 after the displacement); movzx eax, [flag]; mov [flag], al.
    auto* const flag = RipTarget<std::uint8_t*>(site + 0x02) + 1;
    const bool same = flag == RipTarget<std::uint8_t*>(site + 0x3F) && flag == RipTarget<std::uint8_t*>(site + 0x4F);
    char line[160];
    std::snprintf(line, sizeof(line), "gta audio: %s mixer-wait timed-out flag +0x%llx", same ? "OK  " : "FAIL",
                  static_cast<unsigned long long>(flag - img.base));
    log(line);
    return same ? flag : nullptr;
}

void* RequestedSettings(const Audio& a, void* sound) {
    if (!sound || !a.settingsPoolBase || !a.settingsStride) return nullptr;
    const auto* s = static_cast<const std::uint8_t*>(sound);
    const std::uint32_t slot = s[0x80];
    if (slot == 0xFF) return nullptr;
    const std::uint32_t bucket = s[0x62];
    const std::uintptr_t pool = *a.settingsPoolBase;
    if (!pool) return nullptr;
    const std::uintptr_t block = *reinterpret_cast<const std::uintptr_t*>(pool + bucket * 0x34D0 + 0x34C0);
    if (!block) return nullptr;
    return reinterpret_cast<void*>(block + static_cast<std::uint32_t>(slot * *a.settingsStride));
}

} // namespace gtare

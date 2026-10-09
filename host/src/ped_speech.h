#pragma once
// GTA's own voice for a ped: pain grunts (PLAY_PAIN) and ambient speech
// contexts, so whichever character is skating (or is knocked over) sounds
// like themselves.
#include <algorithm>
#include <cstdlib>
#include <string>
#include <vector>
#include "natives.h"

namespace pedspeech {

// reason: GTA's eAudDamageReason (0 default: GTA picks the pain by
// rawDamage; with 0 damage it stays silent); < 0 off.
inline void Pain(Ped ped, int reason, float rawDamage) {
    if (reason >= 0) gta::Call<void>(gta::PLAY_PAIN, ped, reason, rawDamage, 0);
}

// A hit as GTA voices it: a fall-damage event (WEAPON_FALL) of `damage`
// health points, so GTA's own damage code picks and plays the pain (the bare
// PLAY_PAIN default stays silent without one). Health
// is put back at once (`keep`) or the hit never takes the ped below 101.
inline void Hurt(Ped ped, float damage, bool keep) {
    const int health = gta::Call<int>(gta::GET_ENTITY_HEALTH, ped);
    const int amount = std::min(static_cast<int>(damage), health - 101);
    if (amount < 1) return;
    gta::Call<void>(gta::APPLY_DAMAGE_TO_PED, ped, amount, 0, 0, static_cast<Hash>(0xCDC174B0) /*WEAPON_FALL*/);
    if (keep) gta::Call<void>(gta::SET_ENTITY_HEALTH, ped, health, 0, 0);
}

// Plays one context from a comma-separated list, picked at random among those
// the ped's voice has. Returns the context played ("" if the voice has none).
inline std::string Say(Ped ped, const std::string& list) {
    std::vector<std::string> have;
    std::size_t at = 0;
    while (at <= list.size()) {
        std::size_t end = list.find(',', at);
        if (end == std::string::npos) end = list.size();
        std::string c = list.substr(at, end - at);
        while (!c.empty() && c.front() == ' ') c.erase(c.begin());
        while (!c.empty() && c.back() == ' ') c.pop_back();
        if (!c.empty() && gta::Call<BOOL>(gta::DOES_CONTEXT_EXIST_FOR_THIS_PED, ped, c.c_str(), 0)) have.push_back(c);
        at = end + 1;
    }
    if (have.empty()) return {};
    const std::string& c = have[static_cast<std::size_t>(std::rand()) % have.size()];
    gta::Call<void>(gta::PLAY_PED_AMBIENT_SPEECH_NATIVE, ped, c.c_str(), "SPEECH_PARAMS_FORCE_SHOUTED_CRITICAL", 0);
    return c;
}

} // namespace pedspeech

#pragma once
// The board shop's parts (contributed by GTAxSkate): decks, griptapes, trucks
// and wheels built by tools/prepare-board-parts.py with the native board's
// seven-bone skeleton. The chosen ones are registered from loose files when
// first needed and posed like the native board, which they then replace.
#include <string>
#include <vector>
#include "skatev_runtime.h"

namespace boardparts {

// Call on the script thread. Reads <dataRoot>\private\board-parts.
bool Start(const std::wstring& dataRoot, void (*log)(const char*));

// The models to show (file stems without extension, e.g. skatev_d_default).
// Empty: the native board shows.
void SetLoadout(const std::vector<std::string>& models);
const std::vector<std::string>& Loadout();
bool Available(const std::string& model);

// True when every part of the loadout exists and was posed this frame; the
// caller then hides the native board. Not skating: removes the objects.
bool Tick(bool skating, const SvBoardPose& pose);

// The store's preview: `models` posed by `pose` (a rest pose from the
// runtime). Empty `models` removes it.
bool Preview(const std::vector<std::string>& models, const SvBoardPose& pose);

// The store's boards on show: board `index` (0..7) as `models` at rest in
// `pose`. ClearDisplay removes them all.
bool Display(int index, const std::vector<std::string>& models, const SvBoardPose& pose);
void ClearDisplay();

// The board in hand (GTA mode, the board picked in the weapon wheel) as
// `models` in `pose`; empty `models` removes it.
bool Hand(const std::vector<std::string>& models, const SvBoardPose& pose);

// A crew member's board (AI skater `slot`, 0..3) as `models` in `pose`;
// empty `models` removes it.
bool Crew(int slot, const std::vector<std::string>& models, const SvBoardPose& pose);

// Every part object shown (riding and preview): presentation only, never collision.
std::vector<int> Entities();

void Stop();

} // namespace boardparts

#pragma once
// The board shop (contributed by GTAxSkate): in Binco, the Discount Store and
// Suburban, G opens Skate 3's boards and parts (the runtime's board_shop.rs
// keeps the catalog and each character's garage). Buying takes the
// character's cash; the bought board is ridden at once (board_parts).
#include <string>
#include "skatev_runtime.h"

class RuntimeBridge;

namespace boardshop {

// After the runtime loaded: the catalog from `dataRoot`, garages in `gameDir`.
// `wheels`: called with the ridden wheels' hardness from their paint (0..1),
// or -1 for unpainted wheels, whenever the ridden board changes.
bool Start(RuntimeBridge& runtime, const std::string& dataRoot, const std::string& gameDir, void (*log)(const char*),
           void (*wheels)(float hardness));

// Once per script frame. `busy`: on the board or getting on or off (no store then).
void Tick(bool busy);

// A frame of skating (Skate's state, speed in m/s): the ridden board wears;
// a part worn through shows Skate 3's worn finish of it.
void Riding(const char* state, float speed);

// A crew member's board (AI skater `slot`, 0..3, in `pose`): a complete
// board from the shop's stock, the same for that slot each time, from the
// HD part models. False when it cannot show (the caller shows its own);
// ClearCrewBoard removes it.
bool CrewBoard(unsigned slot, const SvBoardPose& pose);
void ClearCrewBoard(unsigned slot);

// The store's menu is open (other menus and Skate's input stay out).
bool IsOpen();

} // namespace boardshop

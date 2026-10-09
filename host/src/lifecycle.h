#pragma once
// GTA side of the seamless GTA <-> Skate lifecycle (ledger P0-1): the host
// state machine's states, the controller board action and the GTA guards
// that keep it from fighting GTA's own use of the button.
#include <windows.h>
#include <types.h>
#include <cstdint>

namespace lifecycle {

// Host lifecycle. The menu's "Skate on / off" maps onto the same transitions.
//   Unprepared -> Preparing   player loaded: first track prepares the session
//   Preparing  -> Ready       runtime reports the session prepared
//   Ready      -> Waiting     take-out, but the area around the player is
//                             not in the working set yet (GTA keeps control)
//   Ready/Waiting -> Entering take-out (weapon wheel or menu) and the area is ready:
//                             ped held, runtime enters off the board
//   Entering   -> Skating     runtime ACTIVE (board held; player Y mounts)
//   Skating    -> Leaving     Y held (PutAwayHoldMs),
//                             or forced: menu / death / runtime error / network
//   Leaving    -> Ready       GTA ped restored where Skate's biped stands
//   Skating    -> Delegated   an action Skate has no movement for: the skater's
//                             chest goes under water (GTA swims) or X, X on
//                             foot at a barrier (GTA climbs over it)
//   Delegated  -> Entering    GTA reports the player back on foot: Skate
//                             resumes off the board, board in hand
enum class State { Unprepared, Preparing, Ready, Waiting, Entering, Skating, Leaving, Delegated };
const char* Name(State s);

// X pressed twice within `windowMs` (rising edges): the off-board climb request.
struct DoubleTap {
    DWORD windowMs = 450;
    bool wasDown = false;
    DWORD lastPress = 0;
    bool Update(bool down, DWORD now) {
        const bool rising = down && !wasDown;
        wasDown = down;
        if (!rising) return false;
        const bool second = lastPress != 0 && now - lastPress <= windowMs;
        lastPress = second ? 0 : now;
        return second;
    }
};

struct Settings {
    std::uint16_t menuButton = 0x0120;  // Back/View + LB opens the settings menu; 0 = keyboard '/' only
    bool inMissions = false;            // allow the board action while a story mission runs
    bool backgroundPrepare = true;      // build the Skate session at load (else at first entry)
};

// Why the board action must not start Skate right now (nullptr: allowed).
// `strict` adds the transient checks (movement/vehicle entry) used for the
// controller action; the menu uses only the hard checks.
const char* Blocked(Ped ped, bool strict, const Settings& settings);

// GTA may start its own vehicle entry with the same press (INPUT_ENTER is Y
// on foot). True while it is doing so.
bool EnteringVehicle(Ped ped);

// Rising edge of a button chord (the menu chord).
class BoardAction {
public:
    bool Pressed(std::uint16_t buttons, std::uint16_t mask) {
        const bool down = mask != 0 && (buttons & mask) == mask;
        const bool rising = down && !wasDown_;
        wasDown_ = down;
        return rising;
    }

private:
    bool wasDown_ = true; // a button held while loading is not a press
};

} // namespace lifecycle

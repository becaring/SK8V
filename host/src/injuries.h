#pragma once
// Injuries (contributed by GTAxSkate): after a hard bail the character limps
// with GTA's own injured walk (move_m@injured) once back on foot in GTA, the
// longer the harder the knock: 30 s from a 9 m/s bone knock, up to 2 min.

namespace injuries {

// A bone's knock (m/s) during a bail (the bail grunt's measure).
void Hit(float speed);

// The bail is over: a hard enough worst knock starts the limp.
void BailOver(void (*log)(const char*));

// Once per script frame: `ped` is the player, `walking` true when GTA has
// them on foot (not skating). `enabled`: the Injuries INI switch.
void Tick(int ped, bool walking, bool enabled, void (*log)(const char*));

} // namespace injuries

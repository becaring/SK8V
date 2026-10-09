#pragma once
// Player records over GTA: a call-out when a Hall
// of Meat bail or a banked line places in the top three (the book keeps ten),
// the record under the Hall of Meat score box during a bail, and a top-ten
// board from the settings menu (no zones). Drawn on the script thread with GTA's
// own text and shard (MIDSIZED_MESSAGE); the book and Skate 3's labels come
// from the runtime (rust/skatev-runtime/src/records.rs).
//
#include <string>

namespace recordsui {
using LogFn = void (*)(const char*);

// After the runtime loaded: looks up the optional record exports and opens
// the book at %LOCALAPPDATA%\SkateV\records.json (runtime nullptr: inert).
void Start(void* runtime, LogFn log);
// Opens or closes the top-ten board (menu item; '/' closes it while shown).
void NextPage();
bool Showing();
// Script thread, once per frame. `skating`: Skate is active; `homBail`: a
// Hall of Meat bail is on screen.
void Tick(bool skating, bool homBail);
}

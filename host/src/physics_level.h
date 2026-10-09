#pragma once
// Adapted for Legacy from Sol4ra's LS-Skate-LiveCollision (GTA V Enhanced),
// shared with SK8V by its author.
//
// GTA's physics level read from live memory: the table of every collision
// object the game has loaded (map bounds, props, vehicles, peds). Read-only.
// Layout and method: docs/PHYSICS-LEVEL.md
// (Enhanced) and evidence/2026-10-06/level-probe-legacy-scan.log (Legacy
// 3889: same 0x30 records, tagged phInst pointers, level index at +0x18).
// The table is found by scanning for the instances GTA's own natives name, so
// no game-build address is stored; a failed walk finds it again.
// Once a walk reads it cleanly the table's address goes to the runtime
// (`onTable`), which reads the static map from it instead of the offline cache
// (rust/skatev-runtime/src/live.rs); a lost table withdraws it (address 0).
#include <cstdint>
namespace physlevel {
// Script thread, once at startup. `onTable` may be called from the reader's
// own thread.
void Start(void (*log)(const char*), void (*onTable)(std::uintptr_t table, std::uintptr_t imageLo, std::uintptr_t imageHi));
// Script thread, every frame: the instance list the locator needs.
void Tick();
}

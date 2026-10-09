#pragma once

// Calls into one GTA audEntity's virtual functions (gta_audio.cpp: the
// frontend audio entity Skate's stream sounds are created on). The entity's
// vtable pointer is pointed at a copy whose slots [first, last] run
// `onCall(self, slot)` and then the original function with the caller's
// registers untouched (rcx, rdx, r8, r9, xmm0-3 preserved; stack arguments are
// never moved). Only that one object is affected.
//
// Why: Rockstar Editor playback runs no script frames, so Skate's audio needs
// a call on GTA's main thread that runs in the editor too; GTA updates its
// audio entities there every frame.
namespace audiohook {

bool Install(void* entity, int first, int last, void (*onCall)(void* self, int slot), void (*log)(const char*));
// Puts the original vtable back (thunks stay allocated: a call may be in flight).
void Uninstall();

} // namespace audiohook

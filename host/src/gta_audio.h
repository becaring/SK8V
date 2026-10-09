#pragma once
#include <windows.h>
#include <string>

// Skate's dry gameplay audio played INSIDE GTA's audio engine (ledger P0-3b).
//
// The runtime (ABI 8 audio block, skatev_runtime.h) renders Skate 3's voices
// dry, one mono 48 kHz stream per emitter (board, body, sense-of-speed bed).
// Each stream becomes a GTA external stream sound, exactly the way GTA's own
// Bink movie audio does it (image +0xE0208C): "BINK_MONO_SOUND" created on the
// frontend audio entity with audSoundInitParams, fed through an
// audReferencedRingBuffer (InitStreamPlayer, PrepareAndPlay). Positional
// emitters get their own naEnvironmentGroup (occlusion, reverb, interior from
// the player ped) and follow the emitter position through the sound's
// requested settings every frame; the speed bed plays non-positional (Pan 0,
// as Bink does for screen movies). GTA then owns 3D position, occlusion,
// reverb/interiors, volume categories and pause.
//
// Threading: every GTA audio call is made from the ScriptHookV script fiber,
// which runs on GTA's main (game) thread, the same context GTA's own script
// commands create sounds from. A feeder thread of ours pulls the runtime's
// PCM (sv_audio_pull is single-consumer) and writes the rings under the
// ring's own CRITICAL_SECTION, as GTA's Bink decoder does.
//
// Failure: when any guard fails, or GTA sound creation keeps failing, the
// bridge logs once and stays silent; it never crashes the game.
//
// SkateVLegacy.ini ([SkateV]):
//   AudioOutput=Gta|Off           (default Gta; legacy Audio=0 means Off)
//   AudioCache=<dir>              prepared Skate sample cache (sv_audio_configure)
//   AudioMasterGain=1.0           folded into every emitter's gain by the runtime
//   AudioGtaPlacement=Position    Position (emitter positions) | Tracker (player ped)
//   AudioGtaCategory=<name>       GTA audio category, by name or 0x hash (default
//                                 0xD4AE89CA, chain -2 dB); SOUND = the sound's own
//   AudioGtaBufferMs=50           queued audio kept in each GTA ring
//   AudioEditorTone=1             440 Hz beeps through GTA in the Rockstar Editor
namespace gtaaudio {

// Script thread, once, after the runtime is loaded (`runtime` may be null).
// Never fails hard.
void Start(const std::wstring& iniPath, HMODULE runtimeModule, void* runtime, void (*log)(const char*));
// Script thread, every frame in every mode. `skating`: skate mode active.
void Tick(bool skating);
// DLL detach (not process exit): stops the feeder thread. GTA sounds are left
// to GTA (their rings stay allocated and play silence).
void Stop();
// Script thread: applies AudioGtaGainDb now (the menu saves it to the INI).
void SetGainDb(float db);

} // namespace gtaaudio

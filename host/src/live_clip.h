#pragma once
#include <cstdint>

// Owned live clip (LiveClip=1): the skater plays skatev_live/skatev_body, a
// two-frame raw-float clip built by rust/ped-export live_clip and shipped as
// the mods dlc pack dlcpacks:/skatev/ (tools/install-live-clip.py). The pose
// hook writes each bone's local pose into both frames, so GTA's own anim
// output (what the Rockstar Editor and every GTA system see) is Skate's pose
// instead of the idle GTA was playing under our overwrite. Port of
// SkateGTA-B4's owned clip (DECISIONS 2026-10-05); format:
// docs/YCD-OWNED-CLIP.md. The clip's data is found in memory by its marker.
namespace liveclip {

// Script thread, every skating tick: loads the dictionary, keeps the clip
// playing and pinned, scans for its data, logs status. `skeleton`: the ped's
// crSkeleton (bone tags are mapped to clip tracks on change).
void Tick(int ped, std::uintptr_t skeleton, void (*log)(const char*));
// Script thread: leaving skate mode; blends the clip out.
void Stop(int ped);
// Script thread: the gun is out (true) or away. While out the clip is stopped
// and Tick leaves the ped's task slot to GTA, so GTA's own aim, weapon and
// wheel tasks run and animate the upper body (gun_pose.h).
void Suspend(int ped, bool on);
// Pose hook, once per write: false when the clip is not located (or its
// marker is gone; a rescan is then requested).
bool BeginWrite();
// Pose hook: bone `index`'s local translation and rotation (x, y, z, w).
void WriteBone(int index, const float t[3], const float q[4]);
// Pose hook, first write only (Comparing): GTA's own local for bone `index`,
// evaluated from the clip's rest values, against those values: confirms the
// rotation convention in game (logged by Tick).
bool Comparing();
void Compare(int index, const float t[3], const float q[4]);
void EndCompare();

} // namespace liveclip

#pragma once
#include <cstdint>
#include <string>
namespace pedcollision {
// Script-thread only. Shapes are authored GTA bounds, carried by native
// kinematic objects. The player's existing collision remains suppressed.
bool Start(const std::wstring& ini, const std::wstring& dataRoot, void (*log)(const char*));
// ignoreVehicle: a vehicle (and its riders) the limb colliders pass through this frame, 0 for none.
void Tick(int ped, const float* boneWorld, std::uint32_t count, int ignoreVehicle = 0);
void Stop();
}

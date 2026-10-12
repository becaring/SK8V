#pragma once
// The map while skating (contributed by GTAxSkate): GTA's GPS route to the
// waypoint stays on in Skate mode, as it does in a vehicle, and GTA's skate
// spots (from its own map data) are blips on the map.

namespace skatemap {

// Once per script frame. `skating`: Skate mode is on. `gps`/`spots`: the
// INI switches (GpsWhileSkating, SkateSpots).
void Tick(bool skating, bool gps, bool spots);

// GPS destinations for the M menu (GTAxSkate's interaction page): the
// nearest clothing store (GTA's own store blips; the board shop), the nearest
// skate spot, or no waypoint. Each returns what was set, for a notice.
const char* RouteToClothingStore();
const char* RouteToSkateSpot();
const char* ClearRoute();

// Removes what this module put on the map (script shutdown).
void Clear();

} // namespace skatemap

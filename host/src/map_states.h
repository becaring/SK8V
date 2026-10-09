#pragma once
// Script-toggled map states (IPLs: alternate interiors, mission states). The
// world cache leaves their collision out of the static bake and writes it
// per state beside the cache (`CACHE.map-states.txt`); the host asks GTA which
// nearby states are active and the runtime adds only those.
#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace mapstates {

// Reads `<worldCache stem>.map-states.txt`; returns the state count (0 when
// the cache has none).
std::size_t Load(const std::string& worldCache);

// The states GTA has active within reach of GTA point (x, y), as name
// hashes, and their names for the log. True when the set differs from the
// last one returned, or with `force` (before an activation). Script thread
// only (calls IS_IPL_ACTIVE).
bool Refresh(float x, float y, bool force, std::vector<std::uint32_t>& active, std::string& names);

}  // namespace mapstates

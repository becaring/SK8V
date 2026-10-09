#pragma once
#include "skatev_runtime.h"
#include <istream>
#include <unordered_set>

// Which collision a GTA entity gets: its exact local template (the cache's
// `.prop-models`), a bounded box, or none. Props are never part of the live
// static map (OBJECT type), so every one in the object pool routes here.
class PropCollisionIndex {
public:
    enum class Route { None, Exact, Box };
    struct ParseStats { std::size_t accepted = 0, rejected = 0; };
    static bool DoorMayOpen(int state) { return state == 0 || state == 3 || state == 5; }
    static bool AllowsBox(Route route) { return route == Route::Box; }
    ParseStats ReadModels(std::istream& input);
    bool HasTemplate(std::uint32_t model) const;
    Route Select(const SvDynamicBody& body, bool object, bool door, bool unlocked) const;
    void RemoveModel(std::uint32_t model) { models_.erase(model); }
    const std::unordered_set<std::uint32_t>& Models() const { return models_; }
private:
    std::unordered_set<std::uint32_t> models_;
};

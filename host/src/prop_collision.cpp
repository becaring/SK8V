#include "prop_collision.h"
#include <algorithm>
#include <charconv>
#include <sstream>
#include <string>

namespace {
bool Hash(const std::string& text, std::uint32_t& value) {
    if (text.empty() || text.size() > 8) return false;
    const auto r = std::from_chars(text.data(), text.data() + text.size(), value, 16);
    return r.ec == std::errc{} && r.ptr == text.data() + text.size();
}
bool NextLine(std::istream& input, std::string& line) {
    while (std::getline(input, line)) {
        const auto comment = line.find('#');
        if (comment != std::string::npos) line.resize(comment);
        if (line.find_first_not_of(" \t\r") != std::string::npos) return true;
    }
    return false;
}
}

PropCollisionIndex::ParseStats PropCollisionIndex::ReadModels(std::istream& input) {
    ParseStats result; std::string line;
    while (NextLine(input, line)) {
        std::istringstream in(line); std::string token, extra; std::uint32_t hash;
        if (!(in >> token) || !Hash(token, hash) || (in >> extra)) { ++result.rejected; continue; }
        models_.insert(hash); ++result.accepted;
    }
    return result;
}
bool PropCollisionIndex::HasTemplate(std::uint32_t model) const { return models_.contains(model); }
PropCollisionIndex::Route PropCollisionIndex::Select(const SvDynamicBody& b, bool object, bool door, bool unlocked) const {
    if (HasTemplate(b.model_hash)) return Route::Exact;
    if (!object) return Route::Box; // Existing ped/vehicle bounds and reactions.
    if (door) return unlocked ? Route::None : Route::Box;
    const float largest = (std::max)({b.fallback.half_extents.x, b.fallback.half_extents.y, b.fallback.half_extents.z});
    return largest <= 2.5f ? Route::Box : Route::None;
}

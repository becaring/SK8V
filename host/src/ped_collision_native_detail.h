#pragma once
#include <cstdint>
#include <istream>
#include <set>
#include <sstream>
#include <string>
#include <vector>

namespace pedcollision::detail {
struct Entry { std::string model; std::uint16_t boneTag; };
inline bool Parse(std::istream& in, std::vector<Entry>& result) {
    result.clear();
    std::string line;
    if (!std::getline(in, line)) return false;
    if (!line.empty() && line.back() == '\r') line.pop_back();
    if (line != "SKATEV_PED_COLLIDERS\t1") return false;
    std::set<std::string> names;
    std::set<unsigned> tags;
    while (std::getline(in, line)) {
        if (!line.empty() && line.back() == '\r') line.pop_back();
        const auto tab = line.find('\t');
        if (tab == std::string::npos) return false;
        Entry e{line.substr(0, tab), 0};
        if (e.model.size() > 48 || !e.model.starts_with("skatev_body_") ||
            e.model.find_first_not_of("abcdefghijklmnopqrstuvwxyz0123456789_") != std::string::npos) return false;
        std::istringstream number(line.substr(tab + 1));
        unsigned tag = 0;
        char extra;
        if (!(number >> tag) || tag > 65535 || (number >> extra) ||
            !names.insert(e.model).second || !tags.insert(tag).second) return false;
        e.boneTag = static_cast<std::uint16_t>(tag);
        result.push_back(e);
        if (result.size() > 64) return false;
    }
    return !result.empty();
}
}

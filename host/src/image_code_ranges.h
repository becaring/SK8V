#pragma once
#include <windows.h>
#include <cstddef>
#include <vector>

namespace gtare {
struct CodeRange { std::size_t begin, end; };
// Packed Legacy images have two sections named .text. Retain every executable
// section, rather than letting the later section overwrite the original code.
inline std::vector<CodeRange> CodeRanges(const IMAGE_SECTION_HEADER* sections,
                                       std::size_t count, std::size_t imageSize) {
    std::vector<CodeRange> result;
    for (std::size_t i=0; i<count; ++i) {
        const auto& s=sections[i];
        const std::size_t begin=s.VirtualAddress, length=s.Misc.VirtualSize;
        if ((s.Characteristics & IMAGE_SCN_MEM_EXECUTE) && length &&
            begin < imageSize && length <= imageSize-begin) result.push_back({begin,begin+length});
    }
    return result;
}
}

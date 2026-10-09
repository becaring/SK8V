#pragma once
// Stall sampler: a watchdog thread that notices when the game's script frames
// stop (main thread blocked, as in the 150..300 ms hitches whose game-thread
// CPU is ~0) and samples the main and render threads' instruction pointers and
// stack return addresses while the stall lasts. The log names which module and
// offset each thread was in, so a hitch can be attributed to the code it waits
// in. While a thread is suspended the watchdog only reads its context and
// copies its stack (no locks, no allocation); all decoding happens after resume.
#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace stall {

enum Role { Main, Render, RoleCount };

// Log sink (the host log); set before the first Beat.
void SetLog(void (*log)(const char*));
// Call from the script loop once per frame, right after WAIT(0) returns.
// The first call registers the calling thread as the main thread and starts
// the watchdog.
void Beat();
// Call from the Present hook; registers the render thread once.
void RegisterRender();
// Stops the watchdog (DLL detach).
void Stop();

// ---- pure helpers (unit tested) -------------------------------------------

struct Module {
    std::string name;
    std::uintptr_t base = 0;
    std::vector<std::pair<std::uintptr_t, std::uintptr_t>> code; // absolute [begin, end)
};

// Index of the module whose executable sections contain `address`, or -1.
int FindCode(const std::vector<Module>& modules, std::uintptr_t address);

// True when the bytes just before return address `ret` (preceding[0..n) holds
// the n bytes ending at ret) encode a call instruction that returns to ret:
// call rel32, call [rip+disp32], call reg, call [reg(+disp)] with or without
// a REX prefix.
bool IsCallSite(const std::uint8_t* preceding, std::size_t n);

// "name+0xrva" for an address inside a module, else "0x<address>".
std::string Symbolize(const std::vector<Module>& modules, std::uintptr_t address);

} // namespace stall

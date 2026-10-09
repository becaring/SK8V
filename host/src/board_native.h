#pragma once
#include <string>
#include "skatev_runtime.h"
namespace boardnative {
// Call on the ScriptHookV script thread, after the Legacy Story Mode guard.
bool Start(const std::wstring& iniPath,const std::wstring& dataRoot,void (*log)(const char*));
// True only when the native custom object exists and all seven bones were posed.
bool Tick(bool skating,const SvBoardPose& pose);
// Put away only: delete presentation on the script thread, retain the
// registered resources and loaded archetype/model for the next invocation.
void Stop();
int Entity(); // Exclude this presentation-only entity from ExternalQueries.
}

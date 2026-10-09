cmake_minimum_required(VERSION 3.24)
project(SkateVLegacy LANGUAGES CXX)

if(NOT WIN32 OR NOT CMAKE_SIZEOF_VOID_P EQUAL 8)
  message(FATAL_ERROR "SkateV Legacy host is x64 Windows-only")
endif()

set(SHVDIR "${CMAKE_CURRENT_LIST_DIR}/../third_party/scripthookv" CACHE PATH "Fetched ScriptHookV SDK root")
if(NOT EXISTS "${SHVDIR}/inc/main.h" OR NOT EXISTS "${SHVDIR}/lib/ScriptHookV.lib")
  message(FATAL_ERROR "ScriptHookV SDK not found. Run tools/fetch-scripthookv.ps1 first.")
endif()

add_library(SkateVLegacy SHARED
  src/main.cpp
  src/lifecycle.cpp
  src/runtime_bridge.cpp
  src/dynamic_world.cpp
  src/map_states.cpp
  src/ped_bodies.cpp
  src/prop_collision.cpp
  src/game_probe.cpp
  src/ped_collision_native.cpp
  src/ped_collider_builder.cpp
  src/pose_hook.cpp
  src/gta_audio.cpp
  src/audio_entity_hook.cpp
  src/gta_audio_re.cpp
  src/gta_audio_dsp.cpp
  src/live_clip.cpp
  src/hud_overlay.cpp
  src/hud_renderer.cpp
  src/xray_renderer.cpp
  src/records_ui.cpp
  src/board_native.cpp
  src/stall_sampler.cpp
  src/physics_level.cpp
  src/menu.cpp
)
target_compile_features(SkateVLegacy PRIVATE cxx_std_20)
target_compile_definitions(SkateVLegacy PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX)
target_include_directories(SkateVLegacy PRIVATE include "${SHVDIR}/inc")
target_link_libraries(SkateVLegacy PRIVATE "${SHVDIR}/lib/ScriptHookV.lib" version xinput shell32)
# Original Skate 3 HUD overlay (hud_overlay.cpp): D3D11 + runtime shader compile.
target_link_libraries(SkateVLegacy PRIVATE d3d11 dxgi d3dcompiler)
set_target_properties(SkateVLegacy PROPERTIES PREFIX "" SUFFIX ".asi")

if(MSVC)
  target_compile_options(SkateVLegacy PRIVATE /W4 /permissive- /EHsc)
  target_compile_options(SkateVLegacy PRIVATE $<$<CONFIG:Release>:/O2>)
endif()

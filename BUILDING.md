# Building SK8V

Windows x64 only. The result is the same release folder and zip the published
SK8V download contains.

## Requirements

- Git
- Rust via rustup (the pinned 1.98.1 toolchain installs itself from `rust-toolchain.toml`)
- Visual Studio 2022 with "Desktop development with C++" (MSVC)
- CMake 3.24 or newer
- Python 3.11 or newer

## Steps

1. Clone this repository.
2. Run `BOOTSTRAP.bat`. It:
   - fetches the pinned upstream sources into `upstream/` (`upstreams.lock.json`): the Skate 3 Rust engine, the RAGE format and archive crates, and rage-cli;
   - fetches the ScriptHookV SDK into `third_party/` (`external.lock.json`);
   - builds the Rust runtime and tools, the ASI and `rage.exe` (later rebuilds: `BUILD.bat`).
3. Copy the `world` folder from a published SK8V release zip into `world/`.
   These are the baked collision sidecars (`los-santos.svsd`, `crackmaps.svgj`),
   too large for git.
4. Run `python tools\build-release.py --version <version>`.
   The release is written to `build\release\SK8V-<version>\` and its `.zip`.

The release build writes `.cargo\config.toml`, a local, git-ignored file. It
maps your checkout and user folder out of the paths that rustc writes into
binaries. After the first release build, run `BUILD.bat` again: the release
build refuses binaries that still contain those paths.

## Layout

- `host/` is the ScriptHookV ASI (C++, `SkateVLegacy.asi`). It hooks the GTA
  side: ped pose, audio, collision, menu and HUD.
- `rust/` holds the runtime (`SkateVRuntime.dll`), which runs Skate's
  simulation, along with its crates and the setup tools:
  - `skate-xma` decodes audio;
  - `skatev-ped-export` covers ped and clip work;
  - `skatev-world-cache` builds the world templates.
- `patches/skate/` holds the changes applied to the pinned Skate engine
  (`tools/prepare-skate-overlay.ps1` builds the overlay under `build/overlay/`).
- `tools/` holds the build scripts, the player setup (`prepare.py`,
  `setup_engine.py`, `setup-wizard.ps1`) and the preparation steps it runs.

No GTA or Skate data is in this repository. Setup builds everything from the
player's own copies.

"""Writes THIRD-PARTY-NOTICES.txt for a release: the project-level credits below,
then every crate linked into the shipped Rust binaries with its own license
files, taken from the resolved dependency graph (cargo metadata).

    python tools/third-party-notices.py [--out build/package/THIRD-PARTY-NOTICES.txt]

Fails when a shipped crate has neither license files nor a license field.
"""
import argparse
import json
import subprocess
from pathlib import Path

from _common import REPO

# Workspace packages whose binaries ship (SkateVRuntime.dll, skate-xma.exe, the
# preparation tools). Their own code is SK8V's or credited below.
SHIPPED = ['skatev-runtime', 'skate-xma', 'skatev-ped-export', 'world-cache', 'svwc']
OURS = {'skatev-runtime', 'skate-xma', 'skatev-ped-export', 'world-cache', 'svwc', 'skate-aems', 'skate-hud'}
# The Skate engine crates compiled from the mashup overlay (build/overlay/skate): credited below.
MASHUP = {'skate-core', 'skate-data', 'skate-host', 'skate-net'}
TARGET = 'x86_64-pc-windows-msvc'
LICENSE_FILES = ('LICENSE', 'LICENCE', 'COPYING', 'NOTICE', 'UNLICENSE')

HEADER = """SK8V third-party notices
========================

SK8V is licensed under the Apache License, Version 2.0 (LICENSE). It contains or
is built from the third-party work below. SK8V ships no GTA V or Skate 3 code or
data: players convert their own copies. ScriptHookV is not included; SK8V's ASI
links against its SDK import library and needs the player's own ScriptHookV.

Real-time collision detection: credit to Sol4ra. The live GTA collision reader
is adapted for Legacy from Sol4ra's LS-Skate-LiveCollision (GTA V Enhanced),
shared with SK8V by its author.

Skate engine and HUD (Apache-2.0)
---------------------------------
2010 Rust Rewrite Mashup, by chasmlol, a fork of IW4L by vladtrc.
  https://github.com/chasmlol/2010-rust-rewrite-mashup  (Apache-2.0)
  Its Skate 3 engine crates (skate-core, skate-data, skate-host, skate-net) are
  compiled into SkateVRuntime.dll. Copyright 2026 vladtrc; Skate engine by chasmlol.
skate-3-rust-engine HUD modules (apt_vm, apt_display, apt_movie, apt_text,
  apt_scene, hud_runtime) and asset-pipeline exports, by chasmlol, used under
  chasmlol's Apache-2.0 grant of 2026-10-07 (commit cb796893).
  https://github.com/SK8-ENGINE/skate-3-rust-engine
  The HUD modules are compiled into SkateVRuntime.dll; the exports run during setup.

EA-XMA audio decoder (LGPL-2.1-or-later)
----------------------------------------
skate-xma.exe is a port of FFmpeg 4.4's xmaframes / WMA Pro decoder
(libavcodec), Copyright the FFmpeg developers, https://ffmpeg.org. It is a
separate program licensed under the GNU Lesser General Public License 2.1 or
later (COPYING.LGPL); its source is rust/skate-xma in the SK8V repository.

RAGE formats (Unlicense)
------------------------
rage-formats, rpf-archive-rs, rage-cli, by VIRUXE, released into the public domain.
  https://github.com/VIRUXE

Rust crates
-----------
Each crate below is linked into a shipped binary and is distributed under its
own license, reproduced from the crate's published sources.
"""


def graph():
    meta = json.loads(subprocess.run(
        ['cargo', 'metadata', '--format-version', '1', '--filter-platform', TARGET],
        cwd=REPO / 'rust', check=True, capture_output=True, text=True).stdout)
    packages = {p['id']: p for p in meta['packages']}
    nodes = {n['id']: n for n in meta['resolve']['nodes']}
    roots = [p['id'] for p in meta['packages'] if p['name'] in SHIPPED and p['id'] in meta['workspace_members']]
    seen, todo = set(), list(roots)
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]['deps']:
            if any(k['kind'] is None for k in dep['dep_kinds']):  # normal deps only: what gets linked
                todo.append(dep['pkg'])
    return [packages[i] for i in seen]


def section(p, printed):
    """`printed`: license text -> the crate that first reproduced it (identical texts appear once)."""
    name = f"{p['name']} {p['version']}"
    lines = [f"{name}  ({p.get('license') or 'see files'})"]
    if p.get('repository'):
        lines.append(f"  {p['repository']}")
    root = Path(p['manifest_path']).parent
    files = sorted(f for f in root.iterdir() if f.is_file() and f.name.upper().startswith(LICENSE_FILES))
    if not files and not p.get('license'):
        raise SystemExit(f"{name}: no license files and no license field")
    for f in files:
        text = f.read_text(encoding='utf-8', errors='replace').strip()
        if text in printed:
            lines.append(f'--- {f.name}: same text as {printed[text]} ---')
        else:
            printed[text] = name
            lines += ['', f'--- {f.name} ---', text]
    return '\n'.join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', type=Path, default=REPO / 'build/package/THIRD-PARTY-NOTICES.txt')
    args = ap.parse_args()
    printed = {}
    crates = sorted((p for p in graph() if p['name'] not in OURS | MASHUP), key=lambda p: (p['name'], p['version']))
    text = HEADER + '\n' + ('\n\n' + '=' * 78 + '\n\n').join(section(p, printed) for p in crates) + '\n'
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(text, encoding='utf-8', newline='\n')
    print(f'{args.out}: {len(crates)} crates')


if __name__ == '__main__':
    main()

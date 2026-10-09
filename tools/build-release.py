"""Builds the SK8V release folder and zip from an allowlist: the built binaries,
the preparation tools, the pinned donor tools snapshot, the shipped world
sidecars, licenses and notices. Nothing retail: every file is listed here, and
a final scan refuses Skate or GTA data formats.

    python tools/build-release.py --version 1.0 [--update]

--update also writes SK8V-<version>-update.zip for players who already ran
setup: the ASI and the runtime, to copy over the installed ones, with the
license files. Only for a release whose setup output is unchanged (no stage
script, shipped sidecar or data format changed); otherwise players rerun setup.

Build first (build-rust.ps1, build-host.ps1, build-rage-cli.ps1; the ped
export and world cache tools with cargo). rustc keeps source paths in
binaries (panic locations): this writes a machine-local .cargo/config.toml
(gitignored) that maps this checkout and the user folder to neutral names, and
refuses binaries that still hold either (rebuild them after the first run). No Python
ships: the setup window uses the player's Python 3.11+ with numpy and Pillow,
or offers to download a pinned one (tools/get-python.ps1). Output: build/release/SK8V-<version>/ and
its .zip, with SHA256SUMS.txt.
"""
import argparse
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

from _common import DONOR, DONOR_PIN, REPO, donor_commit, donor_head, sha256_file

TOOLS = ['prepare.py', 'gta_extract.py', 'xiso.py', 'setup_engine.py', '_common.py', 'convert-skate-data.py',
         'prepare-skate-audio.py', 'prepare-audio-tuning.py', 'prepare-hom-hud.py', 'prepare-hom-xray.py',
         'prepare-board.py', 'prepare-board-native.py', 'prepare-ped-colliders.py', 'ped_collider_templates.py',
         'build-vehicle-masses.py', 'build-board-weapon.py', 'build-board-icon.py', 'check-clip-layout.py',
         'SkateVLegacy.ini.template', 'board-wheel-icon.bin', 'setup-wizard.ps1',
         'get-python.ps1']
BINARIES = ['skate-xma.exe', 'xex_image.exe', 'skatev-ped-export.exe', 'live_clip.exe', 'skatev-world-cache.exe']
FILES = {'LICENSE': 'LICENSE', 'NOTICE': 'NOTICE', 'README.md': 'README.md', 'upstreams.lock.json': 'upstreams.lock.json',
         'host/src/live_clip_layout.h': 'host/src/live_clip_layout.h',  # prepare checks the clip against it
         'rust/skate-xma/COPYING.LGPL': 'rust/skate-xma/COPYING.LGPL',
         'build/package/SkateVLegacy.asi': 'build/package/SkateVLegacy.asi',
         'build/package/SkateVRuntime.dll': 'build/package/SkateVRuntime.dll',
         'build/tools/rage.exe': 'build/tools/rage.exe',
         # Baked sidecars keyed to GTA 1.0.3889.0 collision; too large for git: copy the world folder of a
         # published SK8V release into world/ (BUILDING.md).
         'world/los-santos.svsd': 'world/los-santos.svsd', 'world/crackmaps.svgj': 'world/crackmaps.svgj'}
# Skate and GTA data formats: none may be in a release.
RETAIL = {'.big', '.xex', '.rpf', '.ydr', '.ydd', '.yft', '.ytd', '.ytyp', '.ymap', '.ybn', '.ycd', '.gfx', '.abin',
          '.glb', '.mxb', '.svwc', '.svpc', '.meta'}
# The donor code setup runs (traced 2026-10-08 with an open() audit hook over convert, hom-hud, chyron,
# hom-xray and board): whole packages it touched plus the top-level files it opened.
DONOR_PATHS = ['tools/asset_pipeline', 'tools/owned_game', 'tools/vendor/skate3_anim', 'tools/vendor/skate3_ui',
               'tools/vendor/utt', 'tools/default_skater_retail_manifest.json', 'tools/extract_default_skater.py',
               'tools/extract_session_marker.py', 'tools/install_prepared_hud.py', 'tools/prepare_hud.py',
               'tools/prepare_runtime_huds.py']

def copy(src, dst):
    if not src.is_file():
        raise SystemExit(f'missing {src} (build it first; world/ comes from a published release, BUILDING.md)')
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)


def remap_config():
    """The repo's .cargo/config.toml (gitignored, this machine's paths): every
    cargo run inside the checkout gets the remaps, so builds never flip
    between flagged and unflagged."""
    pairs = [(Path.home(), '~'), (REPO, 'SK8V')]  # the last match wins: the repo after home
    flags = [f'--remap-path-prefix={str(p).replace(chr(92), sep)}={to}' for p, to in pairs for sep in (chr(92), '/')]
    text = ('# Written by tools/build-release.py for this machine: hide local paths in built binaries.\n'
            '[build]\nrustflags = [' + ', '.join(f"'{f}'" for f in flags) + ']\n')
    config = REPO / '.cargo/config.toml'
    if not config.exists() or config.read_text(encoding='utf-8') != text:
        config.parent.mkdir(exist_ok=True)
        config.write_text(text, encoding='utf-8')


def local_paths(shipped):
    """Shipped binaries that still name this checkout or the user folder."""
    needles = [s.lower().encode() for p in (REPO, Path.home()) for s in (str(p), str(p).replace(chr(92), '/'))]
    return [p for p in shipped if p.suffix.lower() in ('.exe', '.dll', '.asi')
            and any(n in p.read_bytes().lower() for n in needles)]


def donor_tools(out):
    """The donor's tracked tools/ tree at the pinned commit, and the pin file
    convert-skate-data.py checks instead of git."""
    if donor_head() != donor_commit():
        raise SystemExit(f'donor checkout is not at the pinned {donor_commit()}; run tools/fetch-upstreams.ps1')
    files = subprocess.run(['git', '-c', 'safe.directory=*', '-C', str(DONOR), 'ls-files', *DONOR_PATHS],
                           capture_output=True, text=True, check=True).stdout.split()
    root = out / DONOR.relative_to(REPO)
    for f in files:
        copy(DONOR / f, root / f)
    for f in ('LICENSE', 'LICENSE.md', 'README.md'):
        if (DONOR / f).exists():
            copy(DONOR / f, root / f)
    (root / DONOR_PIN.name).write_text(donor_commit() + '\n', encoding='ascii')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--version', required=True)
    ap.add_argument('--update', action='store_true', help='also the ASI-and-runtime update zip')
    a = ap.parse_args()
    remap_config()
    out = REPO / f'build/release/SK8V-{a.version}'
    shutil.rmtree(out, ignore_errors=True)
    for dst, src in FILES.items():
        copy(REPO / src, out / dst)
    for f in TOOLS:
        copy(REPO / 'tools' / f, out / 'tools' / f)
    sys.path.insert(0, str(REPO / 'tools'))
    from prepare import binary
    for f in BINARIES:
        copy(binary(f), out / 'tools/bin' / f)
    donor_tools(out)
    subprocess.run([sys.executable, str(REPO / 'tools/third-party-notices.py'), '--out', str(out / 'THIRD-PARTY-NOTICES.txt')],
                   check=True)
    (out / 'SK8V Setup.cmd').write_text(
        '@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\\setup-wizard.ps1"\r\n', encoding='ascii')

    shipped = [p for p in out.rglob('*') if p.is_file()]
    bad = [p for p in shipped if p.suffix.lower() in RETAIL]
    if bad:
        raise SystemExit('retail data format in the release: ' + ', '.join(str(p.relative_to(out)) for p in bad[:10]))
    leaky = local_paths(shipped)
    if leaky:
        raise SystemExit('local paths in ' + ', '.join(str(p.relative_to(out)) for p in leaky) +
                         ': rebuild with the remaps in .cargo/config.toml (build-rust.ps1, build-rage-cli.ps1, '
                         'cargo build --release -p skatev-ped-export -p skatev-world-cache)')
    sums = ''.join(f'{sha256_file(p)}  {p.relative_to(out).as_posix()}\n' for p in sorted(shipped))
    (out / 'SHA256SUMS.txt').write_text(sums, encoding='ascii')
    archive = out.parent / (out.name + '.zip')  # not with_suffix: the version has dots
    with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as z:
        for p in sorted(out.rglob('*')):
            if p.is_file():
                z.write(p, Path(out.name) / p.relative_to(out))
    print(f'{len(shipped) + 1} files -> {archive} ({archive.stat().st_size / 2**20:.0f} MB)')
    if a.update:
        update(out, a.version)


def update(out, version):
    """The update zip, laid out as the GTA folder (setup_engine.install puts both binaries at its root)."""
    files = {'SkateVLegacy.asi': 'build/package/SkateVLegacy.asi', 'SkateVRuntime.dll': 'build/package/SkateVRuntime.dll',
             'LICENSE.txt': 'LICENSE', 'NOTICE.txt': 'NOTICE', 'THIRD-PARTY-NOTICES.txt': 'THIRD-PARTY-NOTICES.txt'}
    readme = '\r\n'.join([
        f'SK8V {version} update', '',
        'For a GTA V folder where SK8V setup already finished. Close GTA V, then copy SkateVLegacy.asi and',
        'SkateVRuntime.dll into the GTA V folder (beside GTA5.exe), replacing the old ones. Your settings and',
        'prepared data stay as they are.', '',
        'Setup never finished? Use the full SK8V zip instead: its setup picks up where the last one stopped.', ''])
    archive = out.parent / f'SK8V-{version}-update.zip'
    with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as z:
        for name, src in files.items():
            z.write(out / src, name)
        z.writestr('README-UPDATE.txt', readme)
    print(f'update: {len(files) + 1} files -> {archive} ({archive.stat().st_size / 2**20:.1f} MB)')


if __name__ == '__main__':
    main()

"""Extracts the few files preparation reads from the player's own GTA V
install with sk8v-rpf (rust/gta-archives; rage only for the keys). The map
placements, vehicle fragments and ped skeletons are read in place by
skatev-world-cache --templates-gta / --vehicle-bounds-gta and
skatev-ped-export --skeletons-gta instead.

    keys(rage, gta, out)                 GTA's keys, from GTA5.exe
    vehicles(...)                        handling/vehicles.meta per archive (local/gta-vehdata layout)
    weapon_meta(...)                     update.rpf weapons/weaponanimations.meta (local/gta-weapon-meta)
    ped_models(...)                      the three players' archives

    python tools/gta_extract.py <GTA folder> <out dir> [--rage rage.exe] [--rpf sk8v-rpf.exe]   # into <out>/<part>
"""
import argparse
import os
import re
import shutil
import subprocess
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

from _common import REPO

PLAYERS = ('player_zero', 'player_one', 'player_two')
JOBS = min(12, os.cpu_count() or 4)  # extractions at once (throttle lowers it on a hard disk)


class ExtractError(Exception):
    pass


def run(rage, *args, keys=None, partial_ok=False):
    """rage's output. A pack's unrelated zero-size entries make rage exit 1
    after extracting everything else, so with partial_ok only a run that
    extracted nothing fails."""
    cmd = [str(rage), '--no-update-check'] + (['--keys', str(keys)] if keys else []) + [str(a) for a in args]
    p = subprocess.run(cmd, capture_output=True, text=True, errors='replace')
    out = p.stdout + p.stderr
    if p.returncode and not (partial_ok and re.search(r'Extracted: [1-9]', out)):
        raise ExtractError(f'{" ".join(cmd[2:])} failed ({p.returncode}): {out.strip()[-400:]}')
    return out


def extract(rpf, keys, archive, patterns, dest, partial_ok=False):
    """rpf: sk8v-rpf (rust/gta-archives), which reads only the entries the patterns name;
    rage's extract decompresses every entry first (the whole game per step on a hard disk)."""
    Path(dest).mkdir(parents=True, exist_ok=True)
    return run(rpf, 'extract', archive, *patterns, '-o', dest, '--recursive', keys=keys, partial_ok=partial_ok)


def keys(rage, gta, out):
    out = Path(out)
    if not (out / 'gtav_aes_key.dat').exists():
        out.mkdir(parents=True, exist_ok=True)
        run(rage, '--exe', Path(gta) / 'GTA5.exe', 'extract-keys', '-o', out)
    return out


def seek_penalty(path):
    """True if path's drive is a spinning disk (Windows' seek-penalty query), False if it is not,
    None if Windows cannot say (a network share; some USB, RAID or Storage Spaces drives)."""
    import ctypes as C
    import ctypes.wintypes as W
    drive = os.path.splitdrive(os.path.abspath(path))[0]
    if len(drive) != 2:  # a UNC path: no volume to ask
        return None
    k32 = C.WinDLL('kernel32')
    k32.CreateFileW.restype = W.HANDLE
    h = k32.CreateFileW('\\\\.\\' + drive, 0, 3, None, 3, 0, None)  # no access needed for the query
    if h in (None, W.HANDLE(-1).value):
        return None
    try:
        query = (C.c_uint32 * 3)(7, 0, 0)  # StorageDeviceSeekPenaltyProperty, PropertyStandardQuery
        out, n = (C.c_uint32 * 3)(), W.DWORD()
        ok = k32.DeviceIoControl(W.HANDLE(h), 0x2D1400, query, C.sizeof(query), out, C.sizeof(out), C.byref(n), None)
        if not ok or n.value < 9:
            return None
        return bool(out[2] & 0xFF)  # DEVICE_SEEK_PENALTY_DESCRIPTOR.IncursSeekPenalty
    finally:
        k32.CloseHandle(W.HANDLE(h))


def throttle(*paths):
    """One rage process when the GTA install or the output is on a hard disk, or on a drive Windows
    cannot classify: several at once make the heads seek between archives and run many times slower
    than one after another (a 1.0 setup with twelve ran for hours, 2026-10-09). Returns those paths."""
    global JOBS
    slow = [str(p) for p in paths if seek_penalty(p) is not False]
    if slow:
        JOBS = 1
    return slow


def parallel(fn, items, progress=None):
    """fn over items, several rage processes at once (rage itself is single-threaded);
    progress(fraction done) after each one."""
    with ThreadPoolExecutor(JOBS) as pool:
        futures = [pool.submit(fn, i) for i in items]
        for n, f in enumerate(as_completed(futures), 1):
            f.result()
            if progress:
                progress(n / len(futures))
        return [f.result() for f in futures]


def warm(*roots):
    """Reads every file under roots once, in parallel; call it right before a
    tool that reads them one at a time. Just after a large extraction, each
    read stalls (the templates read took 64 s instead of 21 s); a parallel pass
    just before costs about 10 s and hides it. Done earlier, it does not help.
    Skipped on a hard disk (one reader): there it is a whole extra read pass."""
    if JOBS == 1:
        return
    files = [p for root in roots for p in Path(root).rglob('*') if p.is_file()]
    with ThreadPoolExecutor(16 if JOBS > 2 else JOBS) as pool:
        list(pool.map(lambda p: len(p.read_bytes()), files))


def vehicle_archives(gta):
    """[(layer, archive)] after the base archives (the caller's 00_common):
    every pack's dlc*.rpf, then update.rpf and update2.rpf (later wins)."""
    gta = Path(gta)
    out = []
    for pack in sorted(p for p in (gta / 'update/x64/dlcpacks').iterdir() if p.is_dir()):
        out += [(f'50_{pack.name}_{rpf.stem}', rpf) for rpf in sorted(pack.glob('dlc*.rpf'))]
    out += [(f'99_{u}', gta / f'update/{u}.rpf') for u in ('update', 'update2') if (gta / f'update/{u}.rpf').exists()]
    return out


META = ['*handling.meta', '*vehicles.meta']


def vehicles(rpf, keys, gta, meta, log=print, progress=None):
    """handling.meta and vehicles.meta per archive under <meta>/<layer>/
    (build-vehicle-masses.py). The fragments are read in place
    (skatev-world-cache --vehicle-bounds-gta)."""
    gta, meta = Path(gta), Path(meta)
    shutil.rmtree(meta, ignore_errors=True)
    jobs = [('00_common', gta / 'common.rpf')] + vehicle_archives(gta)

    def one(job):
        layer, archive = job
        try:
            extract(rpf, keys, archive, META, meta / layer, partial_ok=True)
        except ExtractError as e:  # a damaged or truncated pack costs only its own vehicles, as in the game
            if layer == '00_common':
                raise
            log(f'vehicles: skipped {archive}: {e}')
    parallel(one, jobs, progress)
    log(f'vehicles: {sum(1 for _ in meta.rglob("*.meta"))} meta files from {len(jobs)} archives')


def weapon_meta(rpf, keys, gta, out):
    """The weapons.meta / weaponanimations.meta build-board-weapon.py reads (<out>/common/data/ai/)."""
    extract(rpf, keys, Path(gta) / 'update/update.rpf', ['*common/data/ai/weapons.meta', '*weaponanimations.meta'], out)
    ai = Path(out) / 'common/data/ai'
    if not (ai / 'weapons.meta').exists() or not (ai / 'weaponanimations.meta').exists():
        raise ExtractError(f'weapons.meta / weaponanimations.meta not found in update.rpf (looked in {ai})')
    return ai


def ped_models(rpf, keys, gta, out):
    """<out>/models/cdimages/streamedpeds_players.rpf: the three players' archives. Every other
    ped's skeleton is read in place (skatev-ped-export --skeletons-gta)."""
    gta, out = Path(gta), Path(out)
    extract(rpf, keys, gta / 'x64v.rpf', [f'*streamedpeds_players.rpf/{p}*' for p in PLAYERS], out)
    warm(out)
    return out / 'models/cdimages/streamedpeds_players.rpf'


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('gta', type=Path)
    ap.add_argument('out', type=Path)
    ap.add_argument('--rage', type=Path, default=REPO / 'build/tools/rage.exe', help='for the keys')
    ap.add_argument('--rpf', type=Path, default=REPO / 'rust/target/release/sk8v-rpf.exe')
    ap.add_argument('--only', choices=['vehicles', 'weapon-meta', 'peds'])
    a = ap.parse_args()
    throttle(a.gta, a.out)
    k = keys(a.rage, a.gta, a.out / 'gta-keys')
    parts = {'vehicles': lambda: vehicles(a.rpf, k, a.gta, a.out / 'gta-vehdata'),
             'weapon-meta': lambda: weapon_meta(a.rpf, k, a.gta, a.out / 'gta-weapon-meta'),
             'peds': lambda: ped_models(a.rpf, k, a.gta, a.out / 'gta-peds')}
    for name, fn in parts.items():
        if a.only in (None, name):
            fn()


if __name__ == '__main__':
    main()

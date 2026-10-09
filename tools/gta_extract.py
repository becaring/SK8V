"""Extracts what preparation reads from the player's own GTA V install, with
the rage CLI. Replaces the extract-gta-*.ps1 / export-peds.ps1 passes for
setup; the output layouts are the same, so the build tools read either.

    keys(rage, gta, out)                 GTA's keys, from GTA5.exe
    placements(rage, keys, gta, out)     ymap/ytyp/props per layer (local/gta-meta layout)
    vehicles(...)                        vehicle .yft and handling/vehicles.meta per archive
                                         (local/gta-vehyft and local/gta-vehdata layouts)
    weapon_meta(...)                     update.rpf weapons/weaponanimations.meta (local/gta-weapon-meta)
    ped_models(...)                      every ped .yft (skeletons) and the three players' archives

    python tools/gta_extract.py <GTA folder> <out dir> [--rage rage.exe]   # everything, into <out>/<part>
"""
import argparse
import os
import re
import shutil
import subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from _common import REPO

PLAYERS = ('player_zero', 'player_one', 'player_two')
JOBS = min(12, os.cpu_count() or 4)  # rage processes at once


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


def extract(rage, keys, archive, patterns, dest, partial_ok=False):
    Path(dest).mkdir(parents=True, exist_ok=True)
    return run(rage, 'extract', archive, *patterns, '-o', dest, '--recursive', keys=keys, partial_ok=partial_ok)


def keys(rage, gta, out):
    out = Path(out)
    if not (out / 'gtav_aes_key.dat').exists():
        out.mkdir(parents=True, exist_ok=True)
        run(rage, '--exe', Path(gta) / 'GTA5.exe', 'extract-keys', '-o', out)
    return out


def layers(rage, keys, gta, scratch):
    """[(name, archive)]: update.rpf, then the Story Mode patch packs in
    dlclist.xml order (lowest priority first). MP packs (their map changes load
    only in Online) and *g9ec* packs are skipped."""
    gta, scratch = Path(gta), Path(scratch)
    update = gta / 'update/update.rpf'
    extract(rage, keys, update, ['*dlclist.xml'], scratch)
    found = next(scratch.rglob('dlclist.xml'), None)
    if not found:
        raise ExtractError('dlclist.xml not found in update.rpf')
    packs = [p for p in re.findall(r'dlcpacks:/([^/<]+)', found.read_text(encoding='utf-8', errors='replace'))
             if p.startswith('patch') and 'g9ec' not in p]
    out = [('update', update)]
    out += [(p, gta / f'update/x64/dlcpacks/{p}/dlc.rpf') for p in packs
            if (gta / f'update/x64/dlcpacks/{p}/dlc.rpf').exists()]
    return out


PLACEMENT_PATTERNS = ['*.ymap', '*.ytyp', '*/props/*.ydr', '*/props/*.ydd', '*/props/*.yft', '*/interiors/*.ydr',
                      '*/interiors/*.ydd', '*trailer*.yft', '*_manifest.ymf']


def parallel(fn, items):
    """fn over items, several rage processes at once (rage itself is single-threaded)."""
    with ThreadPoolExecutor(JOBS) as pool:
        return list(pool.map(fn, items))


def warm(*roots):
    """Reads every file under roots once, in parallel; call it right before a
    tool that reads them one at a time. Just after a large extraction, each
    read stalls (the templates read took 64 s instead of 21 s); a parallel pass
    just before costs about 10 s and hides it. Done earlier, it does not help."""
    files = [p for root in roots for p in Path(root).rglob('*') if p.is_file()]
    with ThreadPoolExecutor(16) as pool:
        list(pool.map(lambda p: len(p.read_bytes()), files))


def placements(rage, keys, gta, out, log=print):
    """base/<rpf>/... for every top-level archive, layers/<NN>_<layer>/... with
    each patch pack's content.xml and setup2.xml, and layers.txt."""
    gta, out = Path(gta), Path(out)
    jobs = [(a, out / 'base' / a.stem, False) for a in sorted(gta.glob('*.rpf'))]
    names = []
    for i, (name, archive) in enumerate(layers(rage, keys, gta, out / '_meta')):
        jobs.append((archive, out / 'layers' / f'{i:02d}_{name}', i > 0))
        names.append(jobs[-1][1].name)

    def one(job):
        archive, dest, rules = job
        extract(rage, keys, archive, PLACEMENT_PATTERNS, dest)
        if rules:  # world-cache applies only archives an active change set enables
            run(rage, 'extract', archive, 'content.xml', 'setup2.xml', '-o', dest, keys=keys)
    parallel(one, sorted(jobs, key=lambda j: -j[0].stat().st_size))  # the biggest archive is the floor: start it first
    (out / 'layers.txt').write_text('\n'.join(names) + '\n', encoding='ascii')
    log(f'placements from {len(jobs)} archives')


def vehicle_archives(gta):
    """[(layer, archive)] after the base archives (the caller's 00_common):
    every pack's dlc*.rpf, then update.rpf and update2.rpf (later wins)."""
    gta = Path(gta)
    out = []
    for pack in sorted(p for p in (gta / 'update/x64/dlcpacks').iterdir() if p.is_dir()):
        out += [(f'50_{pack.name}_{rpf.stem}', rpf) for rpf in sorted(pack.glob('dlc*.rpf'))]
    out += [(f'99_{u}', gta / f'update/{u}.rpf') for u in ('update', 'update2') if (gta / f'update/{u}.rpf').exists()]
    return out


FRAGMENTS, META = ['*vehicles*.yft'], ['*handling.meta', '*vehicles.meta']


def vehicles(rage, keys, gta, fragments, meta, log=print):
    """One pass per archive: vehicle .yft (not the render-only _hi variants)
    under <fragments>/<layer>/, handling.meta and vehicles.meta under
    <meta>/<layer>/ (build-vehicle-masses.py)."""
    gta, fragments, meta = Path(gta), Path(fragments), Path(meta)
    jobs = [('00_common', gta / 'x64e.rpf', FRAGMENTS), ('00_common', gta / 'common.rpf', META)]
    jobs += [(layer, archive, FRAGMENTS + META) for layer, archive in vehicle_archives(gta)]
    for d in (fragments, meta):
        shutil.rmtree(d, ignore_errors=True)

    def one(job):
        layer, archive, patterns = job
        tmp = fragments.parent / f'.tmp-{layer}-{archive.stem}'
        shutil.rmtree(tmp, ignore_errors=True)
        extract(rage, keys, archive, patterns, tmp, partial_ok=True)
        kept = 0
        for f in list(tmp.rglob('*')):
            rel = f.relative_to(tmp)
            name = f.name.lower()
            if name.endswith('.meta'):
                dest = meta / layer / rel
            elif name.endswith('.yft') and 'vehicles' in str(rel).lower() and not f.stem.lower().endswith('_hi'):
                dest, kept = fragments / layer / rel, kept + 1
            else:
                continue
            dest.parent.mkdir(parents=True, exist_ok=True)
            f.replace(dest)
        shutil.rmtree(tmp, ignore_errors=True)
        return kept
    kept = sum(parallel(one, jobs))
    log(f'vehicles: {kept} fragments, {sum(1 for _ in meta.rglob("*.meta"))} meta files from {len(jobs)} archives')


def weapon_meta(rage, keys, gta, out):
    """The weapons.meta / weaponanimations.meta build-board-weapon.py reads (<out>/common/data/ai/)."""
    extract(rage, keys, Path(gta) / 'update/update.rpf', ['*common/data/ai/weapons.meta', '*weaponanimations.meta'], out)
    ai = Path(out) / 'common/data/ai'
    if not (ai / 'weapons.meta').exists() or not (ai / 'weaponanimations.meta').exists():
        raise ExtractError(f'weapons.meta / weaponanimations.meta not found in update.rpf (looked in {ai})')
    return ai


def ped_models(rage, keys, gta, out, log=print):
    """<out>/models/cdimages/streamedpeds_players.rpf (the three players) and
    <out>/skeletons (every ped .yft in the install, for skeleton-only entries)."""
    gta, out = Path(gta), Path(out)
    hits = run(rage, 'search', gta, '*peds*.rpf/*.yft', keys=keys)
    archives = sorted({m.group(1) for m in re.finditer(r'^(.+?\.rpf):', hits, re.M)})
    tmp = out / '.tmp'
    shutil.rmtree(tmp, ignore_errors=True)
    jobs = [(gta / 'x64v.rpf', [f'*streamedpeds_players.rpf/{p}*' for p in PLAYERS], out)]
    jobs += [(a, ['*peds*.rpf/*.yft'], tmp / f'{i:03d}') for i, a in enumerate(archives)]
    parallel(lambda j: extract(rage, keys, *j), jobs)
    # merge in archive order, so a name in two archives resolves as a sequential extraction would (later wins)
    skeletons = out / 'skeletons'
    for part in sorted(tmp.iterdir()) if tmp.exists() else []:
        for f in part.rglob('*.yft'):
            dest = skeletons / f.relative_to(part)
            dest.parent.mkdir(parents=True, exist_ok=True)
            f.replace(dest)
    shutil.rmtree(tmp, ignore_errors=True)
    warm(out)
    log(f'ped models from {len(archives)} archives')
    return out / 'models/cdimages/streamedpeds_players.rpf', skeletons


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('gta', type=Path)
    ap.add_argument('out', type=Path)
    ap.add_argument('--rage', type=Path, default=REPO / 'build/tools/rage.exe')
    ap.add_argument('--only', choices=['placements', 'vehicles', 'weapon-meta', 'peds'])
    a = ap.parse_args()
    k = keys(a.rage, a.gta, a.out / 'gta-keys')
    parts = {'placements': lambda: placements(a.rage, k, a.gta, a.out / 'gta-meta'),
             'vehicles': lambda: vehicles(a.rage, k, a.gta, a.out / 'gta-vehyft', a.out / 'gta-vehdata'),
             'weapon-meta': lambda: weapon_meta(a.rage, k, a.gta, a.out / 'gta-weapon-meta'),
             'peds': lambda: ped_models(a.rage, k, a.gta, a.out / 'gta-peds')}
    for name, fn in parts.items():
        if a.only in (None, name):
            fn()


if __name__ == '__main__':
    main()

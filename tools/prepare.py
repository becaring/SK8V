"""Prepares everything SK8V reads from the player's own Skate 3 disc and GTA V
install, then (with --install) installs into the GTA folder. One command for
setup; each stage leaves a receipt, so a rerun resumes after the last stage
that finished (--redo STAGE reruns one and everything after it).

    python tools/prepare.py --skate <Skate 3 ISO or extracted folder> --gta <GTA V folder> [--install] [--clean]

The data lives in <GTA folder>/SK8V (--out for another place): the runtime
data (skate-data/assets, audio, peds, world, live-clip) and the intermediates under gta/ and disc/, which --clean deletes
after a successful run. Skate stages run first: a wrong disc fails in seconds,
not after the GTA extraction.
"""
import argparse
import hashlib
import importlib.util
import json
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from _common import REPO, sha256_file
import gta_extract
import setup_engine
import xiso

TOOLS = REPO / 'tools'
RAGE = REPO / 'build/tools/rage.exe'
# Per stage: rough seconds on a fast PC (the setup window's progress bar) and what the window shows.
STEPS = {'disc': (20, 'Reading the Skate 3 disc'), 'convert': (17, 'Converting Skate 3 data'),
         'audio': (17, 'Converting Skate 3 audio'), 'audio-tuning': (1, 'Reading audio tuning'),
         'hom-hud': (5, 'Preparing the Hall of Meat HUD'), 'hom-xray': (1, 'Preparing the x-ray view'),
         'board': (1, 'Preparing the board'), 'gta-placements': (40, 'Reading GTA V map placements'),
         'gta-vehicles': (40, 'Reading GTA V vehicles'), 'world': (30, 'Building world collision'),
         'peds': (30, 'Reading GTA V characters'), 'live-clip': (4, 'Building the board animation pack'),
         'wheel-icon': (2, 'Adding the weapon wheel icon'), 'board-native': (3, 'Building the GTA board model'),
         'ped-colliders': (2, 'Building character collision'), 'verify': (1, 'Checking the prepared data'),
         'install': (5, 'Installing into the GTA folder')}
PACKAGE = REPO / 'build/package'
# Dev-baked material and ground-joint sidecars shipped with the release (DECISIONS 2026-10-06).
SHIPPED_WORLD = REPO / 'world'
SHADER_REFERENCE = 'base/x64w/dlcpacks/mppilot/dlc.rpf/x64/levels/gta5/props/mppilot_props.rpf/pil_p_para_bag_pilot_s.ydr'
PED_COLLIDER_SOURCE = '88b90e3104464c947439c70764df617f9356ef4f048a3501368c4ab8a494f1b6'  # x64a.rpf z_z_fred.yft
PACK = r'update\x64\dlcpacks\skatev'


def binary(name):
    for d in (REPO / 'tools/bin', PACKAGE, REPO / 'rust/target/release'):
        if (d / name).is_file():
            return d / name
    raise FileNotFoundError(f'{name} not found in tools/bin, build/package or rust/target/release')


def py(script, *args):
    subprocess.run([sys.executable, '-B', str(TOOLS / script), *map(str, args)], check=True)


def exe(name, *args):
    subprocess.run([str(binary(name)), *map(str, args)], check=True)


class Prepare:
    def __init__(self, a):
        self.a, self.out = a, a.out.resolve()
        self.gta = a.gta.resolve()
        self.disc = a.skate.resolve() if a.skate.is_dir() else self.out / 'disc'
        self.assets = self.out / 'skate-data/assets'
        self.work = self.out / 'gta'
        self.audio = self.out / 'audio'
        self.peds = self.out / 'peds'
        self.world = self.out / 'world/sk8v.svwc'
        self.clip = self.out / 'live-clip'
        self.masses = self.work / 'vehicle-masses.txt'
        self.receipts = self.out / '.stages'
        self.clip_ok = None

    @property
    def keys(self):
        return gta_extract.keys(RAGE, self.gta, self.work / 'gta-keys')

    # Stages, in order: (name, chain, scripts whose change invalidates it, function). The skate and gta
    # chains are independent and run side by side; join needs both (and writes into skate-data only after
    # convert has renamed it into place).
    def stages(self):
        return [
            ('disc', 'skate', ['xiso.py'], self.stage_disc),
            ('convert', 'skate', ['convert-skate-data.py'], lambda: py(
                'convert-skate-data.py', '--xex', self.disc / 'default.xex', '--out', self.out / 'skate-data')),
            ('audio', 'skate', ['prepare-skate-audio.py'], lambda: py(
                'prepare-skate-audio.py', '--skate-data', self.disc, '--out', self.audio, '--decoder', binary('skate-xma.exe'))),
            ('audio-tuning', 'skate', ['prepare-audio-tuning.py'], lambda: py(
                'prepare-audio-tuning.py', '--collections', self.assets / 'private/stock/skater-collections.json',
                '--out', self.audio / 'grain-tuning.bin', '--component-out', self.audio / 'component-tuning.bin',
                '--xex', self.disc / 'default.xex', '--xex-tool', binary('xex_image.exe'))),
            ('hom-hud', 'skate', ['prepare-hom-hud.py'], self.stage_hom_hud),
            ('hom-xray', 'skate', ['prepare-hom-xray.py'], lambda: py('prepare-hom-xray.py', '--game', self.disc, '--assets', self.assets)),
            ('board', 'skate', ['prepare-board.py'], lambda: py('prepare-board.py', '--assets', self.assets)),
            ('gta-placements', 'gta', ['gta_extract.py'], lambda: gta_extract.placements(RAGE, self.keys, self.gta, self.work / 'gta-meta')),
            ('board-native', 'join', ['prepare-board-native.py'], lambda: py(
                'prepare-board-native.py', '--board', self.assets / 'private/board/board-materials.json',
                '--lighting', self.assets / 'private/character-lighting.json',
                '--shader-reference', self.work / 'gta-meta' / SHADER_REFERENCE, '--rage', RAGE,
                '--out', self.assets / 'private/board-native')),
            ('ped-colliders', 'join', ['prepare-ped-colliders.py', 'ped_collider_templates.py'], self.stage_ped_colliders),
            ('gta-vehicles', 'gta', ['gta_extract.py', 'build-vehicle-masses.py'], self.stage_vehicles),
            ('world', 'gta', [], self.stage_world),
            ('peds', 'gta', ['gta_extract.py'], self.stage_peds),
            ('live-clip', 'gta', ['gta_extract.py', 'build-board-weapon.py', 'check-clip-layout.py'], self.stage_clip),
            ('wheel-icon', 'gta', ['build-board-icon.py'], self.stage_icon),
            ('verify', 'join', [], self.verify),
        ]

    def stage_disc(self):
        if self.a.skate.is_dir():
            missing = [p for p in xiso.NEEDED if not (self.disc / p).is_file()]
            if missing:
                raise SystemExit(f'{self.disc} is not an extracted Skate 3 disc: missing {", ".join(missing)}')
            return
        xiso.extract_needed(self.a.skate, self.disc)

    def stage_hom_hud(self):
        py('prepare-hom-hud.py', '--game', self.disc, '--assets', self.assets)
        py('prepare-hom-hud.py', '--game', self.disc, '--assets', self.assets, '--movie', 'chyron', '--gta', self.gta)

    def stage_ped_colliders(self):
        src = self.work / 'ped-colliders'
        gta_extract.extract(RAGE, self.keys, self.gta / 'x64a.rpf', ['*models/z_z_fred.yft'], src)
        yft = next(src.rglob('z_z_fred.yft'))
        policy = src / 'binding-policy.json'  # the runtime checks the live ped against the model (prepare-ped-colliders.py)
        policy.write_text(json.dumps({'source_sha256': PED_COLLIDER_SOURCE, 'binding_policy': 'require_live_asset_match',
                                      'expected_frag_name': 'pack:/z_z_fred', 'binding_verified': False}), encoding='utf-8')
        out = self.assets / 'private/ped-colliders'
        py('prepare-ped-colliders.py', '--source-yft', yft, '--binding-receipt', policy, '--out', out, '--rage', RAGE)
        py('ped_collider_templates.py', out, out / 'runtime', RAGE)

    def stage_vehicles(self):
        gta_extract.vehicles(RAGE, self.keys, self.gta, self.work / 'gta-vehyft', self.work / 'gta-vehdata')
        py('build-vehicle-masses.py', self.work / 'gta-vehdata', self.masses)

    def stage_world(self):
        self.world.parent.mkdir(parents=True, exist_ok=True)
        gta_extract.warm(self.work / 'gta-meta', self.work / 'gta-vehyft')
        cache = binary('skatev-world-cache.exe')
        subprocess.run([str(cache), '--templates', self.world, self.work / 'gta-meta', self.masses], check=True)
        # after --templates: it rewrites the template index the vehicles are merged into
        subprocess.run([str(cache), '--vehicle-bounds', self.world, self.work / 'gta-vehyft', self.masses], check=True)
        svsd = sorted(SHIPPED_WORLD.glob('*.svsd'))
        if svsd:
            shutil.copyfile(svsd[0], self.world.with_suffix('.svsd'))
        if (SHIPPED_WORLD / 'crackmaps.svgj').exists():
            shutil.copyfile(SHIPPED_WORLD / 'crackmaps.svgj', self.world.parent / 'crackmaps.svgj')

    def stage_peds(self):
        players, skeletons = gta_extract.ped_models(RAGE, self.keys, self.gta, self.work / 'gta-peds')
        for ped in gta_extract.PLAYERS:
            exe('skatev-ped-export.exe', players, ped, self.peds)
        exe('skatev-ped-export.exe', '--skeletons', skeletons, self.peds)

    def stage_clip(self):
        ai = gta_extract.weapon_meta(RAGE, self.keys, self.gta, self.work / 'gta-weapon-meta')
        py('build-board-weapon.py', ai / 'weapons.meta', ai / 'weaponanimations.meta', self.clip / 'weapon')
        exe('live_clip.exe', self.peds, self.clip, self.clip / 'live_clip_layout.h')

    def stage_icon(self):
        hud = self.work / 'hud'
        gta_extract.extract(RAGE, self.keys, self.gta / 'update/update.rpf', ['*scaleform_generic.rpf/hud.gfx'], hud)
        py('build-board-icon.py', '--source', next(hud.rglob('hud.gfx')), '--out', self.clip / 'hud.gfx')

    def clip_matches(self):
        """The ASI drives the clip with the layout it was compiled with; another ped
        set (other DLC) changes it. The pack (board weapon, icon) still loads."""
        spec = importlib.util.spec_from_file_location('check_clip_layout', TOOLS / 'check-clip-layout.py')
        check = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(check)
        diff = check.difference((self.clip / 'live_clip_layout.h').read_text(encoding='utf-8'),
                                (REPO / 'host/src/live_clip_layout.h').read_text(encoding='utf-8'))
        if diff:
            print(f'live clip layout differs from the ASI\'s ({diff}): installing with LiveClip=0')
        return diff is None

    def verify(self):
        """What the installed runtime needs, checked before anything is installed."""
        need = [self.assets / 'private/hud/runtime/trickdisplay.json', self.assets / 'private/skater.glb',
                self.audio / 'grain-tuning.bin', self.audio / 'component-tuning.bin',
                self.world.with_suffix('.prop-models.txt'), self.clip / 'dlc.rpf', self.clip / 'hud.gfx',
                self.assets / 'private/ped-colliders/runtime/templates.txt']
        missing = [str(p) for p in need if not p.exists()]
        board = self.assets / 'private/board-native'
        receipt = json.loads((board / 'receipt.json').read_text(encoding='utf-8'))
        for kind in ('ydr', 'ytyp'):
            if sha256_file(board / f'skatev_board.{kind}') != receipt.get(f'{kind}_sha256'):
                missing.append(f'native board {kind} does not match its receipt')
        manifest = json.loads((self.audio / 'manifest.json').read_text(encoding='utf-8'))
        if manifest.get('format') != 'skatev-audio-cache' or not manifest.get('complete'):
            missing.append('audio cache incomplete')
        models = self.world.with_suffix('.prop-models')
        for row in self.world.with_suffix('.prop-models.txt').read_text(encoding='utf-8').splitlines():
            model = row.split('#', 1)[0].strip()
            if model and not (models / f'{model.lower()}.svwc').is_file():
                missing.append(f'collision template {model}')
        if missing:
            raise SystemExit('prepared data incomplete:\n  ' + '\n  '.join(missing[:20]))

    # Receipts
    def key(self, scripts):
        h = hashlib.sha256(json.dumps([str(self.a.skate), str(self.gta)]).encode())
        for s in scripts:
            h.update(sha256_file(TOOLS / s).encode())
        return h.hexdigest()

    def run(self):
        self.receipts.mkdir(parents=True, exist_ok=True)
        stages = self.stages()
        if self.a.redo and self.a.redo not in [s[0] for s in stages]:
            raise SystemExit(f'--redo: no stage {self.a.redo}')
        chain = lambda c: [s for s in stages if s[1] == c]
        self.left = [s[0] for s in stages] + ['install' if self.a.install else None]
        self.step(None)
        with ThreadPoolExecutor(2) as pool:
            ran = list(pool.map(self.run_chain, [chain('skate'), chain('gta')]))
        self.run_chain(chain('join'), force=any(ran))
        self.clip_ok = self.clip_matches()

    def run_chain(self, stages, force=False):
        """Runs the stages without a current receipt; once one runs (or from
        --redo), everything after it in the chain reruns. True if any ran."""
        ran = False
        for name, _, scripts, fn in stages:
            receipt, key = self.receipts / f'{name}.json', self.key(scripts)
            force = force or name == self.a.redo
            if not force and receipt.exists() and json.loads(receipt.read_text())['key'] == key:
                self.step(name)
                continue
            if setup_engine.gta_running():  # reads GTA's archives; never compete with the game for them
                raise SystemExit(f'GTA V started: stopped before {name}; rerun to resume')
            print(f'== {name}: {STEPS.get(name, (1, name))[1]}', flush=True)
            receipt.unlink(missing_ok=True)
            t = time.time()
            fn()
            receipt.write_text(json.dumps({'key': key, 'seconds': round(time.time() - t, 1)}))
            force = ran = True
            self.step(name)
        return ran

    def step(self, done):
        """Marks a stage done (None: start) and prints '@progress <percent>' for the setup window."""
        weight = lambda names: sum(STEPS.get(n, (1,))[0] for n in names if n)
        if done is None:
            self.total = weight(self.left)
        else:
            self.left.remove(done)  # list.remove is atomic under the GIL; the two chains share it
        print(f'@progress {100 - 100 * weight(self.left) // self.total}', flush=True)

    def plan(self):
        files = {'SkateVLegacy.asi': PACKAGE / 'SkateVLegacy.asi', 'SkateVRuntime.dll': PACKAGE / 'SkateVRuntime.dll',
                 PACK + r'\dlc.rpf': self.clip / 'dlc.rpf', PACK + r'\hud.gfx': self.clip / 'hud.gfx'}
        return {'files': {k: str(v) for k, v in files.items()},
                'ini': {'DataRoot': str(self.assets), 'WorldCache': str(self.world), 'AudioCache': str(self.audio),
                        'PedCache': str(self.peds), 'PedPoseCollision': 1, 'LiveClip': int(bool(self.clip_ok)),
                        'Stance': self.a.stance or 'Regular', 'Line': ''},
                'keep': ['Line'] + (['Stance'] if self.a.stance is None else [])}  # a reinstall keeps the player's


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--skate', type=Path, required=True, help='Skate 3 (Xbox 360) disc image, or its extracted folder')
    ap.add_argument('--gta', type=Path, required=True, help='GTA V Legacy folder (holds GTA5.exe)')
    ap.add_argument('--out', type=Path, help='where the prepared data lives (default: <GTA folder>/SK8V)')
    ap.add_argument('--install', action='store_true', help='then install into the GTA folder')
    ap.add_argument('--stance', choices=['Regular', 'Goofy'])
    ap.add_argument('--redo', help='rerun this stage and everything after it')
    ap.add_argument('--clean', action='store_true', help='afterwards delete the intermediates (disc and GTA extractions)')
    a = ap.parse_args()
    a.out = a.out or a.gta / setup_engine.DATA
    problems = setup_engine.preflight(a.gta, require_scripthook=a.install)
    if problems:
        raise SystemExit('setup: ' + '; '.join(problems))
    a.out.mkdir(parents=True, exist_ok=True)
    p = Prepare(a)
    p.run()
    if a.install:
        print(f'== install: {STEPS["install"][1]}', flush=True)
        setup_engine.install(a.gta, p.plan())
        p.step('install')
    if a.clean:  # the extractions (about 5 GB); a later run starts over
        for d in (p.work, p.receipts) + (() if a.skate.is_dir() else (p.disc,)):
            shutil.rmtree(d, ignore_errors=True)


if __name__ == '__main__':
    main()

"""Prepare Skate 3's Hall of Meat HUD (the APT `homscoring` movie) from the
user's own game files into the converted data root.

    python tools/prepare-hom-hud.py --game Skate_3 --assets local/skate-data/assets
    python tools/prepare-hom-hud.py --game Skate_3 --assets local/skate-data/assets \
        --movie chyron --gta "<GTA V folder>"

`--movie chyron` prepares Skate's TRAX now-playing banner (GTA's radio on
the board) into `private/hud-chyron`; with `--gta` it also exports GTA's own
radio station logos (HUD atlas `scaleform_generic.rpf/hud.ytd`, read with
build/tools/rage.exe) into `private/hud-chyron/logos`, which replace the EA
logo per station.

Writes `<assets>/private/hud-hom/runtime/homscoring.json` plus the RGBA
textures it references, in the same `skate3-scoring-hud` v1 form as the trick
HUD (`private/hud`). The decoding pipeline is the donor's vendored UI
extractor and HUD preparer, imported in place from
upstream/skate-3-rust-engine-donor/tools (nothing is copied). Output contains
copyrighted assets and stays private (never commit or publish it).

One adaptation: the donor VM has no ToString (APT opcode 0x4B). Each one is
rewritten as `push ""` + `add` at the same offset, which converts the value
the same way (ActionScript string concatenation).
"""
from pathlib import Path
import argparse
import json
import shutil
import sys
import tempfile

from _common import sha256

ROOT = Path(__file__).resolve().parent.parent
DONOR_TOOLS = ROOT / 'upstream' / 'skate-3-rust-engine-donor' / 'tools'
sys.path.insert(0, str(DONOR_TOOLS))

from vendor.skate3_ui.project import extract_project  # noqa: E402
from vendor.skate3_ui.scene_graph import AssetCache, SceneFlattener  # noqa: E402
from vendor.skate3_ui.actions import Actions  # noqa: E402
from prepare_hud import font_mapping  # noqa: E402

BUNDLE = 'data/fe/source/screens/hud2/homscoring'
MANIFEST = 'runtime/homscoring.json'
# --movie: bundle, manifest, destination under <assets>/private. `chyron` is
# the TRAX now-playing banner, shown for GTA's radio on the board.
MOVIES = {
    'homscoring': (BUNDLE, MANIFEST, 'hud-hom'),
    'chyron': ('data/fe/source/screens/main/chyron', 'runtime/chyron.json', 'hud-chyron'),
}


def rewrite_to_string(code):
    """Replace ToString (0x4B) with push "" + add, keeping offsets (jump
    targets resolve to the first instruction at an offset)."""
    out = []
    for row in code:
        if row.get('body'):
            row = dict(row, body=rewrite_to_string(row['body']))
        if row['opcode'] == 0x4B:
            out.append({'offset': row['offset'], 'opcode': 0xA1, 'operand': '', 'next': row['offset']})
            out.append({'offset': row['offset'], 'opcode': 0x47, 'next': row['next']})
        else:
            out.append(row)
    return out


def prepare(game: Path, work: Path, collections: Path, bundle: str = BUNDLE, manifest: str = MANIFEST) -> Path:
    BUNDLE, MANIFEST = bundle, manifest
    extract_project(game, work, prefixes=(BUNDLE,), update=True)
    cache = AssetCache(work)
    mappings = font_mapping(collections, cache)
    bundle = cache.load_bundle(BUNDLE)
    apt_path = work / 'raw' / (BUNDLE + '.apt')
    const_path = apt_path.with_suffix('.const')
    actions = Actions(apt_path.read_bytes(), const_path.read_bytes())
    blocks = {}
    for c in bundle['characters'].values():
        for f in c.get('frames', []):
            for control in f['controls']:
                if control['type_name'] in ('do_action', 'do_init_action') and control.get('actions_offset'):
                    offset = control['actions_offset']
                    blocks[str(offset)] = rewrite_to_string(actions.stream(offset))
    shapes, fonts = {}, {}
    for c in bundle['characters'].values():
        if c['type_name'] == 'shape':
            scene = SceneFlattener(cache, lambda *_: {}).flatten(BUNDLE, c['id'])
            if scene['unresolved']:
                raise ValueError(f"Unresolved HoM HUD shape: {scene['unresolved']}")
            # Untextured `line` primitives are authoring guides (chyron's
            # orange safe-area frame); the HUD renderer draws textures only.
            shapes[str(c['id'])] = [p for p in scene['primitives'] if p['texture']]
        elif c['type_name'] == 'font':
            family = c['font']['name']
            fonts[family] = cache.font_asset(family)
    language = json.loads((work / 'metadata/languages/english_global.json').read_text(encoding='utf-8'))
    result = {
        'format': 'skate3-scoring-hud', 'version': 1,
        'source': {'bundle': BUNDLE,
                   'apt_sha256': sha256(apt_path.read_bytes()),
                   'const_sha256': sha256(const_path.read_bytes()),
                   'collections_sha256': sha256(collections.read_bytes())},
        'characters': list(bundle['characters'].values()),
        'shapes': shapes, 'fonts': fonts, 'actions': blocks,
        'font_mappings': mappings,
        'language': {row['label'].strip(): row['value'] for row in language['entries']},
        'unresolved_fonts': [family for family, asset in fonts.items() if asset is None],
    }
    if result['unresolved_fonts']:
        raise ValueError('Unresolved HoM HUD fonts: ' + ', '.join(result['unresolved_fonts']))
    target = work / MANIFEST
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(result, separators=(',', ':')) + '\n', encoding='utf-8')
    print(f'Prepared {len(shapes)} HoM HUD shapes and {len(blocks)} action blocks')
    return target


# GTA's radio station logos in hud.ytd: texture, cell (x, y) of 128 px ->
# GET_RADIO_STATION_NAME id. White on black; luminance becomes alpha. The
# atlas cells were matched by eye: the mast is Soulwax FM, the
# eagle Blaine County Radio (the two base stations left).
STATION_LOGOS = {
    'gtav_radio_stations_texture_512': {
        (0, 0): 'RADIO_06_COUNTRY', (1, 0): 'RADIO_08_MEXICAN', (2, 0): 'RADIO_04_PUNK',
        (3, 0): 'RADIO_01_CLASS_ROCK', (0, 1): 'RADIO_02_POP', (1, 1): 'RADIO_07_DANCE_01',
        (2, 1): 'RADIO_11_TALK_02', (3, 1): 'RADIO_16_SILVERLAKE', (0, 2): 'RADIO_09_HIPHOP_OLD',
        (1, 2): 'RADIO_18_90S_ROCK', (2, 2): 'RADIO_03_HIPHOP_NEW', (3, 2): 'RADIO_12_REGGAE',
        (0, 3): 'RADIO_13_JAZZ', (1, 3): 'RADIO_14_DANCE_02', (2, 3): 'RADIO_15_MOTOWN', (3, 3): 'OFF',
    },
    'gta_radio_stations_texture02_512': {(0, 0): 'RADIO_17_FUNK', (1, 0): 'RADIO_05_TALK_01'},
    'gta_radio_stations_texture03_128': {(0, 0): 'RADIO_19_USER'},
    'gta_radio_stations_texture04_128': {(0, 0): 'RADIO_20_THELAB'},
    'gta_radio_stations_texture05_128': {(0, 0): 'RADIO_21_DLC_XM17'},
    'gta_radio_stations_texture06_128': {(0, 0): 'RADIO_22_DLC_BATTLE_MIX1_RADIO'},
    'gta_radio_stations_texture07_128': {(0, 0): 'RADIO_23_DLC_XM19_RADIO'},
    'gta_radio_stations_texture08_128': {(0, 0): 'RADIO_35_DLC_HEI4_MLR'},
    'gta_radio_stations_texture09_128': {(0, 0): 'RADIO_27_DLC_PRHEI4'},
    'gta_radio_stations_texture10_128': {(0, 0): 'RADIO_34_DLC_HEI4_KULT'},
    'gta_radio_stations_texture11_128': {(0, 0): 'RADIO_36_AUDIOPLAYER'},
    'gta_radio_stations_texture12_128': {(0, 0): 'RADIO_37_MOTOMAMI'},
}


def station_logos(gta: Path, work: Path, destination: Path) -> int:
    import subprocess
    from PIL import Image
    rage = ROOT / 'build' / 'tools' / 'rage.exe'
    if not rage.exists():
        raise ValueError(f'{rage} missing: run tools/build-rage-cli.ps1')
    common = ['--no-update-check']
    keys = ['--exe', str(gta)]
    subprocess.run([str(rage), *common, 'extract', str(gta / 'update' / 'update.rpf'),
                    '*scaleform_generic.rpf/hud.ytd', '-r', *keys, '-o', str(work / 'gta')], check=True)
    ytd = next((work / 'gta').rglob('hud.ytd'))
    subprocess.run([str(rage), *common, 'textures', str(ytd), '-o', str(work / 'gta-hud')], check=True)
    out = destination / 'logos'
    out.mkdir(parents=True, exist_ok=True)
    rows = []
    for texture, cells in STATION_LOGOS.items():
        image = Image.open(work / 'gta-hud' / f'{texture}.png').convert('L')
        for (x, y), station in cells.items():
            cell = image.crop((x * 128, y * 128, x * 128 + 128, y * 128 + 128))
            rgba = bytearray()
            for value in cell.getdata():
                rgba += bytes((255, 255, 255, value))
            name = station.lower() + '.rgba'
            (out / name).write_bytes(bytes(rgba))
            rows.append({'id': station, 'file': name, 'width': 128, 'height': 128})
    (out / 'logos.json').write_text(json.dumps(rows, indent=1) + '\n', encoding='utf-8')
    return len(rows)


def textures(manifest: dict):
    for shape in manifest['shapes'].values():
        for primitive in shape:
            t = primitive['texture']
            yield t['rgba'], t['width'], t['height']
    for font in manifest['fonts'].values():
        size = font['definition']['textures'][0]
        yield font['texture'], size['width'], size['height']


def contained(root: Path, relative: str) -> Path:
    path = (root / relative).resolve()
    if not path.is_relative_to(root.resolve()) or path == root.resolve():
        raise ValueError(f'HUD path escapes its root: {relative}')
    return path


def install(work: Path, destination: Path, name: str = MANIFEST) -> int:
    manifest = json.loads((work / name).read_text(encoding='utf-8'))
    files = [name]
    for relative, width, height in textures(manifest):
        source = contained(work, relative)
        if width <= 0 or height <= 0 or source.stat().st_size != width * height * 4:
            raise ValueError(f'Invalid RGBA size: {source}')
        files.append(relative)
    staged = destination.with_name(destination.name + '.partial')
    if staged.exists():
        shutil.rmtree(staged)
    for relative in sorted(set(files)):
        target = contained(staged, relative)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(contained(work, relative), target)
    if destination.exists():
        shutil.rmtree(destination)
    staged.rename(destination)
    return len(set(files))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--game', type=Path, required=True, help='extracted Skate 3 folder (contains data/)')
    parser.add_argument('--assets', type=Path, required=True, help='converted data root (<out>/assets)')
    parser.add_argument('--movie', choices=sorted(MOVIES), default='homscoring')
    parser.add_argument('--gta', type=Path, help='GTA V folder: export its radio logos (--movie chyron)')
    args = parser.parse_args()
    bundle, manifest, folder = MOVIES[args.movie]
    assets = args.assets.resolve()
    collections = assets / 'private/stock/skater-collections.json'
    if not collections.exists():
        parser.error(f'{collections} missing: run tools/convert-skate-data.py first')
    # Retail names plus the workspace prefix can exceed legacy path limits.
    with tempfile.TemporaryDirectory(prefix='svhom-') as temporary:
        work = Path(temporary)
        prepare(args.game.resolve(), work, collections, bundle, manifest)
        count = install(work, assets / 'private' / folder, manifest)
        if args.movie == 'chyron' and args.gta:
            print(f'{station_logos(args.gta.resolve(), work, assets / "private" / folder)} station logos')
    print(f'{args.movie} HUD ready: {count} files in {assets / "private" / folder}')


if __name__ == '__main__':
    main()

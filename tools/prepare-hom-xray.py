"""Prepare Skate 3's Hall of Meat x-ray skeleton (the Marquee recipe
`dem_bones_hom`, drawn by the retail HOMSkaterPresEntity) from the user's own
game files into the converted data root.

    python tools/prepare-hom-xray.py --game Skate_3 --assets local/skate-data/assets

Writes `<assets>/private/hom-xray/skeleton.glb`: one skinned primitive per
bone piece (material `Retail_<piece>`, e.g. `Retail_Bones_Skull`), bound to
the same Skate skeleton as `private/skater.glb`. The conversion is the donor's
own Marquee path (asset_pipeline.native_roster's steps: Resources, RX2 parse,
texture decode, character_glb.convert, finalize_glb), imported in place from
upstream/skate-3-rust-engine-donor; nothing is copied. Output contains
copyrighted assets and stays private (never commit or publish it).
"""
from pathlib import Path
import argparse
import os
import shutil
import sys
import tempfile
import xml.etree.ElementTree as ET

sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

ROOT = Path(__file__).resolve().parent.parent
DONOR = ROOT / 'upstream' / 'skate-3-rust-engine-donor'
sys.path.insert(0, str(DONOR))

from PIL import Image  # noqa: E402
from tools.owned_game.big import BigArchive  # noqa: E402
from tools.extract_default_skater import import_rx2_parser, decode_texture  # noqa: E402
from tools.asset_pipeline.character_glb import convert  # noqa: E402
from tools.asset_pipeline.retail_character import RX2, decode_dense_morphs  # noqa: E402
from tools.asset_pipeline.marquee_assets import Resources  # noqa: E402
from tools.asset_pipeline.native_roster import finalize_glb  # noqa: E402

RECIPE = 'dem_bones_hom'
OUTPUT = 'skeleton.glb'


def prepare(game: Path, assets: Path, work: Path) -> Path:
    resources = Resources(BigArchive(game / 'data/content/marquee.big'))
    resources.recipe(RECIPE)  # every authored model and texture is present
    parser = import_rx2_parser(DONOR / 'tools/vendor/utt')

    def extract(path):
        dest = work / 'source' / path
        if not dest.exists():
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(resources.read(path))
        return dest

    root = ET.fromstring(extract(f'data/content/recipe/marquee/{RECIPE}.xml').read_bytes())
    materials = {m.attrib['id']: m for m in root.findall('mat')}
    models, mats = work / 'models', work / 'materials'
    mats.mkdir(parents=True, exist_ok=True)
    recipe = {'components': [], 'morph_assembly': {'expected_targets': {}, 'face_targets': []},
              'preset': {'body_mods': {}}}
    for component in root.findall('comp'):
        slot = component.attrib['n']
        mods = component.findall('mod')
        if len(mods) != 1:
            raise ValueError('Ambiguous x-ray component ' + slot)
        lod = next(l for l in mods[0].findall('lod') if l.get('idx') == '0')
        raw = extract(f"data/content/marquee/model/{RECIPE}/{slot}/{lod.attrib['arenaid']}.rx2")
        dest = models / slot / raw.name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(raw, dest)
        material = materials[lod.find('matinst/matvar').attrib['id']]
        textures = {s.attrib['chn']: s.attrib['id'].lower().removeprefix('0x') for s in material.findall('sp')}
        decoded = work / 'decoded' / (textures['diffuse'] + '.png')
        if not decoded.exists():
            decoded.parent.mkdir(exist_ok=True)
            decode_texture(parser, extract('data/content/marquee/texture/0x' + textures['diffuse'] + '.rx2'), decoded)
        Image.open(decoded).convert('RGBA').save(mats / (slot + '_base_color.png'))
        parsed = RX2.parse_rx2(str(dest))
        mesh = next(m for m in parsed['meshes'] if m.get('positions') and m.get('indices'))
        morphs = decode_dense_morphs(dest, parsed, len(mesh['positions']), RX2)
        recipe['morph_assembly']['expected_targets'][slot] = [m['name'] for m in morphs]
        recipe['components'].append({'slot': slot, 'tint': [1, 1, 1], 'textures': textures,
                                     'alpha_mode': 'OPAQUE'})
    target = work / OUTPUT
    convert(models, assets / 'private', recipe, output=target, materials=mats)
    finalize_glb(target)
    print(f'Prepared {len(recipe["components"])} x-ray bone pieces')
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--game', type=Path, required=True, help='extracted Skate 3 folder (contains data/)')
    parser.add_argument('--assets', type=Path, required=True, help='converted data root (<out>/assets)')
    args = parser.parse_args()
    assets = args.assets.resolve()
    if not (assets / 'private/stock/data/anim/OnBoard.abin').is_file():
        parser.error(f'{assets} has no converted stock animation: run tools/convert-skate-data.py first')
    destination = assets / 'private' / 'hom-xray'
    with tempfile.TemporaryDirectory(prefix='svxray-') as temporary:
        glb = prepare(args.game.resolve(), assets, Path(temporary))
        staged = destination.with_name(destination.name + '.partial')
        if staged.exists():
            shutil.rmtree(staged)
        staged.mkdir(parents=True)
        shutil.copyfile(glb, staged / OUTPUT)
        if destination.exists():
            shutil.rmtree(destination)
        staged.rename(destination)
    print(f'Hall of Meat x-ray ready: {destination / OUTPUT}')


if __name__ == '__main__':
    main()

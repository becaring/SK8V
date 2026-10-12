"""Build the board shop's part models (decks, griptapes, trucks, wheels) as
owned native GTA drawables that share the native board's seven-bone skeleton.

Input: a parts folder as written by the board-parts exporter (one
`<name>.ydr.xml` + `<name>.ytd.xml` and a `<name>/` folder of DDS textures per
part, rigid, in the exporter's board frame) and the native board built by
prepare-board-native.py (skeleton and skinned shader). Output: per part
`<prefix><id>.ydr` + `.ytyp` in `<out>`, posed by the host like the board.

Requires numpy and the locked rage CLI. No retail-derived output may be
committed: the parts are made from the player's own Skate 3 data.
"""
import argparse
import concurrent.futures as futures
import copy
import json
from pathlib import Path
import shutil
import xml.etree.ElementTree as ET

import numpy as np

from _common import REPO, run_rage

# The exporter's board frame (length +Z from the grip end, deck facing -X)
# to the native board's object space (length +Y, deck up +Z): measured on the
# default deck, which both build vertex for vertex.
ROTATION = np.array([[0., 1, 0], [0, 0, -1], [-1, 0, 0]])
ARCHETYPE_FLAGS = 0x20020  # as the native board (prepare-board-native.py)
# glTF board space to GTA's (as prepare-board-native.py).
C = np.array([[1., 0, 0, 0], [0, 0, -1, 0], [0, 1, 0, 0], [0, 0, 0, 1]])
BONES = ['SKATEBOARD_ROOT', 'TRUCK_FRONT', 'TRUCK_BACK', 'LEFT_WHEELFRONT',
         'LEFT_WHEELBACK', 'RIGHT_WHEELFRONT', 'RIGHT_WHEELBACK']
# Part kinds by name: deck-like parts ride on SKATEBOARD_ROOT, trucks split
# between the root (baseplate) and their truck, wheels go to their wheel.
RIGID = ('d', 'g')
TRUCKS = ('t', 'tb', 'th')
WHEELS = ('w',)
# Skate 3 has one deck, one truck and one wheel model, so the native board's
# retail specular map of each fits every finish of that part: it gives the
# grip its grain, the wood and metal their shine, the urethane its sheen.
RETAIL = {'d': 'skateboard', 'g': 'skateboard', 't': 'skatetruck', 'tb': 'skatetruck', 'th': 'skatetruck',
          'w': 'skatewheel'}
# The specular strength, falloff and fresnel the native board uses; a painted
# part keeps its paint's own (matte, gloss, metallic).
SHINE = ('specularIntensityMult', 'specularFalloffMult', 'specularFresnel')


def kind(name):
    return name.split('_')[1]


def floats(row, start, count):
    return [float(v) for v in row[start:start + count]]


def board_template(board_xml):
    root = ET.parse(board_xml).getroot()
    shader = root.find('./ShaderGroup/Shaders/Item')
    # The native board's embedded textures (retail_* maps), by name.
    retail = {t.find('Name').text: t for t in root.iter('Item') if t.find('Usage') is not None and t.find('Name') is not None
              and (t.find('Name').text or '').startswith('retail_')}
    skeleton = root.find('./Skeleton')
    trucks = []
    for geometry in root.iter('Geometries'):
        for item in geometry.findall('Item'):
            for line in item.find('VertexBuffer/Data').text.strip().splitlines():
                row = line.split()
                bone = int(row[7])
                if bone in (0, 1, 2) and float(row[2]) < 0.09 and abs(float(row[0])) < 0.12:
                    trucks.append((floats(row, 0, 3), bone))
    # Truck vertices of the native board: baseplate on the root, hangers on
    # their truck (the deck is above 0.08 and far wider in y).
    truck_points = np.array([p for p, b in trucks if b != 0 or p[2] < 0.088])
    truck_bones = np.array([b for p, b in trucks if b != 0 or p[2] < 0.088])
    return shader, skeleton, truck_points, truck_bones, retail, Path(board_xml).parent


def translation(deck_xml, board_xml):
    """The offset after ROTATION, from the default deck against the board's deck."""
    ours = vertices(ET.parse(deck_xml).getroot())[0]
    theirs = []
    for item in ET.parse(board_xml).getroot().iter('Geometries'):
        first = item.find('Item')
        theirs = [floats(l.split(), 0, 3) for l in first.find('VertexBuffer/Data').text.strip().splitlines()]
        break
    ours = np.array([[float(v) for v in r[0:3]] for r in ours]) @ ROTATION.T
    theirs = np.array(theirs)
    if len(ours) != len(theirs):
        raise ValueError('the default deck and the native board deck differ; cannot place the parts')
    offset = (theirs - ours).mean(axis=0)
    if np.abs(theirs - (ours + offset)).max() > 2e-3:
        raise ValueError('the default deck does not match the native board deck')
    return offset


def binds(board_json):
    """The seven bones' bind matrices in object space, as the host's row-vector
    matrices (rows: X, Y, Z axis, translation): the store poses a preview board
    with them."""
    data = json.loads(board_json.read_text())
    ids = [next(i for i, b in enumerate(data['joints']) if b['name'] == name) for name in BONES]
    out = []
    for i in ids:
        m = C @ np.array(data['joints'][i]['bind']).reshape(4, 4, order='F') @ C.T
        out.append([float(v) for v in m.T.reshape(-1)])
    return out


def vertices(drawable):
    return [[l.split() for l in item.find('VertexBuffer/Data').text.strip().splitlines()]
            for geometry in drawable.iter('Geometries') for item in geometry.findall('Item')]


def bone_of(name_kind, p, truck_points, truck_bones):
    if name_kind in RIGID:
        return 0
    if name_kind in TRUCKS:
        return int(truck_bones[np.argmin(((truck_points - p) ** 2).sum(axis=1))])
    # Wheels: LEFT_WHEELFRONT +x -y, LEFT_WHEELBACK +x +y, RIGHT_* -x.
    return (3 if p[1] < 0 else 4) if p[0] > 0 else (5 if p[1] < 0 else 6)


def text(values):
    return ' '.join(format(float(v), '.9g') for v in values)


def convert(source_dir, name, out_name, offset, template, work):
    shader_template, skeleton, truck_points, truck_bones, retail, retail_dir = template
    painted = 'paint' in name
    # A painted part's own spec map holds its paint's shine (matte, gloss,
    # metallic): it keeps it, with its own strength.
    spec = None if painted else (f'retail_{RETAIL[kind(name)]}_s' if kind(name) in RETAIL and f'retail_{RETAIL[kind(name)]}_s' in retail else None)
    drawable = ET.parse(source_dir / f'{name}.ydr.xml').getroot()
    textures = ET.parse(source_dir / f'{name}.ytd.xml').getroot()
    name_kind = kind(name)
    drawable.find('Name').text = out_name
    # Shaders: the native board's skinned shader with this part's textures and shine.
    group = drawable.find('ShaderGroup')
    shaders = group.find('Shaders')
    for i, ours in enumerate(list(shaders)):
        shader = copy.deepcopy(shader_template)
        params = {p.get('name'): p for p in ours.findall('./Parameters/Item')}
        for p in shader.findall('./Parameters/Item'):
            mine = params.get(p.get('name'))
            if mine is None:
                continue
            if p.get('type') == 'Texture':
                # The exporter's flat placeholder spec map gives way to the retail one.
                own = mine.find('Name').text
                p.find('Name').text = spec if spec and p.get('name') == 'SpecSampler' and own.startswith('skate3board_') else own
            elif p.get('name') in SHINE and spec and not painted:
                continue  # the native board's shine
            else:
                for axis in 'xyzw':
                    if mine.get(axis) is not None:
                        p.set(axis, mine.get(axis))
        shaders.remove(ours)
        shaders.insert(i, shader)
    embedded = copy.deepcopy(textures)
    embedded.tag = 'TextureDictionary'
    if spec:
        for t in list(embedded):
            if (t.find('Name').text or '').startswith('skate3board_s'):
                embedded.remove(t)
        embedded.append(copy.deepcopy(retail[spec]))
    group.insert(0, embedded)
    # Geometry: into the board's object space, rigidly skinned like the board.
    lo, hi = np.full(3, np.inf), np.full(3, -np.inf)
    for model in drawable.iter('DrawableModelsHigh'):
        for item in model.findall('Item'):
            item.find('HasSkin').set('value', '1')
            for geometry in item.find('Geometries').findall('Item'):
                buffer = geometry.find('VertexBuffer')
                layout = buffer.find('Layout')
                names = [e.tag for e in layout]
                if names != ['Position', 'Normal', 'Colour0', 'TexCoord0', 'Tangent']:
                    raise ValueError(f'{name}: unexpected vertex layout {names}')
                for e in list(layout):
                    layout.remove(e)
                for tag in ('Position', 'BlendWeights', 'BlendIndices', 'Normal', 'Colour0', 'TexCoord0', 'Tangent'):
                    ET.SubElement(layout, tag)
                rows = []
                g_lo, g_hi = np.full(3, np.inf), np.full(3, -np.inf)
                for line in buffer.find('Data').text.strip().splitlines():
                    r = line.split()
                    p = ROTATION @ np.array(floats(r, 0, 3)) + offset
                    n = ROTATION @ np.array(floats(r, 3, 3))
                    t = ROTATION @ np.array(floats(r, 12, 3))
                    bone = bone_of(name_kind, p, truck_points, truck_bones)
                    g_lo, g_hi = np.minimum(g_lo, p), np.maximum(g_hi, p)
                    rows.append(f'{text(p)}   255 0 0 0   {bone} 0 0 0   {text(n)}   {" ".join(r[6:10])}   '
                                f'{" ".join(r[10:12])}   {text(t)} {r[15]}')
                buffer.find('Data').text = '\n' + '\n'.join(rows) + '\n'
                geometry.find('BoundingBoxMin').attrib.update(dict(zip('xyz', map(str, g_lo))))
                geometry.find('BoundingBoxMax').attrib.update(dict(zip('xyz', map(str, g_hi))))
                bones = geometry.find('BoneIDs')
                if bones is None:
                    bones = ET.Element('BoneIDs')
                    geometry.insert(list(geometry).index(buffer), bones)
                bones.text = '0, 1, 2, 3, 4, 5, 6'
                lo, hi = np.minimum(lo, g_lo), np.maximum(hi, g_hi)
    centre = (lo + hi) / 2
    drawable.find('BoundingSphereCenter').attrib.update(dict(zip('xyz', map(str, centre))))
    drawable.find('BoundingSphereRadius').set('value', str(float(np.linalg.norm(hi - centre))))
    drawable.find('BoundingBoxMin').attrib.update(dict(zip('xyz', map(str, lo))))
    drawable.find('BoundingBoxMax').attrib.update(dict(zip('xyz', map(str, hi))))
    models = drawable.find('DrawableModelsHigh')
    drawable.insert(list(drawable).index(models), copy.deepcopy(skeleton))
    folder = work / out_name
    folder.mkdir(parents=True, exist_ok=True)
    for dds in (source_dir / name).glob('*.dds'):
        shutil.copyfile(dds, folder / dds.name)
    if spec:
        shutil.copyfile(retail_dir / f'{spec}.dds', folder / f'{spec}.dds')
    xml = folder / f'{out_name}.ydr.xml'
    ET.ElementTree(drawable).write(xml, encoding='UTF-8', xml_declaration=True)
    return xml


def build(rage, xml, out):
    name = xml.name[:-len('.ydr.xml')]
    ydr, ytyp = out / f'{name}.ydr', out / f'{name}.ytyp'
    run_rage(rage, 'resource', 'build', xml, '-o', ydr)
    run_rage(rage, 'ytyp', 'from-drawables', ydr, '-o', ytyp, '--flags', ARCHETYPE_FLAGS)
    return name


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--parts', type=Path, required=True, help='folder of exported parts (parts.txt lists them)')
    parser.add_argument('--board', type=Path, required=True, help="the native board's skatev_board.ydr.xml")
    parser.add_argument('--board-json', type=Path, required=True, help='private/board/board-materials.json')
    parser.add_argument('--rage', type=Path, default=REPO / 'build/tools/rage.exe')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--prefix', default='skatev_')
    parser.add_argument('--only', nargs='*', help='part names to build (default: all in parts.txt)')
    args = parser.parse_args()
    names = args.only or [l.strip() for l in (args.parts / 'parts.txt').read_text().splitlines() if l.strip()]
    # Wear overlays and stickers are decals the skinned opaque shader cannot draw.
    names = [n for n in names if kind(n) in RIGID + TRUCKS + WHEELS]
    template = board_template(args.board)
    offset = translation(args.parts / 'sk3_d_default.ydr.xml', args.board)
    work = args.out / 'xml'
    args.out.mkdir(parents=True, exist_ok=True)
    built = []
    with futures.ThreadPoolExecutor() as pool:
        jobs = []
        for name in names:
            out_name = args.prefix + name[len('sk3_'):]
            xml = convert(args.parts, name, out_name, offset, template, work)
            jobs.append(pool.submit(build, args.rage, xml, args.out))
        for job in jobs:
            built.append(job.result())
    shutil.rmtree(work)
    (args.out / 'parts.json').write_text(json.dumps({'schema': 1, 'offset': [float(v) for v in offset], 'binds': binds(args.board_json),
                                                    'parts': sorted(built)}, indent=1))
    print(f'board parts: {len(built)} models -> {args.out}')


if __name__ == '__main__':
    main()

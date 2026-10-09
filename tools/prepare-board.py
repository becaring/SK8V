"""Export the owned Skate GLB board with per-pixel PBR material inputs.

Run after convert-skate-data.py. Output is retail-derived, local only. No
upstream code is imported or changed. Coordinates and winding remain glTF's;
the host must use the same bone basis conversion as the runtime presentation.
"""
import argparse
import json
import math
from pathlib import Path
import struct

from _common import REPO, donor_commit, donor_head, sha256

DONOR_COMMIT = donor_commit()
SLOTS = ('SkateBoard', 'SkateTruck', 'SkateWheel')
IDENTITY = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]


def parse_glb(data):
    if len(data) < 20:
        raise ValueError('truncated GLB')
    magic, version, length = struct.unpack_from('<4sII', data)
    if magic != b'glTF' or version != 2 or length != len(data):
        raise ValueError('invalid GLB header')
    chunks = {}
    offset = 12
    while offset < length:
        if offset + 8 > length:
            raise ValueError('truncated GLB chunk header')
        size, kind = struct.unpack_from('<I4s', data, offset)
        offset += 8
        if size % 4 or offset + size > length or kind in chunks:
            raise ValueError('invalid GLB chunk')
        chunks[kind] = data[offset:offset + size]
        offset += size
    if b'JSON' not in chunks or b'BIN\0' not in chunks:
        raise ValueError('GLB needs JSON and binary chunks')
    return json.loads(chunks[b'JSON']), chunks[b'BIN\0']


def view(doc, blob, index):
    item = doc['bufferViews'][index]
    start, length = item.get('byteOffset', 0), item['byteLength']
    if item.get('buffer', 0) != 0 or start < 0 or length < 0 or start + length > len(blob):
        raise ValueError('buffer view outside GLB')
    return blob[start:start + length]


def accessor(doc, blob, index):
    item = doc['accessors'][index]
    if 'sparse' in item:
        raise ValueError('sparse accessors unsupported')
    width = {'SCALAR': 1, 'VEC2': 2, 'VEC3': 3, 'VEC4': 4, 'MAT4': 16}[item['type']]
    code = {5121: 'B', 5123: 'H', 5125: 'I', 5126: 'f'}[item['componentType']]
    fmt = '<' + code * width
    size = struct.calcsize(fmt)
    raw = view(doc, blob, item['bufferView'])
    stride = doc['bufferViews'][item['bufferView']].get('byteStride', size)
    start = item.get('byteOffset', 0)
    count = item['count']
    if count < 0 or start < 0 or stride < size or (count and start + (count - 1) * stride + size > len(raw)):
        raise ValueError('accessor outside buffer view')
    rows = [list(struct.unpack_from(fmt, raw, start + i * stride)) for i in range(count)]
    if item.get('normalized') and code != 'f':
        maximum = {'B': 255, 'H': 65535, 'I': 4294967295}[code]
        rows = [[value / maximum for value in row] for row in rows]
    if any(not math.isfinite(v) for row in rows for v in row):
        raise ValueError('nonfinite accessor')
    return rows


def inverse_matrix(columns):
    rows = [[float(columns[c * 4 + r]) for c in range(4)] +
            [float(r == c) for c in range(4)] for r in range(4)]
    for c in range(4):
        pivot = max(range(c, 4), key=lambda r: abs(rows[r][c]))
        rows[c], rows[pivot] = rows[pivot], rows[c]
        divisor = rows[c][c]
        if abs(divisor) < 1e-12:
            raise ValueError('singular bind matrix')
        rows[c] = [v / divisor for v in rows[c]]
        for r in range(4):
            if r != c:
                factor = rows[r][c]
                rows[r] = [a - factor * b for a, b in zip(rows[r], rows[c])]
    return [rows[r][c + 4] for c in range(4) for r in range(4)]


def validate_surface(vertices, indices, joint_count):
    if not vertices or not indices or len(indices) % 3:
        raise ValueError('empty or nontriangle surface')
    if any(i < 0 or i >= len(vertices) for i in indices):
        raise ValueError('index outside vertex stream')
    for v in vertices:
        if any(j < 0 or j >= joint_count for j in v['joints']):
            raise ValueError('joint outside skin')
        if any(w < 0 for w in v['weights']) or abs(sum(v['weights']) - 1) > .001:
            raise ValueError('invalid skin weights')


def export(glb_path, out, manifest_path):
    data = glb_path.read_bytes()
    doc, blob = parse_glb(data)
    manifest = json.loads(manifest_path.read_text())
    skin = doc['skins'][0]
    nodes = skin['joints']
    parents = {child: i for i, n in enumerate(doc['nodes']) for child in n.get('children', [])}
    inverse_binds = accessor(doc, blob, skin['inverseBindMatrices'])
    if len(inverse_binds) != len(nodes):
        raise ValueError('inverse bind count differs from skin')
    joints = []
    for i, n in enumerate(nodes):
        node = doc['nodes'][n]
        parent = parents.get(n)
        joints.append({'name': node['name'], 'parent': nodes.index(parent) if parent in nodes else -1,
                       'inverse_bind': inverse_binds[i], 'bind': inverse_matrix(inverse_binds[i]),
                       'local_bind': node.get('matrix', IDENTITY)})
    textures, pending = [], {}

    def texture(info, slot, semantic):
        if info.get('texCoord', 0) != 0 or info.get('extensions'):
            raise ValueError('unexpected texture coordinates or transform')
        image = doc['images'][doc['textures'][info['index']]['source']]
        if image.get('mimeType') != 'image/png':
            raise ValueError('expected embedded PNG')
        png = view(doc, blob, image['bufferView'])
        if png[:8] != b'\x89PNG\r\n\x1a\n' or png[12:16] != b'IHDR':
            raise ValueError('invalid PNG')
        width, height = struct.unpack_from('>II', png, 16)
        filename = f'{slot}_{semantic}.png'
        pending[filename] = png
        textures.append({'path': filename, 'width': width, 'height': height,
                         'sha256': sha256(png), 'color_space': 'srgb' if semantic == 'base_color' else 'linear'})
        return len(textures) - 1

    surfaces = []
    for primitive in doc['meshes'][0]['primitives']:
        material = doc['materials'][primitive['material']]
        slot = material.get('name', '').removeprefix('Retail_')
        if slot not in SLOTS:
            continue
        if primitive.get('mode', 4) != 4:
            raise ValueError('board primitive is not triangles')
        streams = {key: accessor(doc, blob, primitive['attributes'][attribute]) for key, attribute in
                   [('position', 'POSITION'), ('normal', 'NORMAL'), ('uv', 'TEXCOORD_0'),
                    ('joints', 'JOINTS_0'), ('weights', 'WEIGHTS_0')]}
        count = len(streams['position'])
        if any(len(rows) != count for rows in streams.values()):
            raise ValueError('vertex stream lengths differ')
        vertices = [{key: rows[i] for key, rows in streams.items()} for i in range(count)]
        indices = [r[0] for r in accessor(doc, blob, primitive['indices'])]
        validate_surface(vertices, indices, len(joints))
        pbr = material['pbrMetallicRoughness']
        component = next(c for c in manifest['components'] if c['slot'] == slot)
        surfaces.append({'name': material['name'], 'vertices': vertices, 'indices': indices,
                         'base_color_texture': texture(pbr['baseColorTexture'], slot, 'base_color'),
                         'normal_texture': texture(material['normalTexture'], slot, 'normal'),
                         'normal_scale': material['normalTexture'].get('scale', 1.0),
                         'base_color_factor': pbr.get('baseColorFactor', [1, 1, 1, 1]),
                         'metallic': pbr['metallicFactor'], 'roughness': pbr['roughnessFactor'],
                         'alpha_mode': material.get('alphaMode', 'OPAQUE'),
                         'alpha_cutoff': material.get('alphaCutoff', .5),
                         'double_sided': material.get('doubleSided', False),
                         'retail_material_id': component['material_id'],
                         'retail_texture_ids': component['textures']})
    if sorted(s['name'] for s in surfaces) != sorted('Retail_' + s for s in SLOTS):
        raise ValueError('expected exactly deck, truck and wheel surfaces')
    result = {'schema': 1, 'coordinate_system': 'original-glTF-skin-bind-space',
              'index_winding': 'original-glTF-CCW', 'matrix_layout': 'column-major',
              'normal_convention': 'glTF tangent-space RGB; donor reconstructed retail DXT5nm AG; no green inversion',
              'joints': joints, 'surfaces': surfaces, 'textures': textures,
              'receipt': {'source_glb_sha256': sha256(data), 'donor_commit': DONOR_COMMIT,
                          'material_policy': 'Pinned donor character_glb.py authored PBR approximation; metallic and roughness are not recovered retail shader constants. No board specular or roughness map is declared in the retail manifest.',
                          'manifest_sha256': sha256(manifest_path.read_bytes())}}
    out.mkdir(parents=True, exist_ok=True)
    for filename, png in pending.items():
        (out / filename).write_bytes(png)
    destination = out / 'board-materials.json'
    temporary = destination.with_suffix('.json.partial')
    temporary.write_text(json.dumps(result, separators=(',', ':'), allow_nan=False), encoding='utf-8')
    temporary.replace(destination)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets', type=Path, default=REPO / 'local/skate-data/assets')
    parser.add_argument('--out', type=Path)
    args = parser.parse_args()
    donor = REPO / 'upstream/skate-3-rust-engine-donor'
    if donor_head() != DONOR_COMMIT:
        raise ValueError('donor pin mismatch; run verify-upstreams.ps1')
    out = args.out or args.assets / 'private/board'
    result = export(args.assets / 'private/skater.glb', out,
                    donor / 'tools/default_skater_retail_manifest.json')
    print(json.dumps({'output': str((out / 'board-materials.json').resolve()),
                      'joints': len(result['joints']), 'textures': len(result['textures']),
                      'surfaces': [{'name': s['name'], 'vertices': len(s['vertices']),
                                    'triangles': len(s['indices']) // 3} for s in result['surfaces']]}, indent=2))


if __name__ == '__main__':
    main()

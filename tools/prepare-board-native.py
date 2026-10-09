"""Build an owned native GTA skeletal board from prepare-board.py output.

Requires numpy, Pillow and the locked rage CLI. No retail-derived output may
be committed. Shader schema comes from an owned Legacy skinned drawable.
"""
import argparse
import copy
import json
from pathlib import Path
import struct
import xml.etree.ElementTree as ET
import numpy as np
from PIL import Image

from _common import REPO, run_rage, sha256, verify_pins

C = np.array([[1., 0, 0, 0], [0, 0, -1, 0], [0, 1, 0, 0], [0, 0, 0, 1]])
BONES = ['SKATEBOARD_ROOT', 'TRUCK_FRONT', 'TRUCK_BACK', 'LEFT_WHEELFRONT',
         'LEFT_WHEELBACK', 'RIGHT_WHEELFRONT', 'RIGHT_WHEELBACK']
REFERENCE = REPO / 'local/gta-meta/base/x64w/dlcpacks/mppilot/dlc.rpf/x64/levels/gta5/props/mppilot_props.rpf/pil_p_para_bag_pilot_s.ydr'
# Legacy3889 creator +D18809 requires internal archetype+50 bit4, derived
# from source bit17 at +6174E3. Preserve rage's existing fixed flag (0x20)
# while enabling native object creation. The host still disables simulation.
# Owned-image evidence: evidence/2026-10-02/native-board-host.md.
ARCHETYPE_FLAGS = 0x20020


def rigid_influence(joints, weights, source_ids):
    if any(not np.isfinite(w) or w < 0 for w in weights):
        raise ValueError('invalid native weight')
    active = [(j, w) for j, w in zip(joints, weights) if w > 0]
    if len(active) != 1 or abs(active[0][1] - 1) > 1e-6 or active[0][0] not in source_ids:
        raise ValueError('expected one mapped rigid influence per original board vertex')
    return [source_ids.index(j) if w else 0 for j, w in zip(joints, weights)]


def verify_dds(path, expected):
    raw = path.read_bytes()
    # Classic RGB DDS, no DX10 sRGB resource format and no gamma conversion.
    if raw[:4] != b'DDS ' or raw[84:88] != b'\0' * 4 or not struct.unpack_from('<I', raw, 80)[0] & 0x40:
        raise ValueError('expected uncompressed linear-format DDS')
    actual = np.asarray(Image.open(path).convert('RGBA'))
    np.testing.assert_array_equal(actual, np.asarray(expected.convert('RGBA')))
    return sha256(actual.tobytes())


def rotation_matrix(q):
    x, y, z, w = np.asarray(q) / np.linalg.norm(q)
    return np.array([[1 - 2 * (y*y + z*z), 2 * (x*y - z*w), 2 * (x*z + y*w)],
                     [2 * (x*y + z*w), 1 - 2 * (x*x + z*z), 2 * (y*z - x*w)],
                     [2 * (x*z - y*w), 2 * (y*z + x*w), 1 - 2 * (x*x + y*y)]])


def quaternion(m):
    # Matrix -> normalized xyzw; stable for both small rotations and pi.
    values = [1 + m[0, 0] - m[1, 1] - m[2, 2], 1 - m[0, 0] + m[1, 1] - m[2, 2],
              1 - m[0, 0] - m[1, 1] + m[2, 2], 1 + np.trace(m)]
    i = int(np.argmax(values)); q = np.zeros(4); q[i] = np.sqrt(max(values[i], 0)) / 2
    if i == 3:
        q[:3] = [m[2, 1] - m[1, 2], m[0, 2] - m[2, 0], m[1, 0] - m[0, 1]]
        q[:3] /= 4 * q[3]
    else:
        j, k = (i + 1) % 3, (i + 2) % 3
        q[j] = (m[j, i] + m[i, j]) / (4 * q[i])
        q[k] = (m[k, i] + m[i, k]) / (4 * q[i])
        q[3] = (m[k, j] - m[j, k]) / (4 * q[i])
    return q / np.linalg.norm(q)


def tangents(p, n, uv, indices):
    ts, bs = np.zeros_like(p), np.zeros_like(p)
    for a, b, c in np.asarray(indices).reshape(-1, 3):
        dp1, dp2, du1, du2 = p[b] - p[a], p[c] - p[a], uv[b] - uv[a], uv[c] - uv[a]
        # Match the donor derivative basis including its negative tangent.
        for i in (a, b, c):
            ts[i] -= np.cross(dp2, n[i]) * du1[0] + np.cross(n[i], dp1) * du2[0]
            bs[i] += np.cross(dp2, n[i]) * du1[1] + np.cross(n[i], dp1) * du2[1]
    result = []
    for normal, t, b in zip(n, ts, bs):
        t -= normal * np.dot(normal, t)
        if np.linalg.norm(t) < 1e-12:
            t = np.cross(normal, [1, 0, 0] if abs(normal[0]) < .9 else [0, 1, 0])
        t /= np.linalg.norm(t)
        result.append([*t, -1. if np.dot(np.cross(normal, t), b) < 0 else 1.])
    return np.asarray(result)


def node(parent, tag, text=None, **attributes):
    result = ET.SubElement(parent, tag, {k: str(v) for k, v in attributes.items()})
    if text is not None: result.text = str(text)
    return result


def vec(parent, tag, values):
    return node(parent, tag, **dict(zip('xyzw', (format(float(v), '.9g') for v in values))))


def export_archetype(rage, ydr, out):
    ytyp = out / 'skatev_board.ytyp'
    run_rage(rage, 'ytyp', 'from-drawables', ydr, '-o', ytyp, '--flags', ARCHETYPE_FLAGS)
    dumped = out / 'skatev_board.ytyp.roundtrip.xml'
    run_rage(rage, 'resource', 'dump', ytyp, '-o', dumped)
    archetypes = ET.parse(dumped).findall('./archetypes/Item')
    if len(archetypes) != 1:
        raise ValueError('expected one native board archetype')
    archetype = archetypes[0]
    flags = int(archetype.find('flags').get('value'))
    if flags != ARCHETYPE_FLAGS or not flags & 0x20000:
        raise ValueError('board archetype lacks Legacy native creation capability')
    if archetype.findtext('assetType') != 'ASSET_TYPE_DRAWABLE':
        raise ValueError('board archetype must reference the skeletal drawable')
    if archetype.findtext('physicsDictionary'):
        raise ValueError('presentation board must not acquire GTA physics geometry')
    return flags


def export(board_path, lighting_path, reference, rage, out):
    data = json.loads(board_path.read_text())
    lighting = json.loads(lighting_path.read_text())['materials']
    out.mkdir(parents=True, exist_ok=True)
    reference_xml = out / 'shader-reference.ydr.xml'
    run_rage(rage, 'resource', 'dump', reference, '-o', reference_xml)
    template_root = ET.parse(reference_xml)
    template = next(s for s in template_root.findall('./ShaderGroup/Shaders/Item')
                    if {'DiffuseSampler', 'BumpSampler', 'SpecSampler'} <=
                    {p.get('name') for p in s.findall('./Parameters/Item')})
    source_ids = [next(i for i, b in enumerate(data['joints']) if b['name'] == name) for name in BONES]
    binds = [C @ np.array(data['joints'][i]['bind']).reshape(4, 4, order='F') @ C.T for i in source_ids]
    parents = []
    for i in source_ids:
        parent = data['joints'][i]['parent']
        while parent >= 0 and parent not in source_ids:
            parent = data['joints'][parent]['parent']
        parents.append(source_ids.index(parent) if parent in source_ids else -1)
    if parents[0] != -1: raise ValueError('board root is not independent')
    positions = [np.asarray([v['position'] for v in s['vertices']]) @ C[:3, :3].T for s in data['surfaces']]
    all_positions = np.concatenate(positions)
    minimum, maximum = all_positions.min(axis=0), all_positions.max(axis=0)
    center = (minimum + maximum) / 2
    root = ET.Element('Drawable'); node(root, 'Name', 'skatev_board')
    vec(root, 'BoundingSphereCenter', center)
    node(root, 'BoundingSphereRadius', value=np.linalg.norm(all_positions - center, axis=1).max())
    vec(root, 'BoundingBoxMin', minimum); vec(root, 'BoundingBoxMax', maximum)
    for level in ['High', 'Med', 'Low', 'Vlow']:
        node(root, 'LodDist' + level, value=9998)
        node(root, 'Flags' + level, value=1 if level == 'High' else 0)
    group = node(root, 'ShaderGroup'); dictionary = node(group, 'TextureDictionary'); shaders = node(group, 'Shaders')
    texture_checks, texture_images = [], {}
    for s in data['surfaces']:
        prefix = s['name'].lower()
        names = {'DiffuseSampler': prefix + '_d', 'BumpSampler': prefix + '_n', 'SpecSampler': prefix + '_s'}
        base = Image.open(board_path.parent / data['textures'][s['base_color_texture']]['path']).convert('RGBA')
        normal = Image.open(board_path.parent / data['textures'][s['normal_texture']]['path']).convert('RGBA')
        mask = np.asarray(base)[:, :, 3].astype(float) / 255
        spec = Image.fromarray(np.uint8(np.rint(mask * mask * 255))).convert('RGBA')
        for sampler, image in zip(names, (base, normal, spec)):
            name = names[sampler]; png = out / (name + '.png'); image.save(png)
            run_rage(rage, 'textures', 'encode', png, '-o', out / (name + '.dds'), '-f', 'rgba8')
            pixel_sha = verify_dds(out / (name + '.dds'), image)
            texture_images[name] = image
            texture_checks.append({'name': name, 'sampler': sampler, 'level0_rgba_sha256': pixel_sha,
                                   'encoding': 'linear-format A8R8G8B8; raw original pixels, no gamma transform'})
            t = node(dictionary, 'Item'); node(t, 'Name', name); node(t, 'Unk32', value=128)
            node(t, 'Usage', 'NORMAL' if sampler == 'BumpSampler' else 'DIFFUSE')
            node(t, 'UsageFlags', 'NOT_HALF'); node(t, 'ExtraFlags', value=0)
            node(t, 'FileName', name + '.dds')
        shader = copy.deepcopy(template)
        for p in shader.findall('./Parameters/Item'):
            name = p.get('name')
            if name in names: p.find('Name').text = names[name]
            if name == 'bumpiness': p.set('x', str(s['normal_scale']))
            # Skate's row2 is not GTA's scale: taken raw (intensity 5, falloff 1)
            # the lobe was near-flat and took the light's colour (orange under
            # sodium lamps). GTA props use intensity 0.1..1, falloff 22..200.
            # ponytail: linear map anchored to the reference's falloff 40; tune by eye.
            if name == 'specularIntensityMult': p.set('x', str(max(lighting[s['name']]['params'][2][:3]) / 5))
            if name == 'specularFalloffMult': p.set('x', str(40 * lighting[s['name']]['params'][2][3]))
            # normal_spec deferred PS: gbuffer1.w (puddle mask) = sat((wetnessMultiplier-0.2)*10)
            # on upward normals, so 0.4 drew the ground's puddle onto the deck.
            if name == 'wetnessMultiplier': p.set('x', '0.2')
        shaders.append(shader)
    skeleton = node(root, 'Skeleton')
    for field in ['Unknown1C', 'Unknown50', 'Unknown54', 'Unknown58']: node(skeleton, field, value=0)
    bones = node(skeleton, 'Bones')
    for i, name in enumerate(BONES):
        local = np.linalg.inv(binds[parents[i]]) @ binds[i] if parents[i] >= 0 else binds[i]
        scale = np.linalg.norm(local[:3, :3], axis=0)
        b = node(bones, 'Item'); node(b, 'Name', name)
        for key, value in [('Tag', i), ('Index', i), ('ParentIndex', parents[i]), ('SiblingIndex', -1)]: node(b, key, value=value)
        node(b, 'Flags', 'RotX, RotY, RotZ, TransX, TransY, TransZ')
        vec(b, 'Translation', local[:3, 3]); vec(b, 'Rotation', quaternion(local[:3, :3] / scale))
        vec(b, 'Scale', scale); vec(b, 'TransformUnk', [0, 0, 0, 0])
    model = node(node(root, 'DrawableModelsHigh'), 'Item')
    for key, value in [('RenderMask', 255), ('Flags', 0), ('HasSkin', 1), ('BoneIndex', 0), ('Unknown1', 0)]: node(model, key, value=value)
    geometries = node(model, 'Geometries')
    for si, (s, p) in enumerate(zip(data['surfaces'], positions)):
        normals = np.array([v['normal'] for v in s['vertices']]) @ C[:3, :3].T
        uv = np.array([v['uv'] for v in s['vertices']]); tangent = tangents(p, normals, uv, s['indices'])
        g = node(geometries, 'Item'); node(g, 'ShaderIndex', value=si)
        vec(g, 'BoundingBoxMin', [*p.min(axis=0), 0]); vec(g, 'BoundingBoxMax', [*p.max(axis=0), 0])
        node(g, 'BoneIDs', ', '.join(map(str, range(7))))
        vb = node(g, 'VertexBuffer'); node(vb, 'Flags', value=0); layout = node(vb, 'Layout', type='GTAV1')
        for semantic in ['Position', 'BlendWeights', 'BlendIndices', 'Normal', 'Colour0', 'TexCoord0', 'Tangent']: node(layout, semantic)
        rows = []
        for vi, v in enumerate(s['vertices']):
            weights = np.rint(np.asarray(v['weights']) * 255).astype(int)
            ids = rigid_influence(v['joints'], v['weights'], source_ids)
            values = [*p[vi], *weights, *ids, *normals[vi], 255, 255, 255, 255, *uv[vi], *tangent[vi]]
            rows.append(' '.join(format(float(x), '.9g') for x in values))
        node(vb, 'Data', '\n' + '\n'.join(rows) + '\n')
        node(node(g, 'IndexBuffer'), 'Data', ' '.join(map(str, s['indices'])))
    node(root, 'Lights')
    xml = out / 'skatev_board.ydr.xml'; ET.indent(root); ET.ElementTree(root).write(xml, encoding='utf-8', xml_declaration=True)
    ydr = out / 'skatev_board.ydr'
    run_rage(rage, 'resource', 'build', xml, '-o', ydr, '--strict')
    archetype_flags = export_archetype(rage, ydr, out)
    dumped = out / 'skatev_board.roundtrip.xml'; run_rage(rage, 'resource', 'dump', ydr, '-o', dumped)
    back = ET.parse(dumped)
    checks = []
    converted_geometries = back.findall('./DrawableModelsHigh/Item/Geometries/Item')
    converted_bones = back.findall('./Skeleton/Bones/Item')
    if len(converted_geometries) != 3 or len(converted_bones) != 7:
        raise ValueError('native geometry or skeleton count changed')
    if [b.findtext('Name') for b in converted_bones] != BONES:
        raise ValueError('native bone order changed')
    roundtrip_worlds = []
    for i, bone in enumerate(converted_bones):
        xyz = lambda name, axes: np.array([float(bone.find(name).get(a)) for a in axes])
        local = np.eye(4)
        local[:3, :3] = rotation_matrix(xyz('Rotation', 'xyzw')) @ np.diag(xyz('Scale', 'xyz'))
        local[:3, 3] = xyz('Translation', 'xyz')
        parent = int(bone.find('ParentIndex').get('value'))
        if parent != parents[i] or int(bone.find('Tag').get('value')) != i:
            raise ValueError('native bone tag/parent changed')
        world = roundtrip_worlds[parent] @ local if parent >= 0 else local
        roundtrip_worlds.append(world)
        original_inverse = C @ np.array(data['joints'][source_ids[i]]['inverse_bind']).reshape(4, 4, order='F') @ C.T
        np.testing.assert_allclose(world @ original_inverse, np.eye(4), atol=1e-6)
    for name, image in texture_images.items():
        verify_dds(out / (name + '.dds'), image)
    if len(back.findall('./ShaderGroup/TextureDictionary/Item')) != 9 or len(back.findall('./ShaderGroup/Shaders/Item')) != 3:
        raise ValueError('native material data lost')
    for si, (original, converted) in enumerate(zip(data['surfaces'], converted_geometries)):
        indices = list(map(int, converted.findtext('./IndexBuffer/Data').split()))
        rows = [[float(x) for x in line.split()] for line in converted.findtext('./VertexBuffer/Data').splitlines() if line.strip()]
        if indices != original['indices'] or len(rows) != len(original['vertices']): raise ValueError('geometry changed on native roundtrip')
        np.testing.assert_allclose(np.array(rows)[:, :3], positions[si], atol=1e-6)
        np.testing.assert_allclose(np.array(rows)[:, 18:20], [v['uv'] for v in original['vertices']], atol=1e-6)
        for vi, row in enumerate(rows):
            if abs(sum(row[3:7]) - 255) > .01 or not all(0 <= x < 7 for x in row[7:11]): raise ValueError('native skin invalid')
            if abs(np.linalg.norm(row[-4:-1]) - 1) > .001: raise ValueError('native tangent invalid')
            if abs(np.dot(row[11:14], row[-4:-1])) > .001: raise ValueError('native tangent is not perpendicular to normal')
            for weight, joint, source_joint, source_weight in zip(row[3:7], row[7:11], original['vertices'][vi]['joints'], original['vertices'][vi]['weights']):
                if weight and (source_ids[int(joint)] != source_joint or abs(weight / 255 - source_weight) > 1e-5):
                    raise ValueError('native bone influence differs from source')
        checks.append({'surface': original['name'], 'vertices': len(rows), 'triangles': len(indices) // 3})
    receipt = {'shader_reference': str(reference.resolve()), 'shader_reference_sha256': sha256(reference.read_bytes()),
               'shader_name': template.findtext('Name'), 'shader_file': template.findtext('FileName'),
               'source_board_sha256': sha256(board_path.read_bytes()),
               'bone_names': BONES, 'bone_tags': list(range(7)), 'bone_parents': parents,
               'archetype_flags': archetype_flags,
               'archetype_flag_basis': 'Legacy3889 +6174E3 maps source bit17 to internal archetype+50 bit4 required by creator +D18809; preserve source fixed bit5. Host disables physics.',
               'inverse_binds_gta': [np.linalg.inv(b).flatten(order='F').tolist() for b in binds],
               'space': 'C=(x,-z,y); vertices C*p; bone worlds C*bind*C^-1',
               'adaptation': 'Original diffuse/normal pixels; spec map is original diffuse alpha squared. GTA skinned normal/spec shader and wetness/Fresnel defaults from owned reference; spec intensity=max(retail row2 RGB)/5, falloff=40*retail row2 W (GTA prop range); wetnessMultiplier 0.2 so the deck takes no puddle mask. GTA illumination/reflection response is an adaptation, not retail shader parity or metallic-roughness equivalence.',
               'verification': checks, 'dds_pixel_verification': texture_checks,
               'bone_bind_roundtrip_max_error': max(float(np.max(np.abs(a - b))) for a, b in zip(binds, roundtrip_worlds)),
               'rage_binary_sha256': sha256(rage.read_bytes()),
               'ydr_sha256': sha256(ydr.read_bytes()),
               'ytyp_sha256': sha256((out / 'skatev_board.ytyp').read_bytes())}
    (out / 'receipt.json').write_text(json.dumps(receipt, indent=2))
    print(json.dumps({'output': str(ydr.resolve()), 'verified': checks}, indent=2))


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--board', type=Path, default=REPO / 'local/skate-data/assets/private/board/board-materials.json')
    p.add_argument('--lighting', type=Path, default=REPO / 'local/skate-data/assets/private/character-lighting.json')
    p.add_argument('--shader-reference', type=Path, default=REFERENCE)
    p.add_argument('--rage', type=Path, default=REPO / 'build/tools/rage.exe')
    p.add_argument('--out', type=Path, default=REPO / 'local/skate-data/assets/private/board-native')
    a = p.parse_args(); verify_pins(); export(a.board, a.lighting, a.shader_reference, a.rage, a.out)

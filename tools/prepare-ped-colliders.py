"""Export owned, authored GTA body bounds as invisible bone-posed colliders.

Input is an owned Legacy YFT; no radii or body dimensions are invented. Child
primitives and bone-relative offsets survive a checked native roundtrip.
Requires numpy and the locked rage CLI. Generated assets remain private.
"""
import argparse
import copy
import itertools
import json
from pathlib import Path
import struct
import xml.etree.ElementTree as ET
import zlib
import numpy as np


from _common import REPO, run_rage, sha256, verify_pins

VECTOR_FIELDS = ('BoxMin', 'BoxMax', 'BoxCenter', 'SphereCenter', 'Inertia')
FLOAT_FIELDS = ('SphereRadius', 'Margin', 'Volume')
INTEGER_FIELDS = ('MaterialIndex', 'MaterialColourIndex', 'ProceduralID', 'RoomID',
                  'PedDensity', 'UnkFlags', 'PolyFlags', 'UnkType')
TYPE_MASK, INCLUDE_MASK, ARCHETYPE_FLAGS = 0x2000, 0x600, 0x20020


def resource_size(flags):
    # Public RSC7 section-size bitfields, as documented in locked rage-formats.
    groups = ((27, 1, 0), (26, 1, 1), (25, 1, 2), (24, 1, 3),
              (17, 127, 4), (11, 63, 5), (7, 15, 6), (5, 3, 7), (4, 1, 8))
    return (512 << (flags & 15)) * sum(((flags >> shift) & mask) << power for shift, mask, power in groups)


class Resource:
    def __init__(self, raw):
        if len(raw) < 16 or raw[:4] != b'RSC7':
            raise ValueError('expected owned Legacy RSC7 YFT')
        sys_flags, gfx_flags = struct.unpack_from('<II', raw, 8)
        self.system_size = resource_size(sys_flags)
        expected = self.system_size + resource_size(gfx_flags)
        try:
            self.data = zlib.decompress(raw[16:], -15)
        except zlib.error as error:
            raise ValueError('invalid compressed YFT') from error
        if len(self.data) != expected:
            raise ValueError('YFT section sizes disagree with decompressed data')

    def at(self, offset, size):
        if offset < 0 or size < 0 or offset + size > self.system_size:
            raise ValueError('YFT field outside system section')
        return self.data[offset:offset + size]

    def read(self, code, offset):
        values = struct.unpack('<' + code, self.at(offset, struct.calcsize('<' + code)))
        return values[0] if len(values) == 1 else list(values)

    def pointer(self, offset, size=1):
        address = self.read('Q', offset)
        at = address - 0x50000000
        self.at(at, size)
        return at

    def matrix(self, at):
        value = np.asarray(self.read('16f', at), dtype=float).reshape(4, 4)
        # RAGE packs metadata in column3 of transform arrays; basis and row3
        # translation are the affine transform. The matrix last column is not.
        value[:, 3] = [0, 0, 0, 1]
        if not np.isfinite(value).all(): raise ValueError('nonfinite source transform')
        return value

    def string(self, pointer_offset):
        start = self.pointer(pointer_offset)
        end = self.data.find(b'\0', start, self.system_size)
        if end < 0: raise ValueError('unterminated source name')
        return self.data[start:end].decode('utf-8')


def decode_source(path):
    raw = path.read_bytes(); reader = Resource(raw)
    fragment_name = reader.string(0x58)
    physics_group = reader.pointer(0xf0, 0x20)
    lod = reader.pointer(physics_group + 0x10, 0x130)
    composite = reader.pointer(lod + 0xe8, 0xb0)
    if reader.read('B', composite + 0x10) != 10:
        raise ValueError('LOD1 physics bound is not composite')
    count = reader.read('H', composite + 0xa0)
    if count == 0 or count != reader.read('B', lod + 0x11d):
        raise ValueError('fragment children and physics bounds disagree')
    bounds = reader.pointer(composite + 0x70, count * 8)
    transforms = reader.pointer(composite + 0x78, count * 64)
    fragments = reader.pointer(lod + 0xd0, count * 8)
    drawable_at = reader.pointer(0x30, 0x20)
    skeleton = reader.pointer(drawable_at + 0x18, 0x60)
    bone_count = reader.read('H', skeleton + 0x5e)
    bones = reader.pointer(skeleton + 0x20, bone_count * 80)
    inverse = reader.pointer(skeleton + 0x28, bone_count * 64)
    by_tag = {}
    for i in range(bone_count):
        at = bones + i * 80; tag = reader.read('H', at + 0x44)
        name_at = reader.pointer(at + 0x38)
        end = reader.data.find(b'\0', name_at, reader.system_size)
        if end < 0 or tag in by_tag: raise ValueError('invalid source bone names/tags')
        by_tag[tag] = (i, reader.data[name_at:end].decode('utf-8'))
    result = []
    for i in range(count):
        bound = reader.pointer(bounds + i * 8, 112)
        fragment = reader.pointer(fragments + i * 8, 0x14)
        tag = reader.read('H', fragment + 0x12)
        if tag not in by_tag: raise ValueError('physics child references absent bone tag')
        bi, name = by_tag[tag]
        kind = {1: 'Capsule', 3: 'Box'}.get(reader.read('B', bound + 0x10))
        if kind is None: raise ValueError('only original capsule/box primitives are supported')
        if kind == 'Capsule' and reader.at(bound + 112, 16) != bytes(16):
            raise ValueError('nonzero capsule tail cannot be preserved by locked serializer')
        fields = {key: reader.read('3f', bound + offset) for key, offset in
                  [('BoxMin', 0x30), ('BoxMax', 0x20), ('BoxCenter', 0x40), ('SphereCenter', 0x50), ('Inertia', 0x60)]}
        fields.update({key: reader.read('f', bound + offset) for key, offset in [('SphereRadius', 0x14), ('Margin', 0x2c), ('Volume', 0x6c)]})
        fields.update({key: reader.read('B', bound + offset) for key, offset in
                       [('MaterialIndex', 0x4c), ('MaterialColourIndex', 0x5d), ('ProceduralID', 0x4d), ('UnkFlags', 0x4f), ('PolyFlags', 0x5c)]})
        packed = reader.read('B', bound + 0x4e)
        fields.update(RoomID=packed & 31, PedDensity=packed >> 5, UnkType=reader.read('I', bound + 0x3c))
        if fields['UnkType'] > 255: raise ValueError('UnkType exceeds locked XML preservation range')
        bind = reader.matrix(transforms + i * 64); inverse_bind = reader.matrix(inverse + bi * 64)
        relative = bind @ inverse_bind
        np.testing.assert_allclose(relative @ np.linalg.inv(inverse_bind), bind, atol=1e-6)
        result.append({'child': i, 'kind': kind, 'bone_tag': tag, 'bone_name': name,
                       'skeleton_index': bi, 'fragment_group_raw': reader.read('H', fragment + 0x10),
                       'bound_va': hex(bound + 0x50000000),
                       'bound_raw': reader.at(bound, 128 if kind == 'Capsule' else 112).hex(),
                       'fields': fields, 'bind_transform': bind.flatten().tolist(),
                       'inverse_bone_bind': inverse_bind.flatten().tolist(),
                       'bone_relative_transform': relative.flatten().tolist()})
    return {'source': str(path.resolve()), 'sha256': sha256(raw), 'fragment_name': fragment_name,
            'matrix_convention': 'RAGE row-vector; child world = bone_relative_transform @ posed_bone_world',
            'children': result}


def validate_binding(source, binding):
    if binding.get('source_sha256') != source['sha256']:
        raise ValueError('source binding policy does not match the YFT hash')
    if binding.get('binding_policy') != 'require_live_asset_match':
        raise ValueError('body colliders require runtime asset matching before creation')
    if binding.get('expected_frag_name') != source['fragment_name']:
        raise ValueError('expected live fragment does not match the owned source name')
    if binding.get('binding_verified') is not False:
        raise ValueError('offline export cannot certify a live player binding')


def node(parent, tag, text=None, **attrs):
    n = ET.SubElement(parent, tag, {k: str(v) for k, v in attrs.items()})
    if text is not None: n.text = str(text)
    return n


def number(x):
    # Matrix products are f64; first round once to the native representation.
    # Formatting f64 directly near an f32 midpoint can introduce a second
    # rounding and change the serialized transform by one ULP.
    return format(float(np.float32(x)), '.9g')


def vector(parent, tag, values):
    return node(parent, tag, **dict(zip('xyz', map(number, values))))


def fields_xml(parent, fields):
    for key in VECTOR_FIELDS: vector(parent, key, fields[key])
    for key in FLOAT_FIELDS: node(parent, key, value=number(fields[key]))
    for key in INTEGER_FIELDS: node(parent, key, value=fields[key])


def transformed_bounds(child):
    matrix = np.asarray(child['bone_relative_transform']).reshape(4, 4)
    if not np.isfinite(matrix).all() or not np.allclose(matrix[:, 3], [0, 0, 0, 1], atol=1e-7):
        raise ValueError('invalid bone-relative transform')
    f = child['fields']
    corners = np.array([[*point, 1.] for point in itertools.product(*zip(f['BoxMin'], f['BoxMax']))]) @ matrix
    if not np.isfinite(corners).all(): raise ValueError('nonfinite source bound')
    return corners[:, :3].min(axis=0), corners[:, :3].max(axis=0)


def drawable(child, model):
    if child['kind'] not in ('Capsule', 'Box'): raise ValueError('unsupported source primitive')
    low, high = transformed_bounds(child); center = (low + high) * .5
    radius = float(np.linalg.norm(high - center))
    root = ET.Element('Drawable'); node(root, 'Name', model)
    vector(root, 'BoundingSphereCenter', center); node(root, 'BoundingSphereRadius', value=number(radius))
    vector(root, 'BoundingBoxMin', low); vector(root, 'BoundingBoxMax', high)
    for level in ('High', 'Med', 'Low', 'Vlow'):
        node(root, 'LodDist' + level, value=9998); node(root, 'Flags' + level, value=0)
    # GTA's drawable store hands every placed drawable to a listener that
    # reads the shader group without a null check (Legacy3889 +0x9445c4).
    # Every owned retail YDR carries one; an empty group (no dictionary, no
    # shaders) satisfies it and the placement path (+0x141bc84) unchanged.
    node(node(root, 'ShaderGroup'), 'Shaders')
    compound = node(root, 'Bounds', type='Composite')
    outer = copy.deepcopy(child['fields'])
    outer.update(BoxMin=low, BoxMax=high, BoxCenter=center, SphereCenter=center, SphereRadius=radius)
    fields_xml(compound, outer)
    original = node(node(compound, 'Children'), 'Item', type=child['kind'])
    fields_xml(original, child['fields'])
    node(original, 'CompositeTransform', ' '.join(map(number, child['bone_relative_transform'])))
    node(original, 'CompositeFlags1', 'OBJECT')
    node(original, 'CompositeFlags2', 'PED, RAGDOLL')
    node(root, 'Lights')
    return root


def verify_child(actual, source):
    if actual is None or actual.get('type') != source['kind']: raise ValueError('primitive type changed')
    fields = source['fields']
    for key in VECTOR_FIELDS:
        values = [float(actual.find(key).get(axis)) for axis in 'xyz']
        if struct.pack('<3f', *values) != struct.pack('<3f', *fields[key]): raise ValueError('source field changed: ' + key)
    for key in FLOAT_FIELDS:
        if struct.pack('<f', float(actual.find(key).get('value'))) != struct.pack('<f', fields[key]): raise ValueError('source field changed: ' + key)
    for key in INTEGER_FIELDS:
        if int(actual.find(key).get('value')) != fields[key]: raise ValueError('source field changed: ' + key)
    values = list(map(float, actual.findtext('CompositeTransform').split()))
    if struct.pack('<16f', *values) != struct.pack('<16f', *source['bone_relative_transform']): raise ValueError('bone-relative transform changed')
    if actual.findtext('CompositeFlags1') != 'OBJECT' or actual.findtext('CompositeFlags2') != 'PED, RAGDOLL':
        raise ValueError('native collision masks changed')


def verify_shader_group(root):
    group = root.find('ShaderGroup')
    if group is None: raise ValueError('collider drawable lacks the shader group the GTA drawable store requires')
    if group.find('TextureDictionary') is not None or group.findall('./Shaders/Item'):
        raise ValueError('collider shader group must stay empty')


def export(source, out, rage, binding=None):
    out.mkdir(parents=True, exist_ok=True)
    models, checks = [], []
    for i, child in enumerate(source['children']):
        if child['child'] != i: raise ValueError('source child order changed')
        model = f'skatev_body_{i:02d}'
        root = drawable(child, model); ET.indent(root)
        xml = out / (model + '.ydr.xml'); ET.ElementTree(root).write(xml, encoding='utf-8', xml_declaration=True)
        ydr = out / (model + '.ydr'); run_rage(rage, 'resource', 'build', xml, '-o', ydr, '--strict')
        back = out / (model + '.roundtrip.xml'); run_rage(rage, 'resource', 'dump', ydr, '-o', back)
        parsed = ET.parse(back); children = parsed.findall('./Bounds/Children/Item')
        if len(children) != 1: raise ValueError('collider must have exactly one authored primitive')
        verify_child(children[0], child)
        if parsed.findall('./DrawableModelsHigh/Item'): raise ValueError('collider unexpectedly renders geometry')
        verify_shader_group(parsed.getroot())
        models.append(ydr)
        checks.append({'model': model, 'bone_tag': child['bone_tag'], 'bone_name': child['bone_name'],
                       'ydr_sha256': sha256(ydr.read_bytes()), 'original': child})
    if not models: raise ValueError('no source body bounds')
    ytyp = out / 'skatev_ped_colliders.ytyp'
    run_rage(rage, 'ytyp', 'from-drawables', *models, '-o', ytyp, '--flags', ARCHETYPE_FLAGS)
    ytyp_xml = out / 'skatev_ped_colliders.ytyp.xml'; run_rage(rage, 'resource', 'dump', ytyp, '-o', ytyp_xml)
    archetypes = ET.parse(ytyp_xml).findall('./archetypes/Item')
    if len(archetypes) != len(models): raise ValueError('archetype count mismatch')
    expected = {p.stem for p in models}
    for a in archetypes:
        name = a.findtext('name')
        if name not in expected or a.findtext('assetName') != name or a.findtext('physicsDictionary') != name:
            raise ValueError('archetype did not retain embedded physics dictionary')
        expected.remove(name)
        if a.findtext('assetType') != 'ASSET_TYPE_DRAWABLE' or int(a.find('flags').get('value')) != ARCHETYPE_FLAGS:
            raise ValueError('archetype lacks native creation flags/type')
    receipt = {'schema': 1, 'source': source['source'], 'source_sha256': source['sha256'],
               'source_binding': binding, 'collider_count': len(models), 'type_mask': TYPE_MASK, 'include_mask': INCLUDE_MASK,
               'filter_evidence': 'Legacy3889 +1514DB2..+1514DF1 dual-mask AND; OBJECT type with PED|RAGDOLL include excludes static map types',
               'world_transform': 'native object world = GTA posed bone world; bound child carries authored bone_relative_transform',
               'archetype_flags': ARCHETYPE_FLAGS, 'colliders': checks,
               'ytyp_sha256': sha256(ytyp.read_bytes()), 'rage_binary_sha256': sha256(rage.read_bytes()),
               'verification': 'HOST native roundtrip only; geometryless native creation and interaction require GTA acceptance'}
    manifest = 'SKATEV_PED_COLLIDERS\t1\n' + ''.join(f"{c['model']}\t{c['bone_tag']}\n" for c in checks)
    receipt['manifest_sha256'] = sha256(manifest.encode('utf-8'))
    (out / 'receipt.json').write_text(json.dumps(receipt, indent=2), encoding='utf-8')
    staged = out / 'manifest.tsv.partial'; staged.write_text(manifest, encoding='utf-8', newline=''); staged.replace(out / 'manifest.tsv')
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-yft', type=Path, required=True)
    parser.add_argument('--binding-receipt', type=Path, required=True,
                        help='policy JSON requiring live asset match; source_sha256 and expected_frag_name must match YFT')
    parser.add_argument('--out', type=Path, default=REPO / 'local/skate-data/assets/private/ped-colliders')
    parser.add_argument('--rage', type=Path, default=REPO / 'build/tools/rage.exe')
    args = parser.parse_args()
    verify_pins()
    source = decode_source(args.source_yft)
    binding = json.loads(args.binding_receipt.read_text())
    validate_binding(source, binding)
    receipt = export(source, args.out, args.rage, binding)
    print(json.dumps({'output': str(args.out.resolve()), 'collider_count': receipt['collider_count'],
                      'ytyp_sha256': receipt['ytyp_sha256']}, indent=2))


if __name__ == '__main__': main()

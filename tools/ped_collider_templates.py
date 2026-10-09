"""Runtime-patchable templates for posed body colliders (any ped, any model).

The verified exporter (prepare-ped-colliders.py) builds one native collider
YDR per authored ragdoll child with the locked rage CLI. Every such YDR of a
primitive kind has the same layout; only these values differ: the child's
authored bound fields, its bone-relative transform, the enclosing bounds
computed from them, and the model name. This tool turns one export of each
kind into a template plus a field map found by walking the resource's own
pointers, and proves the map complete: re-patching the template with every
exported child's values must reproduce rage's bytes exactly. The host then
builds colliders for whatever ragdoll compound a live ped carries.

    python tools/ped_collider_templates.py [colliders dir] [out dir] [rage.exe]

Outputs (local, private): capsule.sys / box.sys (decompressed system
sections), templates.json (header, field offsets) and the template YTYP.
"""
import argparse
import itertools
import json
import struct
import subprocess
import zlib
from pathlib import Path

import numpy as np

from _common import joaat

BASE = 0x50000000
# Child bound fields copied verbatim (offset, struct code) - the exporter's set.
CHILD_FIELDS = [(0x14, 'f'), (0x20, '3f'), (0x2c, 'f'), (0x30, '3f'), (0x3c, 'I'), (0x40, '3f'),
                (0x4c, 'B'), (0x4d, 'B'), (0x4e, 'B'), (0x4f, 'B'), (0x50, '3f'), (0x5c, 'B'),
                (0x5d, 'B'), (0x60, '3f'), (0x6c, 'f')]
# Composite fields the exporter copies from the child (outer = child, then
# BoxMin/Max/Center, SphereCenter, SphereRadius replaced by computed bounds).
OUTER_COPIED = [(0x2c, 'f'), (0x3c, 'I'), (0x4c, 'B'), (0x4d, 'B'), (0x4e, 'B'), (0x4f, 'B'),
                (0x5c, 'B'), (0x5d, 'B'), (0x60, '3f'), (0x6c, 'f')]


def u64(data, off):
    return struct.unpack_from('<Q', data, off)[0]


def ptr(data, off):
    v = u64(data, off)
    if not BASE <= v < BASE + len(data):
        raise ValueError(f'pointer at {off:#x} outside system section: {v:#x}')
    return v - BASE


def layout(data):
    composite = ptr(data, 0xC8)
    children = ptr(data, composite + 0x70)
    child = ptr(data, children)
    return {
        'name': ptr(data, 0xA8),
        'composite': composite,
        'child': child,
        'transforms': ptr(data, composite + 0x78),
        'boxes': ptr(data, composite + 0x88),
    }


def enclosing(child_min, child_max, matrix):
    m = np.asarray(matrix, dtype=np.float64).reshape(4, 4)
    corners = np.array([[*p, 1.] for p in itertools.product(*zip(child_min, child_max))]) @ m
    low = corners[:, :3].min(axis=0)
    high = corners[:, :3].max(axis=0)
    center = (low + high) * .5
    radius = float(np.linalg.norm(high - center))
    f32 = lambda v: np.float32(v)
    return [f32(x) for x in low], [f32(x) for x in high], [f32(x) for x in center], f32(radius)


def patch(template, lay, name, child_raw, matrix):
    """Template system section + one child's values -> system section.
    `child_raw`: the child's 112/128 authored bound bytes; `matrix`: 16 floats."""
    data = bytearray(template)
    raw = bytes(child_raw)
    def field(off, code):
        return struct.unpack_from('<' + code, raw, off)
    child_min, child_max = field(0x30, '3f'), field(0x20, '3f')
    low, high, center, radius = enclosing(child_min, child_max, matrix)
    # Drawable bounds.
    struct.pack_into('<3f', data, 0x20, *center)
    struct.pack_into('<f', data, 0x2c, radius)
    struct.pack_into('<3f', data, 0x30, *low)
    struct.pack_into('<3f', data, 0x40, *high)
    # Composite: computed bounds, other fields from the child.
    c = lay['composite']
    for off, code in OUTER_COPIED:
        struct.pack_into('<' + code, data, c + off, *field(off, code))
    struct.pack_into('<f', data, c + 0x14, radius)
    struct.pack_into('<3f', data, c + 0x20, *high)
    struct.pack_into('<3f', data, c + 0x30, *low)
    struct.pack_into('<3f', data, c + 0x40, *center)
    struct.pack_into('<3f', data, c + 0x50, *center)
    # The authored child.
    for off, code in CHILD_FIELDS:
        struct.pack_into('<' + code, data, lay['child'] + off, *field(off, code))
    # Composite transform rows (xyz; the w lanes keep the template's words).
    m = [np.float32(x) for x in matrix]
    for row in range(4):
        struct.pack_into('<3f', data, lay['transforms'] + 16 * row, *m[row * 4: row * 4 + 3])
    # Per-child box in the composite (child's own extents, margin in max.w).
    struct.pack_into('<3f', data, lay['boxes'], *child_min)
    struct.pack_into('<3f', data, lay['boxes'] + 16, *child_max)
    struct.pack_into('<f', data, lay['boxes'] + 28, *field(0x2c, 'f'))
    # Name (same length as the template's).
    old = data[lay['name']:data.index(0, lay['name'])]
    if len(name) != len(old):
        raise ValueError('collider names keep the template length')
    data[lay['name']:lay['name'] + len(name)] = name.encode()
    return bytes(data)


# Single-archetype YTYP (rage `ytyp from-drawables`): archetype bounds equal
# the drawable's; the model's name hash fills name/assetName/physicsDictionary
# and the YTYP's own name (its file stem, which the host gives the model name).
YTYP_BB_MIN, YTYP_BB_MAX, YTYP_BS_CENTRE, YTYP_BS_RADIUS = 0x230, 0x240, 0x250, 0x260
YTYP_HASHES = (0x268, 0x278, 0x280, 0x2C8)


def patch_ytyp(template, name, child_raw, matrix):
    raw = bytes(child_raw)
    child_min = struct.unpack_from('<3f', raw, 0x30)
    child_max = struct.unpack_from('<3f', raw, 0x20)
    low, high, center, radius = enclosing(child_min, child_max, matrix)
    d = bytearray(template)
    struct.pack_into('<3f', d, YTYP_BB_MIN, *low)
    struct.pack_into('<3f', d, YTYP_BB_MAX, *high)
    struct.pack_into('<3f', d, YTYP_BS_CENTRE, *center)
    struct.pack_into('<f', d, YTYP_BS_RADIUS, radius)
    for off in YTYP_HASHES:
        struct.pack_into('<I', d, off, joaat(name))
    return bytes(d)


def ytyp_of(rage, ydr, directory):
    """rage's single-archetype YTYP for `ydr`, written as <model>.ytyp."""
    out = directory / (ydr.stem + '.ytyp')
    subprocess.run([str(rage), '--no-update-check', 'ytyp', 'from-drawables', str(ydr), '-o', str(out),
                    '--flags', str(0x20020)], check=True, capture_output=True)
    raw = out.read_bytes()
    out.unlink()
    return raw[:16], zlib.decompress(raw[16:], -15)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('src', nargs='?', type=Path, default=Path('local/skate-data/assets/private/ped-colliders'))
    p.add_argument('out', nargs='?', type=Path, help='default: <colliders dir>/runtime')
    p.add_argument('rage', nargs='?', type=Path, default=Path('build/tools/rage.exe'))
    a = p.parse_args()
    src, rage = a.src, a.rage
    out = a.out or src / 'runtime'
    out.mkdir(parents=True, exist_ok=True)
    receipt = json.loads((src / 'receipt.json').read_text())
    templates, checked = {}, 0
    for c in receipt['colliders']:
        o = c['original']
        raw = (src / (c['model'] + '.ydr')).read_bytes()
        header, data = raw[:16], zlib.decompress(raw[16:], -15)
        kind = o['kind'].lower()
        if kind not in templates:
            templates[kind] = (header, data, layout(data), c['model'])
        t_header, t_data, lay, _ = templates[kind]
        if header != t_header or len(data) != len(t_data) or layout(data) != lay:
            raise ValueError(f"{c['model']}: layout differs from the {kind} template")
        rebuilt = patch(t_data, lay, c['model'], bytes.fromhex(o['bound_raw']), o['bone_relative_transform'])
        if rebuilt != data:
            diff = [hex(i) for i in range(len(data)) if rebuilt[i] != data[i]]
            raise ValueError(f"{c['model']}: patched template differs at {diff[:12]}")
        # Expected outputs for the host generator test.
        (out / 'expected').mkdir(exist_ok=True)
        (out / 'expected' / (c['model'] + '.ydr.sys')).write_bytes(data)
        checked += 1
    # YTYP template from the first export; every export's own single YTYP must
    # be reproduced by patching it.
    first = receipt['colliders'][0]['model']
    ytyp_header, ytyp_template = ytyp_of(rage, src / (first + '.ydr'), out)
    for c in receipt['colliders']:
        o = c['original']
        header, want = ytyp_of(rage, src / (c['model'] + '.ydr'), out)
        got = patch_ytyp(ytyp_template, c['model'], bytes.fromhex(o['bound_raw']), o['bone_relative_transform'])
        if header != ytyp_header or got != want:
            raise ValueError(f"{c['model']}: YTYP template patch differs")
        (out / 'expected' / (c['model'] + '.ytyp.sys')).write_bytes(want)
    (out / 'ytyp.sys').write_bytes(ytyp_template)
    manifest = {'schema': 1, 'checked_exports': checked, 'kinds': {}}
    for kind, (header, data, lay, model) in templates.items():
        (out / f'{kind}.sys').write_bytes(data)
        manifest['kinds'][kind] = {'header_hex': header.hex(), 'system_size': len(data), 'layout': lay,
                                   'template_model': model, 'primitive_type': 1 if kind == 'capsule' else 3,
                                   'bound_bytes': 128 if kind == 'capsule' else 112}
    manifest['child_fields'] = CHILD_FIELDS
    manifest['outer_copied'] = OUTER_COPIED
    manifest['ytyp'] = {'header_hex': ytyp_header.hex(), 'system_size': len(ytyp_template),
                        'bb_min': YTYP_BB_MIN, 'bb_max': YTYP_BB_MAX, 'bs_centre': YTYP_BS_CENTRE,
                        'bs_radius': YTYP_BS_RADIUS, 'hashes': list(YTYP_HASHES)}
    manifest['source_yft'] = receipt['source']
    (out / 'templates.json').write_text(json.dumps(manifest, indent=2))
    # Plain index for the host: one line per kind plus the YTYP header.
    tab = '\t'
    lines = [tab.join(['SKATEV_COLLIDER_TEMPLATES', '1'])]
    for kind, k in manifest['kinds'].items():
        l = k['layout']
        lines.append(tab.join(map(str, [kind, k['header_hex'], l['name'], l['composite'], l['child'],
                                         l['transforms'], l['boxes']])))
    lines.append(tab.join(['ytyp', ytyp_header.hex()]))
    (out / 'templates.txt').write_text('\n'.join(lines) + '\n', encoding='utf-8', newline='')
    # The decompressed owned source fragment: the host test reads the ragdoll
    # compound from it exactly as it reads a live ped's resident fragment type.
    raw = Path(receipt['source']).read_bytes()
    (out / 'expected' / 'source.sys').write_bytes(zlib.decompress(raw[16:], -15))
    print(f'{checked} exports reproduced byte-for-byte from {len(templates)} templates -> {out}')


if __name__ == '__main__':
    main()

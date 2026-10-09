"""Weapon-wheel icon for WEAPON_SKATEBOARD (tools/build-board-weapon.py).

The wheel shows a weapon by `gotoAndStop("INT" + hash)` on the slot sprite
(SLOT_WEAPONS_4 for melee) and on MASTER_WEAPONS (in a vehicle); every stock
icon is a frame labelled INT<hash> that places one shape. This adds that frame
for the board, editing the player's own hud.gfx at the tag level:

- a DefineBitsLossless2 bitmap of the owner's art (tools/board-wheel-icon.bin,
  premultiplied ARGB, made from tools/board-wheel-icon.png by --make-asset) and
  a DefineShape filled with it, centred like the stock icons;
- one frame per target sprite: RemoveObject2 depth 1, FrameLabel, PlaceObject2,
  ShowFrame;
- the ammo counter and weapon icon scripts treat the board as melee (no "0"):
  both switch on WeaponsLUT constants, so the value of one Online-only melee
  constant, WEAPON_CANDY_CANE, becomes the board's hash. That is a 4-byte change
  in WeaponsLUT's int push; no script code moves or is recompiled.

Standard library only (setup runs it). Input: hud.gfx from the user's
update.rpf (extracted when missing). Output: local/live-clip/hud.gfx, installed
beside the skatev dlc pack (host/src/pack_loader.h).
"""
import argparse
import struct
import subprocess
import zlib
from pathlib import Path

from _common import REPO as ROOT, joaat

WORK = ROOT / 'local/hud'
SOURCE = WORK / 'x64/data/cdimages/scaleform_generic.rpf/hud.gfx'
ART = ROOT / 'tools/board-wheel-icon.png'
ASSET = ROOT / 'tools/board-wheel-icon.bin'
OUT = ROOT / 'local/live-clip/hud.gfx'
RAGE = ROOT / 'build/tools/rage.exe'

HASH = joaat('WEAPON_SKATEBOARD')
WIDTH_TWIPS = 2400          # WEAPON_BAT's icon is 2480 x 287
TWIPS_PER_PIXEL = 5         # stock atlas icons use 20; 4x their resolution
SPRITES = ('SLOT_WEAPONS_4', 'MASTER_WEAPONS')
LUT = '__Packages.com.rockstargames.gtav.constants.WeaponsLUT'
DONOR = 'WEAPON_CANDY_CANE'  # Online-only melee constant in both scripts' melee cases

DO_INIT_ACTION, EXPORT_ASSETS, DEFINE_SPRITE, END = 59, 56, 39, 0
DEFINE_SHAPE, DEFINE_BITS_LOSSLESS2 = 2, 36
SHOW_FRAME, PLACE_OBJECT2, REMOVE_OBJECT2, FRAME_LABEL = 1, 26, 28, 43


# --- SWF tags --------------------------------------------------------------

def tag(code, body, long=False):
    if len(body) < 0x3F and not long:
        return struct.pack('<H', code << 6 | len(body)) + body
    return struct.pack('<HI', code << 6 | 0x3F, len(body)) + body


def parse(data, p, end):
    """(code, header start, body start, body length) of each tag, End included."""
    out = []
    while p < end:
        cl = struct.unpack_from('<H', data, p)[0]
        code, ln, body = cl >> 6, cl & 0x3F, p + 2
        if ln == 0x3F:
            ln = struct.unpack_from('<I', data, body)[0]
            body += 4
        out.append((code, p, body, ln))
        p = body + ln
        if code == END:
            break
    return out


def header_end(data):
    nbits = data[8] >> 3
    return 8 + (5 + 4 * nbits + 7) // 8 + 4  # signature/version/length, frame RECT, rate, count


def exports(data, tags):
    names = {}
    for code, _, body, _ in tags:
        if code == EXPORT_ASSETS:
            q = body + 2
            for _ in range(struct.unpack_from('<H', data, body)[0]):
                cid = struct.unpack_from('<H', data, q)[0]
                e = data.index(b'\0', q + 2)
                names[data[q + 2:e].decode('latin1')] = cid
                q = e + 1
    return names


class Bits:
    def __init__(self):
        self.v, self.n = 0, 0

    def put(self, value, n):
        self.v = self.v << n | (value & ((1 << n) - 1))
        self.n += n
        return self

    def bytes(self):
        pad = -self.n % 8
        return (self.v << pad).to_bytes((self.n + pad) // 8, 'big')


def nbits(*values):
    """Signed bit count SWF needs for these values."""
    return max(abs(int(v)).bit_length() for v in values) + 1


def rect(w, h):
    n = nbits(w, h)
    return Bits().put(n, 5).put(0, n).put(w, n).put(0, n).put(h, n).bytes()


def matrix(scale=None, tx=0, ty=0):
    b = Bits().put(scale is not None, 1)
    if scale is not None:
        n = nbits(scale)
        b.put(n, 5).put(scale, n).put(scale, n)
    b.put(0, 1)  # no rotate
    n = nbits(tx, ty) if tx or ty else 0
    b.put(n, 5)
    if n:
        b.put(tx, n).put(ty, n)
    return b.bytes()


def icon_tags(w, h, argb_z, bitmap_id, shape_id):
    bitmap = tag(DEFINE_BITS_LOSSLESS2, struct.pack('<HBHH', bitmap_id, 5, w, h) + argb_z, long=True)
    W, H = w * TWIPS_PER_PIXEL, h * TWIPS_PER_PIXEL
    fill = struct.pack('<BBH', 1, 0x41, bitmap_id) + matrix(scale=TWIPS_PER_PIXEL << 16)  # clipped bitmap
    move, nw, nh = nbits(W, H), nbits(W), nbits(H)
    edges = Bits().put(0, 1).put(0b00101, 5).put(move, 5).put(W, move).put(H, move).put(1, 1)  # move to (W, H), fill 1
    for dx, dy in ((-W, 0), (0, -H), (W, 0), (0, H)):
        n = nw if dx else nh
        edges.put(1, 1).put(1, 1).put(n - 2, 4).put(0, 1).put(dy != 0, 1).put(dx or dy, n)
    edges.put(0, 6)  # end of shape
    shape = struct.pack('<H', shape_id) + rect(W, H) + fill + b'\0' + b'\x10' + edges.bytes()
    return bitmap + tag(DEFINE_SHAPE, shape, long=True), (W, H)


def icon_frame(shape_id, ratio, size):
    x, y = -size[0] // 2, -size[1] // 2
    place = struct.pack('<BHH', 0x16, 1, shape_id) + matrix(tx=x, ty=y) + struct.pack('<H', ratio)
    return (tag(REMOVE_OBJECT2, struct.pack('<H', 1)) + tag(FRAME_LABEL, f'INT{HASH}'.encode() + b'\0', long=True)
            + tag(PLACE_OBJECT2, place) + tag(SHOW_FRAME, b''))


# --- the edit ----------------------------------------------------------------

def lut_patch(data, tags, names):
    """Offset of WEAPON_CANDY_CANE's int value in WeaponsLUT's push."""
    sid = names[LUT]
    _, _, body, ln = next(t for t in tags if t[0] == DO_INIT_ACTION and struct.unpack_from('<H', data, t[2])[0] == sid)
    p, end, pool, hits = body + 2, body + ln, [], []
    while p < end:
        code = data[p]
        if code < 0x80:
            p += 1
            continue
        n = struct.unpack_from('<H', data, p + 1)[0]
        rec = data[p + 3:p + 3 + n]
        if code == 0x88:  # ConstantPool
            pool = rec[2:].split(b'\0')[:struct.unpack_from('<H', rec)[0]]
        elif code == 0x96 and pool:  # Push: register 1, constant DONOR, int value
            q, values = 0, []
            while q < n:
                t = rec[q]
                size = {0: rec.find(b'\0', q + 1) - q, 1: 4, 2: 0, 3: 0, 4: 1, 5: 1, 6: 8, 7: 4, 8: 1, 9: 2}[t]
                values.append((t, rec[q + 1:q + 1 + size], p + 4 + q))
                q += 1 + size
            for (t1, v1, _), (t2, _, at) in zip(values, values[1:]):
                if t1 in (8, 9) and t2 == 7 and pool[int.from_bytes(v1, 'little')] == DONOR.encode():
                    hits.append(at)
        p += 3 + n
    if len(hits) != 1:
        raise SystemExit(f'WeaponsLUT: expected one int definition of {DONOR}, found {len(hits)}')
    return hits[0]


def build(data, asset):
    if data[:3] != b'GFX':
        raise SystemExit('hud.gfx: expected an uncompressed GFX file')
    w, h = struct.unpack_from('<HH', asset, 4)
    argb_z = asset[8:]
    start = header_end(data)
    tags = parse(data, start, len(data))
    names = exports(data, tags)
    if f'INT{HASH}'.encode() in data:
        raise SystemExit('hud.gfx already has the board frame')
    ids = [struct.unpack_from('<H', data, body)[0] for code, _, body, _ in tags if code in (2, 22, 32, 83, 36, 20, 39, 6, 21, 35, 37, 46, 84)]
    bitmap_id = max(i for i in ids if i < 65534) + 1
    defs, size = icon_tags(w, h, argb_z, bitmap_id, bitmap_id + 1)
    targets = {names[n] for n in SPRITES}
    value_at = lut_patch(data, tags, names)
    out, inserted = bytearray(data[:start]), False
    for code, hp, body, ln in tags:
        raw = data[hp:body + ln]
        if code == DEFINE_SPRITE and struct.unpack_from('<H', data, body)[0] in targets:
            if not inserted:
                out += defs
                inserted = True
            sid, frames = struct.unpack_from('<HH', data, body)
            sub = parse(data, body + 4, body + ln)
            if sub[-1][0] != END:
                raise SystemExit(f'sprite {sid}: no End tag')
            subtags = data[body + 4:sub[-1][1]] + icon_frame(bitmap_id + 1, frames, size) + data[sub[-1][1]:body + ln]
            raw = tag(DEFINE_SPRITE, struct.pack('<HH', sid, frames + 1) + subtags, long=True)
        elif hp <= value_at < body + ln:
            raw = bytearray(raw)
            struct.pack_into('<I', raw, value_at - hp, HASH)
        out += raw
    if not inserted:
        raise SystemExit('target sprites not found')
    struct.pack_into('<I', out, 4, len(out))
    return bytes(out), (w, h), size


def make_asset():
    """Dev step: cut the owner's art out of its flat ground and store it as the
    premultiplied ARGB bitmap the tag wants (needs numpy, scipy, Pillow)."""
    import numpy as np
    from PIL import Image
    from scipy import ndimage

    a = np.asarray(Image.open(ART).convert('RGB')).astype(float)
    border = np.concatenate([a[0], a[-1], a[:, 0], a[:, -1]])
    ground = np.median(border, 0)
    dist = np.linalg.norm(a - ground, axis=2)
    labels, _ = ndimage.label(dist < 25)
    edge = np.unique(np.concatenate([labels[0], labels[-1], labels[:, 0], labels[:, -1]]))
    outside = np.isin(labels, edge[edge > 0])
    near = ndimage.binary_dilation(outside, iterations=2) & ~outside
    alpha = np.where(outside, 0.0, 1.0)
    alpha[near] = np.clip(dist[near] / np.linalg.norm(ground), 0, 1)  # anti-aliased against the outline
    colour = np.clip((a - (1 - alpha[..., None]) * ground) / np.maximum(alpha, 1e-3)[..., None], 0, 255)
    rgba = np.dstack([colour, alpha * 255]).round().astype(np.uint8)
    ys, xs = np.nonzero(alpha > 0)
    image = Image.fromarray(rgba[ys.min():ys.max() + 1, xs.min():xs.max() + 1], 'RGBA')
    image = image.resize((WIDTH_TWIPS // TWIPS_PER_PIXEL,
                          round(image.height * WIDTH_TWIPS / TWIPS_PER_PIXEL / image.width)), Image.LANCZOS)
    px = np.asarray(image.convert('RGBa'))  # premultiplied, as format 5 wants
    ASSET.write_bytes(b'SVIC' + struct.pack('<HH', *image.size) + zlib.compress(px[..., [3, 0, 1, 2]].tobytes(), 9))
    print(f'{ASSET}: {image.width}x{image.height} px')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--game-root', type=Path, help='GTA V Legacy folder (hud.gfx source)')
    p.add_argument('--source', type=Path, default=SOURCE, help='hud.gfx to edit')
    p.add_argument('--out', type=Path, default=OUT)
    p.add_argument('--make-asset', action='store_true', help='dev: rebuild tools/board-wheel-icon.bin from the PNG')
    args = p.parse_args()
    if args.make_asset:
        return make_asset()
    if not args.source.exists():
        if not args.game_root:
            raise SystemExit(f'{args.source} missing: pass --game-root to extract it')
        subprocess.run([RAGE, '--no-update-check', '--exe', args.game_root, 'extract', '-r',
                        args.game_root / 'update/update.rpf', '*scaleform_generic.rpf/hud.gfx', '-o', WORK], check=True)
    data, px, size = build(args.source.read_bytes(), ASSET.read_bytes())
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_bytes(data)
    print(f'{args.out}: INT{HASH} frame, {px[0]}x{px[1]} px, {size[0]}x{size[1]} twips; {DONOR} -> board (no ammo text)')


if __name__ == '__main__':
    main()

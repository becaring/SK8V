"""Reads the files setup needs straight out of a Skate 3 Xbox 360 disc image
(XDVDFS), so players can point setup at their ISO instead of extracting it.

    python tools/xiso.py <Skate 3.iso> <out dir>            # the files SK8V needs
    python tools/xiso.py <Skate 3.iso> --list               # every file on the disc

XDVDFS: the game partition starts at a fixed offset that depends on the disc
format (0 for a trimmed image, XGD2 0xFD90000, XGD3 0x2080000, XGD1
0x18300000). Its volume descriptor is at sector 32: "MICROSOFT*XBOX*MEDIA",
then the root directory's sector and size. A directory is a binary tree of
entries: left and right subtree offsets (in 4-byte units, 0 = none), the
entry's start sector and size, attributes (0x10 = directory), name length and
name. Sectors are 2048 bytes from the partition start. Standard library only.
"""
import argparse
import struct
from pathlib import Path

SECTOR = 2048
MAGIC = b'MICROSOFT*XBOX*MEDIA'
PARTITIONS = (0, 0xFD90000, 0x2080000, 0x18300000)
DIRECTORY = 0x10
# What setup reads from the disc (convert-skate-data, prepare-hom-*, audio and tuning).
NEEDED = [
    'default.xex',
    'data/big/db.big', 'data/big/fedata.big', 'data/big/fedynamic.big', 'data/big/fetexture.big',
    'data/big/miscboot.big', 'data/big/miscload.big',
    'data/content/createacharacter.big', 'data/content/marquee.big',
    'data/audio/audiofiles.big', 'data/audio/grains.big', 'data/audio/wheels.big', 'data/audio/MixMapSK8.mxb',
]


class Disc:
    def __init__(self, path):
        self.f = open(path, 'rb')
        for base in PARTITIONS:
            self.f.seek(base + 32 * SECTOR)
            head = self.f.read(28)
            if head[:20] == MAGIC:
                self.base = base
                self.root = struct.unpack_from('<II', head, 20)
                return
        self.f.close()
        raise ValueError(f'{path}: not an Xbox 360 disc image (no XDVDFS volume found)')

    def close(self):
        self.f.close()

    def read(self, sector, size):
        self.f.seek(self.base + sector * SECTOR)
        return self.f.read(size)

    def entries(self, sector, size):
        """{name: (sector, size, is_dir)} of one directory."""
        table, out, todo = self.read(sector, size), {}, [0] if size else []
        while todo:
            at = todo.pop()
            if at + 14 > len(table):
                continue
            left, right, start, length, attr, n = struct.unpack_from('<HHIIBB', table, at)
            if left == 0xFFFF:  # padding to the end of the sector
                continue
            out[table[at + 14:at + 14 + n].decode('latin1')] = (start, length, bool(attr & DIRECTORY))
            todo += [o * 4 for o in (left, right) if o]
        return out

    def walk(self, prefix='', where=None):
        for name, (start, length, is_dir) in sorted(self.entries(*(where or self.root)).items()):
            path = f'{prefix}{name}'
            if is_dir:
                yield from self.walk(path + '/', (start, length))
            else:
                yield path, start, length

    def find(self, path):
        """(sector, size) of a file, matching names case-insensitively like the console."""
        where = self.root
        parts = path.split('/')
        for i, part in enumerate(parts):
            names = {k.lower(): v for k, v in self.entries(*where).items()}
            hit = names.get(part.lower())
            if not hit or hit[2] != (i < len(parts) - 1):
                raise FileNotFoundError(f'{path} not on the disc')
            where = hit[:2]
        return where

    def extract(self, path, out):
        sector, size = self.find(path)
        out.parent.mkdir(parents=True, exist_ok=True)
        self.f.seek(self.base + sector * SECTOR)
        with open(out, 'wb') as w:
            while size:
                chunk = self.f.read(min(size, 1 << 22))
                if not chunk:
                    raise EOFError(f'{path}: the image ends inside the file (truncated ISO?)')
                w.write(chunk)
                size -= len(chunk)


def extract_needed(iso, out, log=print):
    disc = Disc(iso)
    try:
        for path in NEEDED:
            disc.extract(path, Path(out) / path)
            log(f'{path}')
    finally:
        disc.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('iso', type=Path)
    ap.add_argument('out', type=Path, nargs='?')
    ap.add_argument('--list', action='store_true')
    a = ap.parse_args()
    if a.list:
        disc = Disc(a.iso)
        for path, _, size in disc.walk():
            print(f'{size:>12} {path}')
        return
    extract_needed(a.iso, a.out)


if __name__ == '__main__':
    main()

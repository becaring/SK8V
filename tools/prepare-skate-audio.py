"""Prepare SkateV's Skate 3 audio cache from the user's own Skate 3 files.

    python tools/prepare-skate-audio.py --skate-data <Skate_3 dir> --out <cache dir>

<Skate_3 dir> is the user's extracted Xbox 360 game folder (the one holding
data/audio/audiofiles.big). The cache it writes is what SkateVRuntime.dll
plays from; point SkateVLegacy.ini `AudioCache=` at it. Nothing in it may be
redistributed: it is the user's own game audio.

Steps (docs/AEMS.md "Sample cache"):
1. Unpack the EA "EB" v3 archives the gameplay audio lives in
   (audiofiles.big: .csi symbol files, .abk AEMS banks, .bnk splice banks;
   grains.big: rolling recordings; wheels.big: wheel-spin streams) into
   <out>/raw/<archive>/, byte for byte.
2. Decode every EA-XMA sample of every .abk bank and every .bnk splice bank
   (board pops, landings, impacts; Skate's second sound path) once, continuously, with
   skate-xma (SkateV's own decoder: a port of the FFmpeg 4.4 XMA path the
   Skate 3 recomp oracle uses; LGPL-2.1-or-later, shipped as skate-xma.exe,
   source rust/skate-xma) into <out>/pcm/<archive>/<bank>_<index>.xma16
   (big-endian PCM16, whole 512-sample frames, named by the table index the
   game uses). This equals the reference `xmadec --continuous` decode bit for
   bit (see docs/AEMS.md for the measurement).
3. Write <out>/manifest.json (format marker, version, input/decoder/output
   SHA-256 hashes and counts). Reuse only when all these contents still match.

No network access, no developer-local paths: the decoder is found next to
this script (tools/bin/skate-xma.exe), next to SkateVRuntime.dll in an
install, or given with --decoder. Requires Python 3.11 or newer.
"""
from pathlib import Path
import argparse
import hashlib
import json
import struct
import subprocess
import sys
import time

FORMAT = 'skatev-audio-cache'
VERSION = 2  # 2: splice (.bnk) bank samples are decoded too
ARCHIVES = ('audiofiles.big', 'grains.big', 'wheels.big')
MIX_MAP = 'MixMapSK8.mxb'
HERE = Path(__file__).resolve().parent


def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def cache_current(old, inputs, decoder_hash, root):
    """A size match does not prove either input or decoded output identity."""
    if not isinstance(old, dict) or not isinstance(old.get('outputs'), dict):
        return False
    if not (old.get('format') == FORMAT and old.get('version') == VERSION
            and old.get('inputs') == inputs and old.get('decoder_sha256') == decoder_hash
            and old.get('complete') and old.get('outputs')):
        return False
    base = root.resolve()
    for name, expected in old['outputs'].items():
        path = (base / name).resolve()
        if not path.is_relative_to(base) or not path.is_file() or digest(path) != expected:
            return False
    return True


def read_eb(path: Path):
    """EA "EB" v3 BIG archive -> [(name, bytes)].

    Header (big endian): 'EB' 0x0003, file count, flags (bits 8..15 = offset
    shift), name table offset and size, name entry size (top byte of the next
    word), 0, total size. Entries at 0x30: u32 offset >> shift, u32 0, u32 size,
    u32 name hash. Name entries: u16, then the zero-padded name."""
    data = path.read_bytes()
    magic, count, flags, names_off, _names_size, name_info, _zero, _total = struct.unpack_from('>8I', data, 0)
    if magic >> 16 != 0x4542 or magic & 0xFFFF != 3:
        raise ValueError(f'{path.name}: not an EB v3 archive ({magic:#x})')
    shift = (flags >> 8) & 0xFF
    entry = name_info >> 24
    files = []
    for i in range(count):
        off, _, size, _h = struct.unpack_from('>4I', data, 0x30 + 16 * i)
        n = data[names_off + entry * i + 2: names_off + entry * (i + 1)]
        name = n.split(b'\0', 1)[0].decode('ascii', 'replace')
        start = off << shift
        if start + size > len(data):
            raise ValueError(f'{path.name}: entry {name} out of range')
        files.append((name, data[start:start + size]))
    return files


def job(name, off, h1, h2):
    """One decoder job line (NAME DATA_OFFSET SAMPLES CHANNELS RATE) from an SNR header."""
    looped = (h2 >> 29) & 1
    return f'{name} {off + (12 if looped else 8)} {h2 & 0x1FFFFFFF} {((h1 >> 18) & 0x3F) + 1} {h1 & 0x3FFFF}\n'


def jobs_for(abk: bytes, stem: str):
    """One decoder job per bank sample: NAME DATA_OFFSET SAMPLES CHANNELS RATE."""
    table = struct.unpack_from('>I', abk, 32)[0]
    out = []
    i = 0
    while table and table + 4 * (3 + i) + 4 <= len(abk):
        off = table + struct.unpack_from('>I', abk, table + 4 * (3 + i))[0]
        if off + 12 > len(abk):
            break
        h1, h2 = struct.unpack_from('>2I', abk, off)
        if (h1 >> 24) & 0xF != 3 or h1 >> 28 != 0:
            break
        out.append(job(f'{stem}_{i}', off, h1, h2))
        i += 1
    return out


def jobs_for_bnk(bnk: bytes, stem: str):
    """Splice bank (SPLC, skate_aems::splice) samples, by sample-table index:
    the table follows the sound data at align4(0x3C + size); each entry's
    first word is the SNR header offset from the sample data base."""
    if bnk[:4] != b'SPLC' or len(bnk) < 0x3C:
        return []
    size, _sounds, _groups, _extra, count = struct.unpack_from('>5I', bnk, 8)
    table = (0x3C + size + 3) & ~3
    data = (table + 12 * count + 3) & ~3
    out = []
    for i in range(count):
        if table + 12 * i + 12 > len(bnk):
            break
        off = data + struct.unpack_from('>I', bnk, table + 12 * i)[0]
        if off + 12 > len(bnk):
            break
        h1, h2 = struct.unpack_from('>2I', bnk, off)
        if (h1 >> 24) & 0xF != 3 or h1 >> 28 != 0:
            continue
        out.append(job(f'{stem}_{i}', off, h1, h2))
    return out


def decode_banks(banks, jobs_fn, decoder, out_dir, job_file, failed):
    """Decode every bank's sample jobs into out_dir; returns samples decoded."""
    samples = 0
    out_dir.mkdir(parents=True, exist_ok=True)
    for k, bank in enumerate(banks):
        jobs = jobs_fn(bank.read_bytes(), bank.stem)
        jobs = [j for j in jobs if int(j.split()[3]) <= 2]  # 4-channel (two-stream) samples are not played
        if not jobs:
            continue
        job_file.write_text(''.join(jobs))
        r = subprocess.run([str(decoder), '--continuous', str(bank), str(job_file), str(out_dir)], capture_output=True, text=True)
        if r.returncode != 0:
            failed.append(f'{bank.name}: decoder exit {r.returncode}: {r.stderr.strip()[-200:]}')
            continue
        for j in jobs:
            n = j.split()[0]
            if (out_dir / f'{n}.xma16').is_file():
                samples += 1
            else:
                failed.append(f'{n}: not decoded')
        if (k + 1) % 50 == 0:
            print(f'  {k + 1}/{len(banks)} banks, {samples} samples')
    job_file.unlink(missing_ok=True)
    return samples


def find_decoder(given):
    cands = [Path(given)] if given else []
    cands += [HERE / 'bin' / 'skate-xma.exe', HERE / 'skate-xma.exe', HERE.parent / 'skate-xma.exe']
    repo = HERE.parent / 'rust'
    for t in ('target', 'target-audio', 'target-audio-xma'):
        cands.append(repo / t / 'release' / 'skate-xma.exe')
    for c in cands:
        if c.is_file():
            return c
    return None


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--skate-data', type=Path, required=True, help='the Skate_3 folder (contains data/audio)')
    p.add_argument('--out', type=Path, required=True, help='cache directory to write')
    p.add_argument('--decoder', help='skate-xma.exe (default: found next to this script)')
    p.add_argument('--force', action='store_true', help='rebuild even if the cache is current')
    a = p.parse_args()

    audio = a.skate_data / 'data' / 'audio'
    if not (audio / 'audiofiles.big').is_file():
        audio = a.skate_data / 'audio' if (a.skate_data / 'audio' / 'audiofiles.big').is_file() else audio
    if not (audio / 'audiofiles.big').is_file():
        print(f'error: {a.skate_data} has no data/audio/audiofiles.big (point --skate-data at the Skate_3 folder)')
        return 2
    decoder = find_decoder(a.decoder)
    if decoder is None:
        print('error: skate-xma.exe not found (tools/bin/skate-xma.exe, or build it: cargo build --release -p skate-xma)')
        return 2
    version = subprocess.run([str(decoder), '--version'], capture_output=True, text=True).stdout.strip() or 'skate-xma'
    decoder_hash = digest(decoder)

    inputs = {}
    for name in (*ARCHIVES, MIX_MAP):
        f = audio / name
        if not f.is_file():
            print(f'error: {f} missing')
            return 2
        st = f.stat()
        inputs[name] = {'bytes': st.st_size, 'sha256': digest(f)}
    manifest_path = a.out / 'manifest.json'
    if manifest_path.is_file() and not a.force:
        try:
            old = json.loads(manifest_path.read_text())
            if cache_current(old, inputs, decoder_hash, a.out):
                print(f'{a.out}: cache is current ({old.get("samples")} samples); --force rebuilds')
                return 0
        except (ValueError, OSError):
            pass

    t0 = time.time()
    raw = a.out / 'raw'
    pcm = a.out / 'pcm'
    a.out.mkdir(parents=True, exist_ok=True)
    digests = {}
    counts = {}
    for name in ARCHIVES:
        files = read_eb(audio / name)
        d = raw / Path(name).stem
        d.mkdir(parents=True, exist_ok=True)
        h = hashlib.sha256()
        for fname, blob in files:
            dest = (d / fname).resolve()
            if not dest.is_relative_to(d.resolve()):
                raise ValueError(f'{name}: unsafe archive member {fname}')
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(blob)
            h.update(fname.encode() + b'\0' + blob)
        digests[name] = h.hexdigest()
        counts[name] = len(files)
        print(f'{name}: {len(files)} files unpacked')

    failed = []
    out_dir = pcm / 'audiofiles'
    job_file = a.out / 'jobs.txt'
    samples = decode_banks(sorted((raw / 'audiofiles').glob('*.abk')), jobs_for, decoder, out_dir, job_file, failed)
    samples += decode_banks(sorted((raw / 'audiofiles').glob('*.bnk')), jobs_for_bnk, decoder, out_dir, job_file, failed)
    # Standalone EA SNR streams: rolling grain recordings (.grain: the SNR
    # stream starts at the u32 BE offset at byte 0) and wheel-spin streams.
    streams = 0
    for arch, pattern in (('grains', '*.grain'), ('wheels', '*.snr')):
        out = pcm / arch
        out.mkdir(parents=True, exist_ok=True)
        for f in sorted((raw / arch).glob(pattern)):
            offset = struct.unpack_from('>I', f.read_bytes(), 0)[0] if f.suffix == '.grain' else 0
            r = subprocess.run([str(decoder), '--snr', str(f), str(offset), str(out / f'{f.stem}.xma16')],
                               capture_output=True, text=True)
            if r.returncode != 0:
                failed.append(f'{arch}/{f.name}: decoder exit {r.returncode}: {r.stderr.strip()[-200:]}')
            else:
                streams += 1
    print(f'{streams} streams decoded')
    print(f'{samples} bank samples decoded in {time.time() - t0:.0f}s; {len(failed)} problems')
    for f in failed[:20]:
        print('  ', f)

    outputs = {f.relative_to(a.out).as_posix(): digest(f)
               for tree in (raw, pcm) for f in sorted(tree.rglob('*')) if f.is_file()}
    (a.out / MIX_MAP).write_bytes((audio / MIX_MAP).read_bytes())
    outputs[MIX_MAP] = digest(a.out / MIX_MAP)
    if outputs[MIX_MAP] != inputs[MIX_MAP]['sha256']:
        raise ValueError('mix map changed during cache preparation')

    manifest = {
        'format': FORMAT,
        'version': VERSION,
        'decoder': version,
        'decoder_sha256': decoder_hash,
        'inputs': inputs,
        'archives': counts,
        'archive_member_sha256': digests,
        'outputs': outputs,
        'samples': samples,
        'streams': streams,
        'problems': failed,
        'complete': not failed,
        'note': "User's own Skate 3 audio, decoded locally. Do not redistribute.",
    }
    manifest_path.write_text(json.dumps(manifest, indent=1))
    print(f'wrote {manifest_path}')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())

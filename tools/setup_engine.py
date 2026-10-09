"""Install engine for the GTA folder: preflight, a transactional install with
rollback, an ownership manifest and the settings merge. There is no
uninstaller (owner): removing SK8V is deleting its loose files (README).
Used by the release setup. Standard library only.

    python tools/setup_engine.py install --gta <GTA folder> --plan plan.json
    python tools/setup_engine.py preflight --gta <GTA folder>

plan.json: {"files": {"<path in GTA folder>": "<source file>"},
            "ini": {"<key>": "<value>"},          # template placeholders
            "keep": ["<key>", ...],               # also taken from the old INI
            "allow_existing": ["<path>", ...],    # may replace a file we did not install
            "require_scripthook": true}

Ownership: the manifest (SkateVLegacy.installed.json) records the SHA-256 of
every file written. Install refuses to replace a file it does not own unless
the plan allows it, and removes a file a newer version dropped only while it
still matches. Records (%LOCALAPPDATA%\\SkateV\\records.json) are never touched.
"""
import argparse
import ctypes
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

REQUIRED_GTA = '1.0.3889.0'
MANIFEST = 'SkateVLegacy.installed.json'
LEGACY_MANIFEST = 'SkateVLegacy.installed.txt'
INI = 'SkateVLegacy.ini'
TEMPLATE = Path(__file__).with_name('SkateVLegacy.ini.template')
NEW, OLD = '.sk8v-new', '.sk8v-old'
DATA = 'SK8V'  # the prepared data (tools/prepare.py), a folder in the GTA folder
# INI keys the in-game menu saves: a reinstall keeps the player's value.
MENU_KEYS = [
    'LipRule', 'AirTimeLimit', 'BailTimeLimit', 'SkitchStandoff', 'GtaDelegation', 'SwimDepth', 'PutAwayHoldMs',
    'BoardAim', 'GunIK', 'GunProbe', 'AimTwist', 'CameraFovScale', 'HallOfMeat', 'HallOfMeatMetrics',
    'HallOfMeatXray', 'BailPain', 'GetUpSpeech', 'NearMissSpeech', 'DynamicWorld', 'DynamicRadius', 'PedHitboxes',
    'PedLaunch', 'PedLaunchScale', 'PedLaunchLift', 'PedLaunchSpin', 'PedGetUpSpeech', 'VehicleDamage',
    'VehicleDamageScale', 'BackwardsMan', 'BackwardsManDirection', 'BackwardsManRemountDelay', 'BackwardsManChord',
    'AudioMasterGain', 'AudioOutput', 'MenuButton', 'GunButton', 'AimButton', 'FireButton', 'BoardActionInMissions',
    'Hud', 'ShowDebug', 'VerboseLog', 'PedZOffset', 'BailHitSpeed', 'BailScreamSpeed', 'PedPain', 'VehicleDamageMin',
    'VehicleDamageRadius', 'BackgroundPrepare', 'Runtime', 'StallSampler', 'BoardNative', 'DrawBoard', 'PosePed',
    'AudioGtaPlacement', 'AudioEditorTone', 'AudioGtaGainDb']


class SetupError(Exception):
    pass


def sha256(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def file_version(exe):
    """FileVersion of a Windows executable as 'a.b.c.d', or None."""
    ver = ctypes.windll.version
    size = ver.GetFileVersionInfoSizeW(str(exe), None)
    if not size:
        return None
    buf = ctypes.create_string_buffer(size)
    if not ver.GetFileVersionInfoW(str(exe), 0, size, buf):
        return None
    ptr, n = ctypes.c_void_p(), ctypes.c_uint()
    if not ver.VerQueryValueW(buf, '\\', ctypes.byref(ptr), ctypes.byref(n)):
        return None
    ms, ls = ctypes.cast(ptr, ctypes.POINTER(ctypes.c_uint32))[2:4]  # VS_FIXEDFILEINFO dwFileVersionMS/LS
    return f'{ms >> 16}.{ms & 0xFFFF}.{ls >> 16}.{ls & 0xFFFF}'


def gta_running():
    out = subprocess.run(['tasklist', '/FI', 'IMAGENAME eq GTA5.exe', '/NH'], capture_output=True, text=True).stdout
    return 'GTA5.exe' in out


def preflight(gta, require_scripthook=True):
    """Problems that stop an install, as messages (empty: go)."""
    gta = Path(gta)
    exe = gta / 'GTA5.exe'
    if not exe.is_file():
        return [f'GTA5.exe not found in {gta}']
    problems = []
    version = file_version(exe)
    if version != REQUIRED_GTA:
        problems.append(f'GTA5.exe is {version or "unknown"}; SK8V needs GTA V Legacy {REQUIRED_GTA}')
    if not (gta / 'update' / 'update.rpf').is_file():
        problems.append(r'update\update.rpf is missing: the GTA install is incomplete (verify or reinstall GTA V first)')
    if gta_running():
        problems.append('GTA V is running; close it first')
    if require_scripthook and not (gta / 'ScriptHookV.dll').is_file():
        problems.append('ScriptHookV.dll not found: install ScriptHookV and its ASI loader (dinput8.dll) first')
    if not writable(gta):
        problems.append(f'no write access to {gta}: run SK8V Setup as administrator')
    return problems


def writable(folder):
    """Whether a file can be created in `folder`. os.access cannot tell on
    Windows (it reads the read-only flag, not the folder's permissions, which
    a GTA re-download or repair resets to read-only for users)."""
    probe = Path(folder) / f'.sk8v-write-test-{os.getpid()}'
    try:
        probe.write_bytes(b'')
        probe.unlink()
        return True
    except OSError:
        return False


def read_manifest(gta):
    """{relative path: sha256 or None (legacy manifest: owned, hash unknown)}."""
    p = Path(gta) / MANIFEST
    if p.exists():
        m = json.loads(p.read_text(encoding='utf-8'))
        if m.get('format') != 'sk8v-install':
            raise SetupError(f'{p}: not an SK8V manifest')
        return dict(m['files'])
    legacy = Path(gta) / LEGACY_MANIFEST
    if legacy.exists():  # the dev installer's list; its log entries were never ours to remove
        return {l.strip(): None for l in legacy.read_text(encoding='ascii').splitlines()
                if l.strip() and not l.strip().lower().endswith('.log')}
    return {}


def write_manifest(gta, files):
    p = Path(gta) / MANIFEST
    tmp = p.with_name(p.name + NEW)
    tmp.write_text(json.dumps({'format': 'sk8v-install', 'version': 1, 'gta': REQUIRED_GTA, 'files': files},
                              indent=1, sort_keys=True), encoding='utf-8')
    os.replace(tmp, p)
    (Path(gta) / LEGACY_MANIFEST).unlink(missing_ok=True)


def ini_values(text):
    out = {}
    for line in text.splitlines():
        key, sep, value = line.partition('=')
        if sep and key.strip().isidentifier() and not key.startswith(';'):
            out[key.strip()] = value.strip()
    return out


def render_ini(template, values, old_text, keep):
    """The template with `values` filled in; keys in `keep` (and MENU_KEYS) take
    the old INI's value when it has one, and old keys the template lacks are
    appended."""
    old = ini_values(old_text or '')
    keep = set(MENU_KEYS) | set(keep)
    lines, seen = [], set()
    for line in template.splitlines():
        for k, v in values.items():
            line = line.replace('{' + k + '}', str(v))
        key, sep, _ = line.partition('=')
        if sep and key in keep:
            seen.add(key)
            if key in old:
                line = f'{key}={old[key]}'
        lines.append(line)
    lines += [f'{k}={old[k]}' for k in sorted(keep) if k in old and k not in seen]
    return '\r\n'.join(lines) + '\r\n'


def ours(rel):
    """SK8V's own names (SkateV* files, the skatev pack folder): a copy left
    from an earlier install without a manifest is still ours to replace."""
    rel = rel.replace('/', '\\').lower()
    return rel.startswith('update\\x64\\dlcpacks\\skatev\\') or rel.rsplit('\\', 1)[-1].startswith('skatev')


def recover(gta, rels):
    """Undo an interrupted transaction: leftover new copies go, old copies come back."""
    for rel in rels:
        target = Path(gta) / rel
        Path(str(target) + NEW).unlink(missing_ok=True)
        old = Path(str(target) + OLD)
        if old.exists():
            os.replace(old, target)


def install(gta, plan, log=print):
    gta = Path(gta)
    problems = preflight(gta, plan.get('require_scripthook', True))
    if problems:
        raise SetupError('; '.join(problems))
    owned = read_manifest(gta)
    files = dict(plan['files'])
    recover(gta, set(files) | set(owned) | {INI})
    allowed = set(plan.get('allow_existing', []))
    for rel, src in files.items():
        if not Path(src).is_file():
            raise SetupError(f'missing payload file {src}')
        if (gta / rel).exists() and rel not in owned and rel not in allowed and not ours(rel):
            raise SetupError(f'{rel} already exists and was not installed by SK8V; refusing to overwrite it')
    ini = gta / INI
    old_ini = ini.read_text(encoding='utf-16') if ini.exists() else None
    ini_text = render_ini(TEMPLATE.read_text(encoding='utf-8'), plan.get('ini', {}), old_ini, plan.get('keep', []))

    # Stage every new file beside its target, then swap them in; any failure puts everything back.
    staged, created = [], []
    try:
        for rel, src in files.items():
            dst = Path(str(gta / rel) + NEW)
            dst.parent.mkdir(parents=True, exist_ok=True)
            with open(src, 'rb') as a, open(dst, 'wb') as b:
                b.write(a.read())
            staged.append(rel)
        Path(str(ini) + NEW).write_text(ini_text, encoding='utf-16')
        staged.append(INI)
        for rel in staged:
            target = gta / rel
            if target.exists():
                os.replace(target, Path(str(target) + OLD))
            else:
                created.append(rel)
            os.replace(Path(str(target) + NEW), target)
        record = {rel: sha256(gta / rel) for rel in staged}
        for rel in set(owned) - set(record):  # dropped from this version: remove if still ours
            if not remove_owned(gta, rel, owned[rel], force=False, log=log):
                record[rel] = owned[rel]
        write_manifest(gta, record)
    except BaseException:
        for rel in created:
            (gta / rel).unlink(missing_ok=True)
        recover(gta, staged)
        raise
    for rel in staged:
        Path(str(gta / rel) + OLD).unlink(missing_ok=True)
    log(f'installed {len(staged)} files into {gta}')
    return record


def remove_owned(gta, rel, recorded, force, log=print):
    """Removes rel if it still matches what was installed (or force). True when gone."""
    p = Path(gta) / rel
    if not p.exists():
        return True
    if recorded and not force and rel != INI and sha256(p) != recorded:
        log(f'kept {rel}: changed since SK8V installed it')
        return False
    p.unlink()
    log(f'removed {rel}')
    for parent in p.parents:  # directories SK8V created, once empty
        if parent == Path(gta) or any(parent.iterdir()):
            break
        parent.rmdir()
    return True


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('command', choices=['install', 'preflight'])
    ap.add_argument('--gta', required=True, type=Path)
    ap.add_argument('--plan', type=Path)
    a = ap.parse_args()
    try:
        if a.command == 'install':
            install(a.gta, json.loads(a.plan.read_text(encoding='utf-8-sig')))
        else:
            problems = preflight(a.gta)
            print('\n'.join(problems) or 'ok')
            sys.exit(1 if problems else 0)
    except SetupError as e:
        sys.exit(f'setup: {e}')


if __name__ == '__main__':
    main()

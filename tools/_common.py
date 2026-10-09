"""Helpers shared by the tools/*.py scripts. prepare-skate-audio.py is shipped
alone in the runtime package, so it keeps its own copies instead."""
import hashlib
import json
import os
import shutil
import subprocess
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def sha256_file(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def joaat(text):
    """GTA's case-insensitive Jenkins one-at-a-time hash."""
    h = 0
    for c in text.lower().encode():
        h = (h + c) & 0xFFFFFFFF
        h = (h + (h << 10)) & 0xFFFFFFFF
        h ^= h >> 6
    h = (h + (h << 3)) & 0xFFFFFFFF
    h ^= h >> 11
    return (h + (h << 15)) & 0xFFFFFFFF


def donor_commit():
    """The pinned Skate donor commit, from upstreams.lock.json."""
    lock = json.loads((REPO / 'upstreams.lock.json').read_text(encoding='utf-8'))
    return next(r['commit'] for r in lock['repositories'] if r['name'] == 'skate-3-rust-engine-donor')


DONOR = REPO / 'upstream/skate-3-rust-engine-donor'
# A release ships the donor's tools as a snapshot without .git; the release build
# writes the commit it verified here.
DONOR_PIN = DONOR / '.sk8v-pin'


def donor_head():
    """The donor tree's commit: git in a developer checkout, else the release pin file."""
    if (DONOR / '.git').exists():
        return subprocess.run(['git', '-c', f'safe.directory={DONOR.as_posix()}', '-C', str(DONOR), 'rev-parse', 'HEAD'],
                              capture_output=True, text=True, check=True).stdout.strip()
    return DONOR_PIN.read_text(encoding='ascii').strip() if DONOR_PIN.exists() else None


def verify_pins():
    """tools/verify-upstreams.ps1 owns the lock format and checks every fetched upstream.
    A release (no .git) carries prebuilt tools and a pinned donor snapshot instead."""
    if not (REPO / '.git').exists():
        return
    env = os.environ.copy()
    env.update(GIT_CONFIG_COUNT='1', GIT_CONFIG_KEY_0='safe.directory', GIT_CONFIG_VALUE_0='*')
    subprocess.run([shutil.which('pwsh') or 'powershell', '-ExecutionPolicy', 'Bypass', '-File',
                    str(REPO / 'tools/verify-upstreams.ps1')], check=True, env=env, cwd=REPO)


def run_rage(rage, *args):
    subprocess.run([str(rage), '--no-update-check', *map(str, args)], check=True)

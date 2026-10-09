"""Convert the user's own extracted Skate 3 (Xbox 360) data into the asset tree
the adopted Skate runtime reads (`Session::new(root, ...)`).

Mirrors the pinned mashup's `skate/converter/iw4l_skate_convert.py`
(Apache-2.0, chasmlol/2010-rust-rewrite-mashup @ ab43b8a): it runs the donor
engine's own `tools/asset_pipeline` exports (`core` + `character`) in place
from `upstream/skate-3-rust-engine-donor`. No donor code is copied into this
repository; the upstream tree is executed read-only (bytecode writes disabled).

Exports: `core` + `character` (Skate session) and `hud` (the original APT
`trickdisplay` scoring HUD + session-marker HUD, donor
`asset_exports.hud` -> `tools/prepare_runtime_huds.py`). All three by default.

    python tools/convert-skate-data.py --xex Skate_3/default.xex --out local/skate-data
    # add only the HUD to an existing conversion (core is not redone):
    python tools/convert-skate-data.py --xex Skate_3/default.xex --out local/skate-data --exports hud --incremental

Output is retail-derived and must never be committed (`local/` is ignored).
"""
from pathlib import Path
import argparse, json, os, shutil, sys, tempfile, traceback

sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from _common import REPO, donor_commit, donor_head

DONOR = REPO / 'upstream' / 'skate-3-rust-engine-donor'
DONOR_COMMIT = donor_commit()

REQUIRED = [
    'data/big/miscload.big',
    'data/big/miscboot.big',
    'data/big/db.big',
    'data/content/createacharacter.big',
    # the hud export's APT movies and their textures
    'data/big/fedata.big',
    'data/big/fetexture.big',
    'data/big/fedynamic.big',
]
PRODUCED = {
    'core': ['private/stock/physics-skeletons.json', 'private/stock/skater-collections.json'],
    'character': ['private/skater.glb', 'private/game.json'],
    'hud': ['private/hud/runtime/trickdisplay.json'],
}
EXPORTS = ['core', 'character', 'hud']
# Directories (under assets/private) a `hud` export owns; incremental runs
# replace exactly these in an existing conversion.
HUD_DIRS = ['hud', 'session-marker']


def check_inputs(xex):
    xex = xex.resolve()
    if xex.name.lower() != 'default.xex' or not xex.is_file():
        raise RuntimeError(f'expected an extracted Skate 3 default.xex, got {xex}')
    game = xex.parent
    missing = [p for p in REQUIRED if not (game / p).is_file()]
    if missing:
        raise RuntimeError('missing Skate 3 data beside default.xex: ' + ', '.join(missing))
    head = donor_head()
    if head != DONOR_COMMIT:
        raise RuntimeError(f'donor checkout at {head}, expected {DONOR_COMMIT}; run tools/fetch-upstreams.ps1')
    # Child task scripts are started with sys.executable and import `tools.*`.
    os.environ['PYTHONPATH'] = os.pathsep.join(filter(None, [str(DONOR), os.environ.get('PYTHONPATH')]))
    sys.path.insert(0, str(DONOR))
    return xex, game, head


def run_exports(names, game, stage, report):
    from tools.asset_pipeline import asset_exports as exports
    with tempfile.TemporaryDirectory(prefix='skatev-convert-', dir=stage.parent) as work, \
            (stage / 'conversion.log').open('a', encoding='utf-8') as log:
        work = Path(work)
        converted = None
        for name in names:
            result = getattr(exports, name)(game, stage, work, report, log, converted)
            if name == 'core':
                converted = result
    assets = stage / 'assets'
    for name in names:
        for needed in PRODUCED[name]:
            if not (assets / needed).is_file():
                raise RuntimeError(f'{name} export finished without {needed}')


def write_receipt(path, head, xex, exports):
    path.write_text(json.dumps({
        'converter': 'tools/convert-skate-data.py',
        'donor_commit': head,
        'exports': exports,
        'xex_size': xex.stat().st_size,
        'required_inputs': REQUIRED,
    }, indent=2), encoding='utf-8')


def convert(xex, out, names):
    xex, game, head = check_inputs(xex)
    if 'hud' in names and 'core' not in names:
        raise RuntimeError('hud needs core output (skater-collections.json); add core or use --incremental')
    out = out.resolve()
    stage = out.with_name(out.name + '.partial')
    shutil.rmtree(stage, ignore_errors=True)
    stage.mkdir(parents=True)

    def report(text):
        print(text, flush=True)

    run_exports(names, game, stage, report)
    write_receipt(stage / 'receipt.json', head, xex, names)
    shutil.rmtree(out, ignore_errors=True)
    stage.rename(out)
    report(f'Skate 3 data ready: {out / "assets"}')


def convert_incremental(xex, out, names):
    """Adds exports to an existing conversion without redoing the others.

    Only `hud` is incremental: it reads the existing core output
    (skater-collections.json) and replaces `assets/private/{hud,session-marker}`.
    """
    xex, game, head = check_inputs(xex)
    out = out.resolve()
    receipt_path = out / 'receipt.json'
    if not receipt_path.is_file():
        raise RuntimeError(f'no existing conversion at {out}; run without --incremental first')
    receipt = json.loads(receipt_path.read_text(encoding='utf-8'))
    if receipt.get('donor_commit') != head:
        raise RuntimeError(f'existing conversion was made from donor {receipt.get("donor_commit")}, not {head}')
    unsupported = [n for n in names if n != 'hud']
    if unsupported:
        raise RuntimeError('--incremental supports only the hud export, not ' + ', '.join(unsupported))
    collections = out / 'assets' / 'private' / 'stock' / 'skater-collections.json'
    if not collections.is_file():
        raise RuntimeError(f'existing conversion lacks {collections} (core export)')

    def report(text):
        print(text, flush=True)

    stage = out.with_name(out.name + '.hud-partial')
    shutil.rmtree(stage, ignore_errors=True)
    (stage / 'assets' / 'private' / 'stock').mkdir(parents=True)
    shutil.copyfile(collections, stage / 'assets' / 'private' / 'stock' / 'skater-collections.json')
    run_exports(['hud'], game, stage, report)
    for name in HUD_DIRS:
        produced = stage / 'assets' / 'private' / name
        target = out / 'assets' / 'private' / name
        if produced.is_dir():
            shutil.rmtree(target, ignore_errors=True)
            produced.rename(target)
    with (stage / 'conversion.log').open(encoding='utf-8') as src, \
            (out / 'conversion.log').open('a', encoding='utf-8') as dst:
        dst.write(src.read())
    exports = [e for e in EXPORTS if e in set(receipt.get('exports', [])) | {'hud'}]
    write_receipt(receipt_path, head, xex, exports)
    shutil.rmtree(stage, ignore_errors=True)
    report(f'Skate 3 HUD added: {out / "assets" / "private" / "hud"}')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--xex', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=REPO / 'local' / 'skate-data')
    parser.add_argument('--exports', default=','.join(EXPORTS),
                        help='comma-separated donor exports (default: core,character,hud)')
    parser.add_argument('--incremental', action='store_true',
                        help='add exports (hud) to an existing --out without redoing core')
    args = parser.parse_args()
    names = [n.strip() for n in args.exports.split(',') if n.strip()]
    bad = [n for n in names if n not in EXPORTS]
    if bad or not names:
        parser.error(f'unknown exports {bad}; choose from {EXPORTS}')
    names = [n for n in EXPORTS if n in names]  # dependency order
    try:
        if args.incremental:
            convert_incremental(args.xex, args.out, names)
        else:
            convert(args.xex, args.out, names)
    except Exception as error:
        traceback.print_exc()
        print(f'ERROR: {error}', flush=True)
        return 2
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

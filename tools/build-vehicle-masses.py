"""Vehicle mass table for the GTA <-> Skate contact exchange (local only).

Reads the user's own handling.meta / vehicles.meta, extracted into
local/gta-vehdata/<NN>_<archive>/ (common.rpf, every dlcpacks/*/dlc.rpf,
update.rpf), and writes one line per vehicle model:

    <model joaat hex> <fMass> <inertia multiplier x y z> <centre of mass offset x y z>

Override order: base common.rpf, then DLC packs, then update.rpf (which carries
common/data overrides and dlc_patch copies of the packs); within an archive,
dlc_patch paths win. Later definitions of a handlingName or modelName replace
earlier ones. Values are GTA's authored data; nothing is estimated here.

    python tools/build-vehicle-masses.py local/gta-vehdata local/vehicle-masses.txt
"""
import argparse
import os
import sys
import xml.etree.ElementTree as ET

from _common import joaat


def layer_rank(name: str) -> tuple:
    # 00_common first, DLC packs next, update.rpf last.
    if name.endswith("_common"):
        return (0, name)
    if name.endswith("_update"):
        return (2, name)
    return (1, name)


def files(root: str, suffix: str):
    out = []
    for layer in sorted(os.listdir(root), key=layer_rank):
        for dp, _, fn in os.walk(os.path.join(root, layer)):
            for f in fn:
                if f.lower().endswith(suffix):
                    p = os.path.join(dp, f)
                    out.append((layer_rank(layer), "dlc_patch" in p.replace("\\", "/"), p))
    out.sort(key=lambda t: (t[0], t[1], t[2]))
    return [p for _, _, p in out]


def parse(path: str):
    data = open(path, "rb").read()
    # Some files carry a BOM or stray bytes before the declaration.
    start = data.find(b"<")
    return ET.fromstring(data[start:])


def vec(item, tag):
    e = item.find(tag)
    if e is None:
        return (0.0, 0.0, 0.0)
    return tuple(float(e.get(k, "0")) for k in ("x", "y", "z"))


def main(root: str, out: str) -> None:
    handling = {}
    for p in files(root, "handling.meta"):
        try:
            tree = parse(p)
        except ET.ParseError as e:
            print(f"skip {p}: {e}", file=sys.stderr)
            continue
        for item in tree.iter("Item"):
            name = item.findtext("handlingName")
            mass = item.find("fMass")
            if not name or mass is None:
                continue
            handling[name.strip().upper()] = (
                float(mass.get("value")),
                vec(item, "vecInertiaMultiplier"),
                vec(item, "vecCentreOfMassOffset"),
            )
    models = {}
    for p in files(root, "vehicles.meta"):
        try:
            tree = parse(p)
        except ET.ParseError as e:
            print(f"skip {p}: {e}", file=sys.stderr)
            continue
        for item in tree.iter("Item"):
            model = item.findtext("modelName")
            hid = item.findtext("handlingId")
            if model and hid:
                models[model.strip().lower()] = hid.strip().upper()
    lines = []
    missing = 0
    for model, hid in sorted(models.items()):
        h = handling.get(hid)
        if h is None:
            missing += 1
            continue
        mass, inertia, com = h
        if not (mass > 0.0):
            continue
        lines.append(f"{joaat(model):08x} {mass:.3f} {inertia[0]:.4f} {inertia[1]:.4f} {inertia[2]:.4f} "
                     f"{com[0]:.4f} {com[1]:.4f} {com[2]:.4f}  # {model} {hid}")
    with open(out, "w", encoding="utf-8") as f:
        f.write("# SkateV vehicle masses v1: model mass inertia_multiplier.xyz centre_of_mass_offset.xyz\n")
        f.write("\n".join(lines) + "\n")
    print(f"{len(lines)} vehicle models, {len(handling)} handling entries, {missing} models without handling -> {out}")


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("root", help="local/gta-vehdata")
    p.add_argument("out", help="vehicle masses file to write")
    a = p.parse_args()
    main(a.root, a.out)

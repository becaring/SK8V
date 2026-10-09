"""WEAPON_SKATEBOARD for the skatev DLC pack (roadmap Phase 2: the board in
GTA's weapon wheel).

Clones the retail WEAPON_HAMMER (one-hand melee, B4's v3 carry: weapons@melee_1h,
melee@small_wpn@streamed_core) from the owner's own update.rpf meta into
weapons_skatev.meta and weaponanimations_skatev.meta under its own slot.
The hammer model stays the weapon model; the host hides it and shows the
native board. Melee damage comes from GTA's action tables (every retail melee
weapon has Damage 0), so the clone hits like the hammer.

usage: build-board-weapon.py [weapons.meta] [weaponanimations.meta] [out dir]
defaults: local/gta-weapon-meta/common/data/ai/*, local/live-clip/weapon
(extract with: rage.exe extract -r <GTA>/update/update.rpf '*common/data/ai/weapons.meta'
'*weaponanimations.meta' -o local/gta-weapon-meta)
"""
import copy
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = 'WEAPON_HAMMER'
NAME = 'WEAPON_SKATEBOARD'
SLOT = 'SLOT_SKATEBOARD'


def weapon_info(src):
    blob = ET.parse(src).getroot()
    infos = [i for i in blob.iter('Item') if i.get('type') == 'CWeaponInfo' and i.findtext('Name') == SRC]
    assert len(infos) == 1, f'{SRC} not found once in {src}'
    w = copy.deepcopy(infos[0])
    w.find('Name').text = NAME
    w.find('Slot').text = SLOT
    w.find('HumanNameHash').text = 'WT_SKATEBOARD'
    w.find('PickupHash').text = None  # no hammer pickup dropped on death
    # Slot orders: the wheel steps fists -> board (owner 2026-10-07: the board
    # first after fists); the best-weapon list keeps it after the hammer so
    # GTA never auto-equips it.
    order = {}
    navs = blob.find('SlotNavigateOrder').findall('Item')
    for lst in navs + [blob.find('SlotBestOrder')]:
        after = 'SLOT_UNARMED' if lst in navs else 'SLOT_HAMMER'
        for it in lst.find('WeaponSlots').findall('Item'):
            if it.findtext('Entry') == after:
                order.setdefault(id(lst), int(it.find('OrderNumber').get('value')) + 5)
    nav = [order[id(l)] for l in blob.find('SlotNavigateOrder').findall('Item')]
    best = order[id(blob.find('SlotBestOrder'))]

    def slots(parent, n):
        ws = ET.SubElement(parent, 'WeaponSlots')
        it = ET.SubElement(ws, 'Item')
        ET.SubElement(it, 'OrderNumber', value=str(n))
        ET.SubElement(it, 'Entry').text = SLOT

    out = ET.Element('CWeaponInfoBlob')
    sno = ET.SubElement(out, 'SlotNavigateOrder')
    for n in nav:
        slots(ET.SubElement(sno, 'Item'), n)
    slots(ET.SubElement(out, 'SlotBestOrder'), best)
    for tag in ('TintSpecValues', 'FiringPatternAliases', 'UpperBodyFixupExpressionData', 'AimingInfos'):
        ET.SubElement(out, tag)
    item = ET.SubElement(ET.SubElement(out, 'Infos'), 'Item')
    ET.SubElement(item, 'Infos').append(w)
    ET.SubElement(out, 'VehicleWeaponInfos')
    ET.SubElement(out, 'Name').text = 'SkateV - Skateboard'
    return out, nav, best


def animations(src):
    sets = ET.parse(src).getroot().find('WeaponAnimationsSets')
    out = ET.Element('CWeaponAnimationsSets')
    oset = ET.SubElement(out, 'WeaponAnimationsSets')
    n = 0
    for s in sets.findall('Item'):
        hit = [a for a in s.find('WeaponAnimations').findall('Item') if a.get('key') == SRC]
        if not hit:
            continue
        o = ET.SubElement(oset, 'Item', key=s.get('key'))
        ET.SubElement(o, 'Fallback').text = s.findtext('Fallback') or None
        a = copy.deepcopy(hit[0])
        a.set('key', NAME)
        ET.SubElement(o, 'WeaponAnimations').append(a)
        n += 1
    assert n, f'{SRC} has no animation sets in {src}'
    return out, n


def write(tree, path):
    ET.indent(tree)
    path.write_bytes(b'<?xml version="1.0" encoding="UTF-8"?>\n' + ET.tostring(tree, encoding='utf-8') + b'\n')


def main():
    a = sys.argv[1:]
    meta = ROOT / 'local/gta-weapon-meta/common/data/ai'
    weapons = Path(a[0]) if a else meta / 'weapons.meta'
    anims = Path(a[1]) if len(a) > 1 else meta / 'weaponanimations.meta'
    out = Path(a[2]) if len(a) > 2 else ROOT / 'local/live-clip/weapon'
    out.mkdir(parents=True, exist_ok=True)
    w, nav, best = weapon_info(weapons)
    an, sets = animations(anims)
    write(w, out / 'weapons_skatev.meta')
    write(an, out / 'weaponanimations_skatev.meta')
    # Self-check: the written files parse back and name the weapon once.
    back = ET.parse(out / 'weapons_skatev.meta').getroot()
    assert [i.findtext('Name') for i in back.iter('Item') if i.get('type') == 'CWeaponInfo'] == [NAME]
    print(f'{NAME} from {SRC}: slot orders {nav} / best {best}, {sets} animation sets -> {out}')


if __name__ == '__main__':
    main()

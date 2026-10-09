//! Door identity from explicit Meta extension links or the door-physics flag.
//! Format oracle: CodeWalker MetaTypes.cs CEntityDef.extensions @96,
//! CBaseArchetypeDef.extensions @120; MetaNames.CExtensionDefDoor = 1965932561.
//! Only binary field layout facts are used; no reference implementation copied.
//! CodeWalker EditYtypArchetypePanel labels flag 0x04000000 Enable Door Physics;
//! CBaseArchetypeDef.flags is at +12. Script-controlled gates need no extension.
use rage_formats::YmapEntity;
use rage_formats::resource::{ResReader, SYSTEM_BASE, prepare_rsc7, u16_le, u32_le, u64_le};
use std::collections::HashSet;

const ENTITY: u32 = 3_461_354_627;
const DOOR: u32 = 1_965_932_561;
const MAP_TYPES: u32 = 3_649_811_809;
const ARCHETYPES: [u32; 3] = [2_195_127_427, 1_991_296_364, 273_704_021];

// Identity of the source placement, before composition with an MLO transform.
// Same model elsewhere (even another instance within the same file) is distinct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EntityKey([u32; 12]);
impl EntityKey {
    fn from_entity(e: &YmapEntity) -> Self {
        Self([
            e.archetype_hash,
            e.flags,
            e.guid,
            e.position.x.to_bits(),
            e.position.y.to_bits(),
            e.position.z.to_bits(),
            e.rotation[0].to_bits(),
            e.rotation[1].to_bits(),
            e.rotation[2].to_bits(),
            e.rotation[3].to_bits(),
            e.scale_xy.to_bits(),
            e.scale_z.to_bits(),
        ])
    }
    fn from_bytes(e: &[u8]) -> Self {
        Self([8, 12, 16, 32, 36, 40, 48, 52, 56, 60, 64, 68].map(|o| u32_le(e, o)))
    }
}
#[derive(Default, Debug, Clone)]
pub struct Metadata {
    pub archetypes: HashSet<u32>,
    entities: HashSet<EntityKey>,
}
impl Metadata {
    pub fn entity_is_door(&self, e: &YmapEntity) -> bool {
        self.entities.contains(&EntityKey::from_entity(e))
    }
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }
    pub fn entity_models(&self) -> impl Iterator<Item = u32> + '_ {
        self.entities.iter().map(|e| e.0[0])
    }
}
struct Block<'a> {
    kind: u32,
    data: &'a [u8],
}
fn pointer<'a>(blocks: &[Block<'a>], raw: u64, len: usize) -> Option<(u32, &'a [u8])> {
    let id = (raw & 0xfff) as usize;
    let offset = ((raw >> 12) & 0xfffff) as usize;
    let b = blocks.get(id.checked_sub(1)?)?;
    Some((b.kind, b.data.get(offset..offset.checked_add(len)?)?))
}
fn array<'a>(blocks: &'a [Block<'a>], owner: &[u8], offset: usize) -> Result<&'a [u8], String> {
    let field = owner
        .get(offset..offset + 16)
        .ok_or("truncated extension/list field")?;
    let count = u16_le(field, 8) as usize;
    if count == 0 {
        return Ok(&[]);
    }
    pointer(blocks, u64_le(field, 0), count * 8)
        .map(|(_, data)| data)
        .ok_or_else(|| "invalid metadata pointer array".into())
}
fn has_door(blocks: &[Block<'_>], record: &[u8], offset: usize) -> Result<bool, String> {
    for p in array(blocks, record, offset)?.chunks_exact(8) {
        let (kind, _) = pointer(blocks, u64_le(p, 0), 1).ok_or("invalid extension pointer")?;
        if kind == DOOR {
            pointer(blocks, u64_le(p, 0), 48).ok_or("truncated door extension")?;
            return Ok(true);
        }
    }
    Ok(false)
}
pub fn read(data: &[u8]) -> Result<Metadata, String> {
    let (system, graphics) = prepare_rsc7(data).map_err(|e| e.to_string())?;
    read_resource(&ResReader {
        system: &system,
        graphics: &graphics,
    })
}
fn read_resource(reader: &ResReader<'_>) -> Result<Metadata, String> {
    let header = reader
        .resolve(SYSTEM_BASE, 0x70)
        .ok_or("missing Meta header")?;
    let count = u16_le(header, 0x4c) as usize;
    let descriptors = reader
        .resolve(u64_le(header, 0x30), count * 16)
        .ok_or("invalid Meta blocks")?;
    let mut blocks = Vec::with_capacity(count);
    for d in descriptors.chunks_exact(16) {
        let data = reader
            .resolve(u64_le(d, 8), u32_le(d, 4) as usize)
            .ok_or("invalid Meta block extent")?;
        blocks.push(Block {
            kind: u32_le(d, 0),
            data,
        });
    }
    let mut out = Metadata::default();
    // CEntityDef records have a fixed 128-byte stride, in both YMAP and MLO
    // furniture data. Only records carrying a resolved door link are marked.
    for block in blocks.iter().filter(|b| b.kind == ENTITY) {
        for entity in block.data.chunks_exact(128) {
            if has_door(&blocks, entity, 96)? {
                out.entities.insert(EntityKey::from_bytes(entity));
            }
        }
    }
    // Archetype list pointers preserve the varying Base/Time/MLO record sizes.
    for root in blocks.iter().filter(|b| b.kind == MAP_TYPES) {
        for p in array(&blocks, root.data, 24)?.chunks_exact(8) {
            let raw = u64_le(p, 0);
            if raw == 0 {
                continue;
            }
            let (kind, _) = pointer(&blocks, raw, 1).ok_or("invalid archetype pointer")?;
            if !ARCHETYPES.contains(&kind) {
                continue;
            }
            // The pinned archetype parser also skips pre-Legacy 128-byte
            // records. They never reach the prop bake.
            let Some((_, base)) = pointer(&blocks, raw, 144) else {
                continue;
            };
            let extension_door = has_door(&blocks, base, 120)?;
            if extension_door || u32_le(base, 12) & 0x0400_0000 != 0 {
                let name = u32_le(base, 88);
                out.archetypes
                    .insert(if name == 0 { u32_le(base, 112) } else { name });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn w32(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn w64(b: &mut [u8], o: usize, v: u64) {
        b[o..o + 8].copy_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn fixture() -> Vec<u8> {
        let mut b = vec![0; 0x900];
        // 1 root, 2 pointer array, 3 two entities, 4 archetype, 5 extensions, 6 door.
        let rows = [
            (MAP_TYPES, 0x100, 80),
            (0, 0x180, 8),
            (ENTITY, 0x200, 256),
            (ARCHETYPES[0], 0x400, 144),
            (0, 0x500, 8),
            (DOOR, 0x600, 48),
        ];
        w64(&mut b, 0x30, SYSTEM_BASE + 0x70);
        b[0x4c] = rows.len() as u8;
        for (i, (kind, offset, len)) in rows.into_iter().enumerate() {
            w32(&mut b, 0x70 + i * 16, kind);
            w32(&mut b, 0x74 + i * 16, len);
            w64(&mut b, 0x78 + i * 16, SYSTEM_BASE + offset);
        }
        w64(&mut b, 0x118, 2);
        b[0x120] = 1;
        w64(&mut b, 0x180, 4);
        for e in [0x200, 0x280] {
            w32(&mut b, e + 8, 0x1234);
            w32(&mut b, e + 60, 1f32.to_bits());
            w32(&mut b, e + 64, 1f32.to_bits());
            w32(&mut b, e + 68, 1f32.to_bits());
        }
        w32(&mut b, 0x280 + 32, 5f32.to_bits());
        w64(&mut b, 0x260, 5);
        b[0x268] = 1;
        w32(&mut b, 0x458, 0x5678);
        w64(&mut b, 0x478, 5);
        b[0x480] = 1;
        w64(&mut b, 0x500, 6);
        b
    }
    fn parse(b: &[u8]) -> Result<Metadata, String> {
        read_resource(&ResReader {
            system: b,
            graphics: &[],
        })
    }
    #[test]
    fn explicit_archetype_and_entity_doors_only() {
        let b = fixture();
        let m = parse(&b).unwrap();
        assert!(m.archetypes.contains(&0x5678));
        assert_eq!(m.entity_count(), 1);
        assert!(
            m.entities
                .contains(&EntityKey::from_bytes(&b[0x200..0x280]))
        );
        assert!(
            !m.entities
                .contains(&EntityKey::from_bytes(&b[0x280..0x300]))
        );
    }
    #[test]
    fn unrelated_extensions_do_not_exclude_props() {
        let mut b = fixture();
        w32(&mut b, 0x70 + 5 * 16, 663891011); // light extension
        let m = parse(&b).unwrap();
        assert!(m.archetypes.is_empty());
        assert_eq!(m.entity_count(), 0);
    }
    #[test]
    fn door_physics_archetype_without_extension_stays_dynamic() {
        for kind in ARCHETYPES {
            let mut b = fixture();
            w32(&mut b, 0x70 + 3 * 16, kind);
            b[0x480] = 0; // No CExtensionDefDoor: script-controlled sliding gate.
            w32(&mut b, 0x40c, 0x2402_0000); // Owned prop_lrggate_02_ld flags.
            assert!(parse(&b).unwrap().archetypes.contains(&0x5678));
            w32(&mut b, 0x40c, 0x2002_0000); // Dynamic alone is not a door.
            assert!(!parse(&b).unwrap().archetypes.contains(&0x5678));
        }
    }
    #[test]
    fn malformed_extension_fails_explicitly() {
        let mut b = fixture();
        w64(&mut b, 0x500, 0xfff);
        assert!(parse(&b).is_err());
        let mut b = fixture();
        w32(&mut b, 0x74 + 5 * 16, 16);
        assert!(parse(&b).is_err());
    }
    #[test]
    fn archetype_asset_alias_is_used_when_name_is_zero() {
        let mut b = fixture();
        w32(&mut b, 0x458, 0);
        w32(&mut b, 0x470, 0x9876);
        assert!(parse(&b).unwrap().archetypes.contains(&0x9876));
    }
}

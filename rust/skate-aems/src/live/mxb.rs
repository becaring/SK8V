//! Bounded reader for the MixMapSK8 data. Offsets are relative to the
//! enclosing map/type; absent blocks use -1. The first type's unused tag word
//! overlaps the final directory entry in the retail file, so it is not an ID.
//! Record interpretation follows the MXB loader's variable declarations
//! (8294CDE8/82951E00), without reproducing its guest allocator or pointers.

#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub source: u32,
    pub flags: u16,
    pub value: u16,
    pub links: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Type {
    pub variables: Vec<Variable>,
    pub spatial: Vec<[u32; 7]>,
    pub sums: Vec<Sum>,
    pub outputs: Vec<Output>,
    pub destinations: Vec<Destination>,
    pub controls: Vec<Control>,
    pub banks: usize,
    /// The seven optional definition blocks. Kept as owned bytes so later
    /// stages cannot follow unchecked retail offsets into another allocation.
    pub blocks: [Option<Vec<u8>>; 7],
}

#[derive(Debug, Clone)]
pub struct Sum {
    pub definition: u32,
    pub limits: u32,
    pub sources: Vec<u32>,
}
#[derive(Debug, Clone)]
pub struct Output {
    pub definition: u32,
    pub limits: u32,
    pub handle: u32,
    pub sources: Vec<u32>,
}
#[derive(Debug, Clone)]
pub struct Destination {
    pub definition: u32,
    pub entries: Vec<u32>,
}
#[derive(Debug, Clone)]
pub struct Control {
    pub words: [u32; 6],
    pub sources: Vec<u32>,
}

fn records<T>(
    block: &Option<Vec<u8>>,
    mut read: impl FnMut(&[u8], &mut usize) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let Some(b) = block else {
        return Ok(Vec::new());
    };
    let count = word(b, 0)? as usize;
    if count > b.len() / 4 {
        return Err("invalid MXB record count".into());
    }
    let mut p = 16;
    let values = (0..count)
        .map(|_| read(b, &mut p))
        .collect::<Result<Vec<_>, _>>()?;
    if p != b.len() {
        return Err("unexpected MXB record block size".into());
    }
    Ok(values)
}
fn take(b: &[u8], p: &mut usize) -> Result<u32, String> {
    let v = word(b, *p)?;
    *p += 4;
    Ok(v)
}
fn take_many(b: &[u8], p: &mut usize, count: usize) -> Result<Vec<u32>, String> {
    (0..count).map(|_| take(b, p)).collect()
}

#[derive(Debug, Clone)]
pub struct MixMap {
    pub types: Vec<Option<Type>>,
}

fn word(b: &[u8], off: usize) -> Result<u32, String> {
    b.get(off..off.checked_add(4).ok_or("MXB offset overflow")?)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
        .ok_or_else(|| "truncated MXB word".into())
}

fn half(b: &[u8], off: usize) -> Result<u16, String> {
    b.get(off..off.checked_add(2).ok_or("MXB offset overflow")?)
        .map(|b| u16::from_be_bytes(b.try_into().unwrap()))
        .ok_or_else(|| "truncated MXB halfword".into())
}

impl MixMap {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if word(bytes, 0)? != 0 {
            return Err("unsupported MXB version".into());
        }
        let count = word(bytes, 4)? as usize;
        if count == 0 || count > 256 {
            return Err("invalid MXB type count".into());
        }
        let table = word(bytes, 8)? as usize;
        let directory_end = table
            .checked_add(count * 4)
            .ok_or("MXB directory overflow")?;
        if table < 16 || directory_end > bytes.len() {
            return Err("invalid MXB directory".into());
        }
        let offsets: Vec<_> = (0..count)
            .map(|i| word(bytes, table + i * 4))
            .collect::<Result<_, _>>()?;
        let mut types = Vec::with_capacity(count);
        for &offset in &offsets {
            if offset == u32::MAX {
                types.push(None);
                continue;
            }
            let start = offset as usize;
            // The ignored first word may alias the last directory word.
            if start < directory_end - 4 || !start.is_multiple_of(4) {
                return Err("invalid MXB type offset".into());
            }
            let end = offsets
                .iter()
                .copied()
                .filter(|p| *p != u32::MAX && *p > offset)
                .min()
                .map_or(bytes.len(), |p| p as usize);
            let ty = bytes
                .get(start..end)
                .filter(|v| v.len() >= 32)
                .ok_or("truncated MXB type")?;
            let mut locations = [None; 7];
            for (i, loc) in locations.iter_mut().enumerate() {
                let rel = word(ty, 4 + 4 * i)?;
                if rel != u32::MAX {
                    let rel = rel as usize;
                    if rel < 32 || !rel.is_multiple_of(4) || rel >= ty.len() {
                        return Err("invalid MXB block offset".into());
                    }
                    *loc = Some(rel);
                }
            }
            let mut blocks: [Option<Vec<u8>>; 7] = std::array::from_fn(|_| None);
            for (i, loc) in locations.iter().enumerate() {
                if let Some(start) = loc {
                    let end = locations
                        .iter()
                        .flatten()
                        .filter(|v| **v > *start)
                        .min()
                        .copied()
                        .unwrap_or(ty.len());
                    blocks[i] = Some(ty[*start..end].to_vec());
                }
            }
            let mut variables = Vec::new();
            if let Some(b) = &blocks[0] {
                let count = word(b, 0)? as usize;
                if count > b.len() / 8 {
                    return Err("invalid MXB variable count".into());
                }
                let mut cursor = 16;
                for _ in 0..count {
                    let source = word(b, cursor)?;
                    let flags = half(b, cursor + 4)?;
                    let value = half(b, cursor + 6)?;
                    let n = (flags & 15) as usize;
                    let links = (0..n)
                        .map(|i| word(b, cursor + 8 + 4 * i))
                        .collect::<Result<_, _>>()?;
                    variables.push(Variable {
                        source,
                        flags,
                        value,
                        links,
                    });
                    cursor += 8 + 4 * n;
                }
                if cursor != b.len() {
                    return Err("unexpected MXB variable block size".into());
                }
            }
            let spatial = records(&blocks[1], |b, p| {
                let values = take_many(b, p, 7)?;
                Ok(values.try_into().unwrap())
            })?;
            let sums = records(&blocks[2], |b, p| {
                let definition = take(b, p)?;
                let limits = take(b, p)?;
                let sources = take_many(b, p, ((definition >> 16) & 255) as usize)?;
                Ok(Sum {
                    definition,
                    limits,
                    sources,
                })
            })?;
            let outputs = records(&blocks[3], |b, p| {
                let definition = take(b, p)?;
                let limits = take(b, p)?;
                let handle = take(b, p)?;
                let sources = take_many(b, p, ((definition >> 16) & 255) as usize)?;
                Ok(Output {
                    definition,
                    limits,
                    handle,
                    sources,
                })
            })?;
            let banks = blocks[3]
                .as_ref()
                .map(|b| word(b, 4))
                .transpose()?
                .unwrap_or(0) as usize;
            let mut destinations = Vec::new();
            if let Some(b) = &blocks[4] {
                let mut p = 0;
                while p < b.len() {
                    let definition = take(b, &mut p)?;
                    let entries = take_many(b, &mut p, (definition & 31) as usize)?;
                    destinations.push(Destination {
                        definition,
                        entries,
                    });
                }
            }
            if outputs.len() != destinations.len() {
                return Err("MXB output/destination count mismatch".into());
            }
            let controls = records(&blocks[5], |b, p| {
                let words: [u32; 6] = take_many(b, p, 6)?.try_into().unwrap();
                let sources = take_many(b, p, ((words[1] >> 16) & 15) as usize)?;
                Ok(Control { words, sources })
            })?;
            types.push(Some(Type {
                variables,
                spatial,
                sums,
                outputs,
                destinations,
                controls,
                banks,
                blocks,
            }));
        }
        Ok(Self { types })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncated_directories_and_offsets_fail() {
        for size in 0..16 {
            assert!(MixMap::parse(&vec![0; size]).is_err());
        }
        let mut b = vec![0; 64];
        b[4..8].copy_from_slice(&1u32.to_be_bytes());
        b[8..12].copy_from_slice(&16u32.to_be_bytes());
        b[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(MixMap::parse(&b).unwrap().types.len(), 1);
        b[16..20].copy_from_slice(&60u32.to_be_bytes());
        assert!(MixMap::parse(&b).is_err());
    }
}

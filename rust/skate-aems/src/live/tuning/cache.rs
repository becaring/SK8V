//! SVAT v1 reader. Counts and lengths are checked before allocation; records
//! are owned data, and source collection identities survive relocation.
use super::*;

const MAX_BYTES: usize = 16 * 1024 * 1024;
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        if count > self.0.len() { return Err("truncated component tuning".into()); }
        let (value, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
}

impl Tuning {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES { return Err("component tuning exceeds size limit".into()); }
        let mut r = Reader(bytes);
        if r.take(4)? != b"SVAT" || r.u32()? != 1 {
            return Err("unsupported component tuning cache".into());
        }
        let collections = r.u32()?;
        let roots = r.u32()?;
        let fixed = r.u32()?;
        if collections == 0 || collections > 4096 || roots > 1024 || fixed > 65536 {
            return Err("invalid component tuning counts".into());
        }
        let mut out = Self::default();
        for _ in 0..collections {
            let class = r.u64()?;
            let key = r.u64()?;
            let count = r.u32()?;
            if class == 0 || key == 0 || count > 4096 || out.collections.contains_key(&(class, key)) {
                return Err("invalid or duplicate component collection".into());
            }
            let handle = out.set_collection(class, key);
            for _ in 0..count {
                let key = r.u64()?;
                let stride = r.u32()?;
                let count = r.u32()?;
                if key == 0 || stride == 0 || stride > 65536 || count > 65536 {
                    return Err("invalid component record dimensions".into());
                }
                let size = (stride as usize).checked_mul(count as usize)
                    .filter(|size| *size <= MAX_BYTES)
                    .ok_or("component record size overflow")?;
                let data = r.take(size)?;
                if out.bytes.len().checked_add(size + 4).filter(|n| *n <= MAX_BYTES).is_none() {
                    return Err("component pool exceeds size limit".into());
                }
                let address = out.alloc(size);
                let offset = (address - BASE) as usize;
                out.bytes[offset..offset + size].copy_from_slice(data);
                if out.records.insert((handle, key), Record { address, stride, count }).is_some() {
                    return Err("duplicate component field".into());
                }
            }
        }
        for _ in 0..roots {
            let root = r.u32()?;
            let class = r.u64()?;
            let key = r.u64()?;
            let handle = out.collections.get(&(class, key)).copied()
                .ok_or("component root references absent collection")?;
            if out.roots.insert(root, handle).is_some() { return Err("duplicate component root".into()); }
        }
        for _ in 0..fixed {
            let address = r.u32()?;
            let value = r.u32()?;
            if !(0x8200_0000..0x8400_0000).contains(&address) || address & 3 != 0
                || out.fixed.insert(address, value).is_some()
            {
                return Err("invalid or duplicate fixed tuning word".into());
            }
        }
        if !r.0.is_empty() { return Err("trailing component tuning bytes".into()); }
        Ok(out)
    }

    pub(super) fn validate_live_roots(&self) -> Result<(), String> {
        for root in [4, 24, 28, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92, 96, 100, 104, 132, 136, 140] {
            if self.root_collection(root).is_none() {
                return Err(format!("component tuning has no live root {root}"));
            }
        }
        for (root, key, stride, count) in [
            (56, 0x880C_82E8_EF64_7EC4, 4, 16),
            (64, 0x4CA6_0755_8B1C_F440, 72, 95),
            (92, 0x6364_64FB_AD0D_71A3, 4, 6),
            (36, super::super::components::IMPULSE_SPEED_CURVE, 80, 1),
        ] {
            let row = self.root_record(root, key).ok_or("missing required component array")?;
            if row.stride != stride || row.count != count {
                return Err(format!("component tuning root {root} array shape mismatch"));
            }
        }
        {
            use super::super::ui_sounds::{EVENT_CLASS, MULTIPLYER_2, MULTIPLYER_3};
            if [MULTIPLYER_2, MULTIPLYER_3].iter().any(|&k| !self.collections.contains_key(&(EVENT_CLASS, k))) {
                return Err("component tuning lacks the combo multiplier sound events".into());
            }
        }
        for index in 0..28 {
            if !self.fixed.contains_key(&(0x8224_9F90 + index * 4)) {
                return Err("component tuning lacks the owned grind identity table".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<u8> {
        let mut b = b"SVAT".to_vec();
        for v in [1u32, 1, 1, 0] { b.extend(v.to_be_bytes()); }
        b.extend(0x100u64.to_be_bytes()); b.extend(0x200u64.to_be_bytes());
        b.extend(1u32.to_be_bytes()); b.extend(0x300u64.to_be_bytes());
        b.extend(8u32.to_be_bytes()); b.extend(2u32.to_be_bytes());
        for v in [0x11223344u32, 0x01000000, 0x55667788, 0x00000007] { b.extend(v.to_be_bytes()); }
        b.extend(64u32.to_be_bytes()); b.extend(0x100u64.to_be_bytes()); b.extend(0x200u64.to_be_bytes());
        b
    }
    #[test]
    fn imported_arrays_keep_endianness_stride_and_missing_semantics() {
        let tuning = Tuning::parse(&fixture()).unwrap();
        let handle = tuning.collection(0x100, 0x200);
        assert_ne!(handle, 0);
        assert_eq!(tuning.g32(tuning.attrib(handle, 0x300)), 0x11223344);
        let second = tuning.tuning_at(64, 0x300, 1);
        assert_eq!(tuning.g32(second), 0x55667788);
        assert_eq!(tuning.g8(tuning.tuning_at(64, 0x300, 0) + 4), 1);
        assert_eq!(tuning.g32(second + 4), 7);
        assert_eq!(tuning.tuning_at(64, 0x300, 2), DEFAULT);
        assert_eq!(tuning.g32(DEFAULT), 0);
        assert!(tuning.required_word(64, 0x301, 0).is_err());
    }
    #[test]
    fn rejects_truncation_dimensions_and_broken_bindings() {
        let b = fixture();
        for end in 0..b.len() { assert!(Tuning::parse(&b[..end]).is_err(), "length {end}"); }
        let mut bad = b.clone(); bad[48..52].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(Tuning::parse(&bad).is_err());
        let mut bad = b.clone(); bad.extend([0]); assert!(Tuning::parse(&bad).is_err());
        let mut bad = b.clone(); *bad.last_mut().unwrap() = 3; assert!(Tuning::parse(&bad).is_err());
    }
}

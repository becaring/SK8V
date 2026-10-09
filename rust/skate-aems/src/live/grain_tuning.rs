//! User-owned surface curves imported by `tools/prepare-audio-tuning.py`.
use std::collections::HashMap;
use crate::eac::grain::{Curve, GrainParams};

pub struct Surface {
    pub recording: String,
    pub curve: Curve,
    pub players: [GrainParams; 2],
    pub scalars: HashMap<u64, u32>,
}

pub struct GrainTuning {
    pub surfaces: HashMap<u64, Surface>,
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n { return Err("truncated grain tuning".into()); }
        let (v, rest) = self.0.split_at(n); self.0 = rest; Ok(v)
    }
    fn u16(&mut self) -> Result<u16, String> { Ok(u16::from_be_bytes(self.bytes(2)?.try_into().unwrap())) }
    fn u32(&mut self) -> Result<u32, String> { Ok(u32::from_be_bytes(self.bytes(4)?.try_into().unwrap())) }
    fn u64(&mut self) -> Result<u64, String> { Ok(u64::from_be_bytes(self.bytes(8)?.try_into().unwrap())) }
    fn float(&mut self) -> Result<f32, String> {
        let v = f32::from_bits(self.u32()?);
        if !v.is_finite() { return Err("nonfinite grain tuning".into()); } Ok(v)
    }
    fn player(&mut self) -> Result<GrainParams, String> {
        let p = GrainParams { fade_in: self.float()?, hold: self.float()?, fade_out: self.float()?, window: self.float()?, tolerance: self.float()? };
        if p.fade_in < 0.0 || p.hold <= 0.0 || p.fade_out < 0.0 || p.window <= 0.0 || p.tolerance < 0.0 {
            return Err("invalid grain timing".into());
        }
        Ok(p)
    }
}

impl GrainTuning {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut r = Reader(bytes);
        if r.bytes(4)? != b"SVGT" || r.u32()? != 1 { return Err("unsupported grain tuning".into()); }
        let count = r.u32()?;
        if count == 0 || count > 256 { return Err("invalid grain surface count".into()); }
        let mut surfaces = HashMap::new();
        for _ in 0..count {
            let key = r.u64()?;
            let name_len = r.u16()? as usize;
            let scalar_count = r.u16()?;
            let name = r.bytes(name_len)?;
            if name.is_empty() || !name.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_') {
                return Err("invalid grain recording name".into());
            }
            let recording = String::from_utf8(name.to_vec()).unwrap();
            let mut f = [0.0; 23];
            for v in &mut f { *v = r.float()?; }
            let curve = Curve::from_floats(&f);
            if curve.top_kmh <= 0.0 { return Err("invalid grain curve speed".into()); }
            let players = [r.player()?, r.player()?];
            let mut scalars = HashMap::new();
            for _ in 0..scalar_count {
                if scalars.insert(r.u64()?, r.u32()?).is_some() { return Err("duplicate grain scalar".into()); }
            }
            if surfaces.insert(key, Surface { recording, curve, players, scalars }).is_some() {
                return Err("duplicate grain surface".into());
            }
        }
        if !r.0.is_empty() { return Err("trailing grain tuning bytes".into()); }
        Ok(Self { surfaces })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_record_cannot_escape_or_allocate_from_untrusted_count() {
        for size in 0..12 { assert!(GrainTuning::parse(&vec![0; size]).is_err()); }
        let mut b = b"SVGT".to_vec();
        b.extend(1u32.to_be_bytes());
        b.extend(u32::MAX.to_be_bytes());
        assert!(GrainTuning::parse(&b).err().unwrap().contains("count"));
        b[8..12].copy_from_slice(&1u32.to_be_bytes());
        b.extend(1u64.to_be_bytes());
        b.extend(3u16.to_be_bytes());
        b.extend(0u16.to_be_bytes());
        b.extend(b"../");
        assert!(GrainTuning::parse(&b).err().unwrap().contains("name"));
    }
}

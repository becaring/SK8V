//! What the class handlers read from the game's memory beyond their own
//! component and skater state: AttribSys tuned values, collections, and a
//! few game structures reached through pointers (controller gates). The
//! live engine models these as a small synthetic big-endian guest memory:
//! every value a handler looks up gets an address there.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use super::grain_tuning::GrainTuning;

mod cache;

/// AttribSys's "missing" default (`0x830D0850`, reads 0.0).
pub const DEFAULT: u32 = 0x830D_0850;
/// Synthetic guest memory base.
const BASE: u32 = 0xE000_0000;

/// A source record/array in the owned cache. Its bytes retain native endianness.
#[derive(Clone, Copy, Debug)]
pub struct Record {
    pub address: u32,
    pub stride: u32,
    pub count: u32,
}

#[derive(Default, Clone)]
pub struct Tuning {
    bytes: Vec<u8>,
    /// Fixed guest addresses the handlers read directly (e.g. `0x830CFDC4`).
    fixed: HashMap<u32, u32>,
    /// `(root, key, index)` → address.
    tuned: HashMap<(u32, u64, u32), u32>,
    /// `(collection, key)` → address.
    attribs: HashMap<(u32, u64), u32>,
    /// `(class key, collection key)` → collection handle.
    collections: HashMap<(u64, u64), u32>,
    records: HashMap<(u32, u64), Record>,
    roots: HashMap<u32, u32>,
    layouts: HashMap<u32,u32>,
    grain: Option<Arc<GrainTuning>>,
}

impl Tuning {
    /// Production data only: no heap or executable is read
    /// by the running audio engine. The offline importer produces both files.
    pub fn load(cache_dir: &Path) -> Result<Self, String> {
        let path = cache_dir.join("component-tuning.bin");
        let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut tuning = Self::parse(&data)?;
        tuning.validate_live_roots()?;
        let path = cache_dir.join("grain-tuning.bin");
        let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        tuning.grain = Some(Arc::new(GrainTuning::parse(&data)?));
        tuning.build_layouts()?;
        Ok(tuning)
    }

    pub fn grain(&self) -> Option<&GrainTuning> { self.grain.as_deref() }

    /// Native Attrib layout view for handlers which directly load typed fields.
    pub fn collection_layout(&self,collection:u32)->u32 {
        self.layouts.get(&collection).copied().unwrap_or(DEFAULT)
    }

    fn build_layouts(&mut self)->Result<(),String> {
        // 7242F32831ED3332: all five offsets are the attrib::value address
        // minus collection+36, the same in all 16 seam collections.
        let fields=[0x1991_70BB_1C52_EE64,0xD18F_436B_5764_F260,0x534B_0A71_9762_E2E8,
            0x107A_78BA_11A2_B813,0x5FAD_918A_2DE5_459A];
        let handles:Vec<_>=self.collection_entries().filter_map(|((class,_),handle)|
            (class==0x7242_F328_31ED_3332).then_some(handle)).collect();
        for handle in handles {
            let mut words=[0;5];
            for (i,key) in fields.into_iter().enumerate() {
                let r=self.record(handle,key).ok_or("seam typed-layout field absent")?;
                words[i]=self.g32(r.address);
            }
            let address=self.alloc(20);
            for (i,word) in words.into_iter().enumerate(){self.w32(address+4*i as u32,word);}
            self.layouts.insert(handle,address);
        }
        Ok(())
    }

    pub fn root_collection(&self, root: u32) -> Option<u32> { self.roots.get(&root).copied() }

    pub fn record(&self, collection: u32, key: u64) -> Option<Record> {
        self.records.get(&(collection, key)).copied()
    }

    /// Imported collection identities and their handles.
    pub fn collection_entries(&self) -> impl Iterator<Item = ((u64, u64), u32)> + '_ {
        self.collections.iter().map(|(&identity, &handle)| (identity, handle))
    }

    pub fn attrib_at(&self, collection: u32, key: u64, index: u32) -> u32 {
        self.record(collection, key).filter(|r| index < r.count)
            .map(|r| r.address + r.stride * index).unwrap_or(DEFAULT)
    }

    pub fn root_record(&self, root: u32, key: u64) -> Option<Record> {
        self.root_collection(root).and_then(|c| self.record(c, key))
    }

    /// Constructors may request a required source word without interpreting a
    /// missing lookup's native zero default as valid tuning.
    pub fn required_word(&self, root: u32, key: u64, index: u32) -> Result<u32, String> {
        let record = self.root_record(root, key)
            .filter(|r| r.stride >= 4 && index < r.count)
            .ok_or_else(|| format!("missing audio tuning root {root} key {key:016x} index {index}"))?;
        Ok(self.g32(record.address + record.stride * index))
    }

    /// Reserves `n` bytes of synthetic memory, zeroed; returns the address.
    pub fn alloc(&mut self, n: usize) -> u32 {
        let at = BASE + self.bytes.len() as u32;
        self.bytes.resize(self.bytes.len() + n.div_ceil(4) * 4, 0);
        at
    }
    pub fn w32(&mut self, addr: u32, v: u32) {
        if let Some(o) = addr.checked_sub(BASE).map(|o| o as usize).filter(|o| o + 4 <= self.bytes.len()) {
            self.bytes[o..o + 4].copy_from_slice(&v.to_be_bytes());
        } else {
            self.fixed.insert(addr, v);
        }
    }
    pub fn w8(&mut self, addr: u32, v: u8) {
        if let Some(o) = addr.checked_sub(BASE).map(|o| o as usize).filter(|o| *o < self.bytes.len()) {
            self.bytes[o] = v;
        }
    }
    pub fn g8(&self, addr: u32) -> u8 {
        match addr.checked_sub(BASE).map(|o| o as usize) {
            Some(o) if o < self.bytes.len() => self.bytes[o],
            _ => (self.g32(addr & !3) >> ((3 - (addr & 3)) * 8)) as u8,
        }
    }
    pub fn g32(&self, addr: u32) -> u32 {
        match addr.checked_sub(BASE).map(|o| o as usize) {
            Some(o) if o + 4 <= self.bytes.len() => u32::from_be_bytes(self.bytes[o..o + 4].try_into().unwrap()),
            _ => self.fixed.get(&addr).copied().unwrap_or(0),
        }
    }

    /// A tuned value (`f32` or `i32` bits) at `(root, key, index)`.
    #[cfg(test)]
    pub fn set_tuned(&mut self, root: u32, key: u64, index: u32, bits: u32) {
        let at = self.alloc(4);
        self.w32(at, bits);
        self.tuned.insert((root, key, index), at);
    }
    /// A tuned value that is a pointer-sized record: returns its address so
    /// the caller can fill fields (e.g. the surface table entries).
    #[cfg(test)]
    pub fn set_tuned_record(&mut self, root: u32, key: u64, index: u32, size: usize) -> u32 {
        let at = self.alloc(size);
        self.tuned.insert((root, key, index), at);
        at
    }
    #[cfg(test)]
    pub fn set_attrib(&mut self, collection: u32, key: u64, bits: u32) {
        let at = self.alloc(4);
        self.w32(at, bits);
        self.attribs.insert((collection, key), at);
    }
    pub fn set_collection(&mut self, class_key: u64, key: u64) -> u32 {
        let c = self.alloc(4);
        self.collections.insert((class_key, key), c);
        c
    }

    pub fn tuning_at(&self, root: u32, key: u64, index: u32) -> u32 {
        self.tuned.get(&(root, key, index)).copied().unwrap_or_else(|| {
            self.root_collection(root).map(|c| self.attrib_at(c, key, index)).unwrap_or(DEFAULT)
        })
    }
    pub fn attrib(&self, collection: u32, key: u64) -> u32 {
        self.attribs.get(&(collection, key)).copied().unwrap_or_else(|| self.attrib_at(collection, key, 0))
    }
    pub fn collection(&self, class_key: u64, key: u64) -> u32 {
        self.collections.get(&(class_key, key)).copied().unwrap_or(0)
    }
}

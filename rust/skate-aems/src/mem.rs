//! A 32-bit, big-endian address space mirroring the game's memory layout.
//!
//! AEMS patches address instance memory by byte offset and keep pointers to
//! bank tables, symbol objects and each other; reproducing the layout exactly
//! lets every handler be a literal translation of the PowerPC. Regions are
//! allocated at fixed virtual bases per kind; function pointers stored in
//! memory keep the game's own handler addresses and dispatch by value.

#[derive(Default)]
pub struct Memory {
    regions: Vec<Region>, // sorted by base
    next: [u32; 4],
    /// Freed object and instance blocks by rounded size, reused so the bump
    /// allocator does not exhaust its window in a long live session.
    holes: std::collections::HashMap<(usize, u32), Vec<u32>>,
}

struct Region {
    base: u32,
    bytes: Vec<u8>,
}

/// Virtual bases per allocation kind (each a 256 MB window).
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Bank = 0,
    Symbol = 1,
    Object = 2,
    Instance = 3,
}

const BASES: [u32; 4] = [0x1000_0000, 0x2000_0000, 0x3000_0000, 0x4000_0000];
const WINDOW: u32 = 0x1000_0000;
const ALIGN: u32 = 16;

impl Memory {
    /// Allocates `bytes` (copied in) and returns its address.
    pub fn alloc(&mut self, kind: Kind, bytes: Vec<u8>) -> u32 {
        let k = kind as usize;
        let size = (bytes.len() as u32).max(1);
        let rounded = size.div_ceil(ALIGN) * ALIGN;
        if k >= 2
            && let Some(base) = self.holes.get_mut(&(k, rounded)).and_then(|v| v.pop())
        {
            let at = self.regions.partition_point(|r| r.base < base);
            self.regions.insert(at, Region { base, bytes });
            return base;
        }
        let base = BASES[k] + self.next[k];
        self.next[k] += rounded;
        assert!(self.next[k] < WINDOW, "AEMS memory window exhausted");
        let at = self.regions.partition_point(|r| r.base < base);
        self.regions.insert(at, Region { base, bytes });
        base
    }

    pub fn alloc_zeroed(&mut self, kind: Kind, size: usize) -> u32 {
        self.alloc(kind, vec![0; size])
    }

    pub fn free(&mut self, addr: u32) {
        let i = self.regions.partition_point(|r| r.base < addr);
        if self.regions.get(i).is_some_and(|r| r.base == addr) {
            let r = self.regions.remove(i);
            let k = ((addr - BASES[0]) / WINDOW) as usize;
            if k >= 2 {
                let rounded = (r.bytes.len() as u32).max(1).div_ceil(ALIGN) * ALIGN;
                self.holes.entry((k, rounded)).or_default().push(addr);
            }
        }
    }

    fn find(&self, addr: u32, len: u32) -> Option<(usize, usize)> {
        let i = self.regions.partition_point(|r| r.base <= addr);
        if i == 0 {
            return None;
        }
        let r = &self.regions[i - 1];
        let off = (addr - r.base) as usize;
        (off + len as usize <= r.bytes.len()).then_some((i - 1, off))
    }

    /// Base address of the allocation containing `addr` (port convenience).
    pub fn region_base(&self, addr: u32) -> Option<u32> {
        self.find(addr, 1).map(|(i, _)| self.regions[i].base)
    }

    pub fn valid(&self, addr: u32, len: u32) -> bool {
        self.find(addr, len).is_some()
    }

    fn at(&self, addr: u32, len: u32) -> &[u8] {
        let (i, off) = self.find(addr, len).unwrap_or_else(|| panic!("AEMS read outside memory at {addr:#010x}"));
        &self.regions[i].bytes[off..off + len as usize]
    }

    fn at_mut(&mut self, addr: u32, len: u32) -> &mut [u8] {
        let (i, off) = self.find(addr, len).unwrap_or_else(|| panic!("AEMS write outside memory at {addr:#010x}"));
        &mut self.regions[i].bytes[off..off + len as usize]
    }

    pub fn r8(&self, a: u32) -> u8 {
        self.at(a, 1)[0]
    }
    pub fn r16(&self, a: u32) -> u16 {
        let b = self.at(a, 2);
        u16::from_be_bytes([b[0], b[1]])
    }
    pub fn r32(&self, a: u32) -> u32 {
        let b = self.at(a, 4);
        u32::from_be_bytes([b[0], b[1], b[2], b[3]])
    }
    pub fn ri32(&self, a: u32) -> i32 {
        self.r32(a) as i32
    }
    pub fn rf32(&self, a: u32) -> f32 {
        f32::from_bits(self.r32(a))
    }
    pub fn w8(&mut self, a: u32, v: u8) {
        self.at_mut(a, 1)[0] = v;
    }
    pub fn w16(&mut self, a: u32, v: u16) {
        self.at_mut(a, 2).copy_from_slice(&v.to_be_bytes());
    }
    pub fn w32(&mut self, a: u32, v: u32) {
        self.at_mut(a, 4).copy_from_slice(&v.to_be_bytes());
    }
    pub fn wi32(&mut self, a: u32, v: i32) {
        self.w32(a, v as u32);
    }
    pub fn wf32(&mut self, a: u32, v: f32) {
        self.w32(a, v.to_bits());
    }
    pub fn bytes(&self, a: u32, len: u32) -> &[u8] {
        self.at(a, len)
    }
    /// Doubly linked list push at a head word (`+0 next`, `+4 prev` nodes),
    /// the pattern the game uses for every listener list.
    pub fn list_push(&mut self, head: u32, node: u32) {
        let first = self.r32(head);
        self.w32(node, first);
        self.w32(node + 4, 0);
        if first != 0 {
            self.w32(first + 4, node);
        }
        self.w32(head, node);
    }

    /// Unlink `node` from the list at `head` (game pattern in op 4 /
    /// `0x82B1C150`).
    pub fn list_remove(&mut self, head: u32, node: u32) {
        if self.r32(head) == node {
            let next = self.r32(node);
            self.w32(head, next);
        }
        let prev = self.r32(node + 4);
        if prev != 0 {
            let next = self.r32(node);
            self.w32(prev, next);
        }
        let next = self.r32(node);
        if next != 0 {
            let prev = self.r32(node + 4);
            self.w32(next + 4, prev);
        }
    }
}

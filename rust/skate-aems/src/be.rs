//! Big-endian reads from byte slices (the game's data is PowerPC big-endian).

pub fn u16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

pub fn u32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// `u16` at `o`, or `None` past the end (files that may be truncated).
pub fn get_u16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?))
}

/// `u32` at `o`, or `None` past the end.
pub fn get_u32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

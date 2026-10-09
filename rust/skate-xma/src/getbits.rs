// Port of FFmpeg 4.4 libavcodec/get_bits.h (the non-cached, big-endian,
// CONFIG_SAFE_BITSTREAM_READER=1 reader the oracle build uses).
// LGPL-2.1-or-later, see lib.rs.

/// `GetBitContext` over a borrowed buffer. Reads past `size_in_bits` behave
/// like FFmpeg's safe reader: the index saturates at `size_in_bits + 8` and
/// the cache is filled from whatever bytes follow in the buffer (callers
/// must pass the same trailing bytes FFmpeg would see).
pub struct GetBits<'a> {
    buf: &'a [u8],
    pub index: u32,
    pub size_in_bits: i32,
    size_plus8: u32,
}

#[inline(always)]
fn rb32(buf: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

pub type VlcTable = [[i16; 2]];

impl<'a> GetBits<'a> {
    /// init_get_bits
    pub fn new(buf: &'a [u8], bit_size: i32) -> Self {
        GetBits { buf, index: 0, size_in_bits: bit_size, size_plus8: (bit_size + 8) as u32 }
    }

    #[inline(always)]
    fn cache(&self, index: u32) -> u32 {
        rb32(self.buf, (index >> 3) as usize) << (index & 7)
    }

    #[inline(always)]
    fn skip_counter(&self, index: u32, num: i32) -> u32 {
        // name##_index = FFMIN(size_plus8, name##_index + (num)), unsigned
        let v = index.wrapping_add(num as u32);
        v.min(self.size_plus8)
    }

    #[inline(always)]
    pub fn get_bits_count(&self) -> i32 {
        self.index as i32
    }

    #[inline(always)]
    pub fn get_bits_left(&self) -> i32 {
        self.size_in_bits - self.get_bits_count()
    }

    /// Read 1-25 bits.
    #[inline(always)]
    pub fn get_bits(&mut self, n: u32) -> u32 {
        let cache = self.cache(self.index);
        let tmp = cache >> (32 - n);
        self.index = self.skip_counter(self.index, n as i32);
        tmp
    }

    #[inline(always)]
    pub fn get_bitsz(&mut self, n: u32) -> u32 {
        if n != 0 { self.get_bits(n) } else { 0 }
    }

    #[inline(always)]
    pub fn get_sbits(&mut self, n: u32) -> i32 {
        let cache = self.cache(self.index);
        let tmp = (cache as i32) >> (32 - n);
        self.index = self.skip_counter(self.index, n as i32);
        tmp
    }

    #[inline(always)]
    pub fn get_bits1(&mut self) -> u32 {
        let index = self.index;
        let mut result = self.buf[(index >> 3) as usize] as u32;
        result <<= index & 7;
        result = (result as u8 as u32) >> 7;
        if self.index < self.size_plus8 {
            self.index = index + 1;
        }
        result
    }

    #[inline(always)]
    pub fn show_bits(&self, n: u32) -> u32 {
        self.cache(self.index) >> (32 - n)
    }

    #[inline(always)]
    pub fn skip_bits(&mut self, n: i32) {
        self.index = self.skip_counter(self.index, n);
    }

    #[inline(always)]
    pub fn skip_bits_long(&mut self, n: i32) {
        let idx = self.index as i32;
        let lo = -idx;
        let hi = self.size_plus8 as i32 - idx;
        let c = if n < lo { lo } else if n > hi { hi } else { n };
        self.index = (idx + c) as u32;
    }

    /// Read 0-32 bits.
    #[inline(always)]
    pub fn get_bits_long(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else if n <= 25 {
            self.get_bits(n)
        } else {
            let ret = self.get_bits(16) << (n - 16);
            ret | self.get_bits(n - 16)
        }
    }

    /// get_vlc2 (GET_VLC with OPEN_READER/UPDATE_CACHE/CLOSE_READER).
    #[inline(always)]
    pub fn get_vlc2(&mut self, table: &VlcTable, bits: u32, max_depth: u32) -> i32 {
        let mut idx = self.index;
        let mut cache = self.cache(idx);
        let index = (cache >> (32 - bits)) as usize;
        let mut code = table[index][0] as i32;
        let mut n = table[index][1] as i32;
        if max_depth > 1 && n < 0 {
            idx = self.skip_counter(idx, bits as i32);
            cache = self.cache(idx);
            let nb_bits = (-n) as u32;
            let index = ((cache >> (32 - nb_bits)) as i32 + code) as usize;
            code = table[index][0] as i32;
            n = table[index][1] as i32;
            if max_depth > 2 && n < 0 {
                idx = self.skip_counter(idx, nb_bits as i32);
                cache = self.cache(idx);
                let nb_bits = (-n) as u32;
                let index = ((cache >> (32 - nb_bits)) as i32 + code) as usize;
                code = table[index][0] as i32;
                n = table[index][1] as i32;
            }
        }
        // SKIP_BITS(name, gb, n): the cache shift is irrelevant after this.
        idx = self.skip_counter(idx, n);
        self.index = idx;
        code
    }
}

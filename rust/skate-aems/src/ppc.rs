//! PowerPC float-to-int conversions and the VMX128 lane operations the live
//! audio path uses, with the recomp SDK's semantics (`rex/ppc/context.h`).
//! The register file, condition fields and guest memory interface that
//! `tools/recomp2rs.py` output needs sit behind the `translated` feature.
//!
//! Vector registers keep the SDK's host layout: the 16 guest bytes reversed,
//! so PowerPC element `i` is host lane `3 - i`. The `vmx` helpers follow the
//! SDK's SIMDe lowering operation by operation (built without hardware FMA:
//! `vmaddfp` is `a * b + c` in single precision, rounded twice). Run under
//! MXCSR flush-to-zero / denormals-are-zero, as the recomp does in both its
//! scalar and vector modes.

#[cfg(feature = "translated")]
#[derive(Clone, Copy, Debug, Default)]
pub struct Cr {
    pub lt: bool,
    pub gt: bool,
    pub eq: bool,
    pub so: bool,
}

#[cfg(feature = "translated")]
impl Cr {
    pub fn cmp_i(&mut self, a: i64, b: i64, so: bool) {
        self.lt = a < b;
        self.gt = a > b;
        self.eq = a == b;
        self.so = so;
    }
    pub fn cmp_u(&mut self, a: u64, b: u64, so: bool) {
        self.lt = a < b;
        self.gt = a > b;
        self.eq = a == b;
        self.so = so;
    }
    /// `compare(double, double)`: unordered clears lt/gt/eq and sets so.
    pub fn cmp_f(&mut self, a: f64, b: f64) {
        let un = a.is_nan() || b.is_nan();
        self.lt = !un && a < b;
        self.gt = !un && a > b;
        self.eq = !un && a == b;
        self.so = un;
    }
}

/// General and float registers (floats as raw f64 bits, as the SDK's union
/// stores integer conversions in them), vector registers, condition fields,
/// CTR, LR, XER bits.
#[cfg(feature = "translated")]
#[derive(Clone, Debug)]
pub struct Ctx {
    pub r: [u64; 32],
    pub f: [u64; 32],
    pub v: [vmx::V128; 128],
    /// The SDK's scratch `PPCVRegister vTemp`.
    pub vt: vmx::V128,
    pub cr: [Cr; 8],
    pub ctr: u64,
    pub lr: u64,
    pub ca: bool,
    pub so: bool,
}

#[cfg(feature = "translated")]
impl Default for Ctx {
    fn default() -> Ctx {
        Ctx {
            r: [0; 32],
            f: [0; 32],
            v: [vmx::V128::default(); 128],
            vt: vmx::V128::default(),
            cr: [Cr::default(); 8],
            ctr: 0,
            lr: 0,
            ca: false,
            so: false,
        }
    }
}

/// The SDK's scratch `PPCRegister temp` (a union over 8 bytes).
#[cfg(feature = "translated")]
#[derive(Clone, Copy, Debug, Default)]
pub struct Temp(pub u64);

#[cfg(feature = "translated")]
impl Temp {
    pub fn u8(&self) -> u8 {
        self.0 as u8
    }
    pub fn u32(&self) -> u32 {
        self.0 as u32
    }
    pub fn f32(&self) -> f32 {
        f32::from_bits(self.0 as u32)
    }
    pub fn set_u8(&mut self, v: u8) {
        self.0 = (self.0 & !0xFF) | v as u64;
    }
    pub fn set_u32(&mut self, v: u32) {
        self.0 = (self.0 & !0xFFFF_FFFF) | v as u64;
    }
    pub fn set_f32(&mut self, v: f32) {
        self.set_u32(v.to_bits());
    }
}

/// `fctidz` / `fctid` as the SDK emits them: NaN gives `i64::MIN`, above
/// 2^63 `i64::MAX`, otherwise `cvttsd2si` / `cvtsd2si` (nearest even), which
/// give `i64::MIN` out of range.
pub fn fctidz(x: f64) -> i64 {
    cvt64(x, x.trunc())
}

pub fn fctid(x: f64) -> i64 {
    cvt64(x, x.round_ties_even())
}

fn cvt64(x: f64, r: f64) -> i64 {
    const TWO63: f64 = 9_223_372_036_854_775_808.0;
    if x.is_nan() {
        i64::MIN
    } else if x > TWO63 {
        i64::MAX
    } else if r >= TWO63 || r < -TWO63 {
        i64::MIN
    } else {
        r as i64
    }
}

/// `fctiwz` as the SDK emits it: NaN → 0x80000000 (as a positive i64),
/// above `i32::MAX` → `i32::MAX`, otherwise truncation (cvttsd2si, which
/// gives `i32::MIN` below range).
pub fn fctiwz(x: f64) -> i64 {
    if x.is_nan() {
        0x8000_0000
    } else if x > i32::MAX as f64 {
        i32::MAX as i64
    } else if x < i32::MIN as f64 {
        i32::MIN as i64
    } else {
        x as i32 as i64
    }
}

/// Big-endian guest memory and calls out of the translated code.
#[cfg(feature = "translated")]
pub trait Mem {
    fn ld_u8(&self, a: u32) -> u8;
    fn st_u8(&mut self, a: u32, v: u8);
    fn ld_u16(&self, a: u32) -> u16 {
        u16::from_be_bytes([self.ld_u8(a), self.ld_u8(a.wrapping_add(1))])
    }
    fn ld_u32(&self, a: u32) -> u32 {
        (self.ld_u16(a) as u32) << 16 | self.ld_u16(a.wrapping_add(2)) as u32
    }
    fn ld_u64(&self, a: u32) -> u64 {
        (self.ld_u32(a) as u64) << 32 | self.ld_u32(a.wrapping_add(4)) as u64
    }
    fn st_u16(&mut self, a: u32, v: u16) {
        let b = v.to_be_bytes();
        self.st_u8(a, b[0]);
        self.st_u8(a.wrapping_add(1), b[1]);
    }
    fn st_u32(&mut self, a: u32, v: u32) {
        self.st_u16(a, (v >> 16) as u16);
        self.st_u16(a.wrapping_add(2), v as u16);
    }
    fn st_u64(&mut self, a: u32, v: u64) {
        self.st_u32(a, (v >> 32) as u32);
        self.st_u32(a.wrapping_add(4), v as u32);
    }
    /// Trace hooks around every translated function (default: nothing).
    fn enter(&mut self, addr: u32, c: &Ctx) {
        let _ = (addr, c);
    }
    fn leave(&mut self, addr: u32, c: &Ctx) {
        let _ = (addr, c);
    }
    /// A call to a function that was not translated.
    fn call(&mut self, addr: u32, c: &mut Ctx) {
        let _ = c;
        panic!("call to untranslated guest function {addr:#010x}");
    }
}

/// Vector (VMX128) operations on the SDK's host layout.
pub mod vmx {
    #[cfg(feature = "translated")]
    use super::Mem;

    /// One vector register: the guest's 16 bytes in reverse order (host
    /// little-endian lanes; lane `i` is PowerPC element `3 - i`).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct V128(pub [u8; 16]);

    impl V128 {
        pub fn u32(&self, i: usize) -> u32 {
            u32::from_le_bytes(self.0[4 * i..4 * i + 4].try_into().unwrap())
        }
        pub fn set_u32(&mut self, i: usize, v: u32) {
            self.0[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        pub fn f32(&self, i: usize) -> f32 {
            f32::from_bits(self.u32(i))
        }
#[cfg(feature = "translated")]
        pub fn s16(&self, i: usize) -> i16 {
            i16::from_le_bytes([self.0[2 * i], self.0[2 * i + 1]])
        }
        fn map_u32(a: V128, f: impl Fn(u32, usize) -> u32) -> V128 {
            let mut r = V128::default();
            for i in 0..4 {
                r.set_u32(i, f(a.u32(i), i));
            }
            r
        }
        fn zip_f32(a: V128, b: V128, f: impl Fn(f32, f32) -> f32) -> V128 {
            let mut r = V128::default();
            for i in 0..4 {
                r.set_u32(i, f(a.f32(i), b.f32(i)).to_bits());
            }
            r
        }
    }

    // SDK tables (`rex/ppc/intrinsics.h`) as functions of row and byte.
#[cfg(feature = "translated")]
    fn mask_l(k: usize, j: usize) -> u8 {
        if j < k { 0xFF } else { (15 - (j - k)) as u8 }
    }
#[cfg(feature = "translated")]
    fn mask_r(k: usize, j: usize) -> u8 {
        if j < k { (k - 1 - j) as u8 } else { 0xFF }
    }
#[cfg(feature = "translated")]
    fn shift_l(k: usize, j: usize) -> u8 {
        (15 - j + k) as u8
    }

#[cfg(feature = "translated")]
    fn raw(m: &dyn Mem, ea: u32) -> [u8; 16] {
        std::array::from_fn(|i| m.ld_u8(ea.wrapping_add(i as u32)))
    }
    /// `simde_mm_shuffle_epi8`: a mask byte with the top bit set gives 0.
#[cfg(feature = "translated")]
    pub fn shuffle_epi8(src: [u8; 16], mask: impl Fn(usize) -> u8) -> V128 {
        V128(std::array::from_fn(|i| {
            let k = mask(i);
            if k & 0x80 != 0 { 0 } else { src[(k & 15) as usize] }
        }))
    }
    /// `lvx` (address already aligned by the caller).
#[cfg(feature = "translated")]
    pub fn load(m: &dyn Mem, ea: u32) -> V128 {
        shuffle_epi8(raw(m, ea), |j| mask_l(0, j))
    }
    /// `stvx` (address already aligned by the caller).
#[cfg(feature = "translated")]
    pub fn store(m: &mut dyn Mem, ea: u32, v: V128) {
        for i in 0..16 {
            m.st_u8(ea.wrapping_add(i as u32), v.0[15 - i]);
        }
    }
    /// `lvlx`.
#[cfg(feature = "translated")]
    pub fn load_left(m: &dyn Mem, a: u32) -> V128 {
        let k = (a & 15) as usize;
        shuffle_epi8(raw(m, a & !15), |j| mask_l(k, j))
    }
    /// `lvrx`.
#[cfg(feature = "translated")]
    pub fn load_right(m: &dyn Mem, a: u32) -> V128 {
        let k = (a & 15) as usize;
        if k == 0 { V128::default() } else { shuffle_epi8(raw(m, a & !15), |j| mask_r(k, j)) }
    }
    /// `lvsl`.
#[cfg(feature = "translated")]
    pub fn lvsl(a: u32) -> V128 {
        let k = (a & 15) as usize;
        V128(std::array::from_fn(|j| shift_l(k, j)))
    }

#[cfg(feature = "translated")]
    pub fn and(a: V128, b: V128) -> V128 {
        V128(std::array::from_fn(|i| a.0[i] & b.0[i]))
    }
#[cfg(feature = "translated")]
    pub fn or(a: V128, b: V128) -> V128 {
        V128(std::array::from_fn(|i| a.0[i] | b.0[i]))
    }
    pub fn xor(a: V128, b: V128) -> V128 {
        V128(std::array::from_fn(|i| a.0[i] ^ b.0[i]))
    }
    /// `vsel`: `or(andnot(mask, a), and(mask, b))`.
    pub fn sel(mask: V128, a: V128, b: V128) -> V128 {
        V128(std::array::from_fn(|i| (!mask.0[i] & a.0[i]) | (mask.0[i] & b.0[i])))
    }
    pub fn set1_u32(v: u32) -> V128 {
        V128::map_u32(V128::default(), |_, _| v)
    }
#[cfg(feature = "translated")]
    pub fn set1_u8(v: u8) -> V128 {
        V128([v; 16])
    }
#[cfg(feature = "translated")]
    pub fn add_u32(a: V128, b: V128) -> V128 {
        V128::map_u32(a, |x, i| x.wrapping_add(b.u32(i)))
    }
    /// `simde_mm_shuffle_epi32`.
#[cfg(feature = "translated")]
    pub fn shuffle_u32(a: V128, imm: u32) -> V128 {
        V128::map_u32(a, |_, i| a.u32(((imm >> (2 * i)) & 3) as usize))
    }
    /// `simde_mm_perm_epi8_` (`vperm`): byte `c & 15` of `a`, or of `b`
    /// when `c & 16`, in guest order.
#[cfg(feature = "translated")]
    pub fn perm(a: V128, b: V128, c: V128) -> V128 {
        V128(std::array::from_fn(|j| {
            let e = 15 - (c.0[j] & 15) as usize;
            if c.0[j] & 0x10 != 0 { b.0[e] } else { a.0[e] }
        }))
    }
    /// `simde_mm_sllv_epi8` (counts masked to 0..7 by the caller).
#[cfg(feature = "translated")]
    pub fn sllv_u8(a: V128, s: V128) -> V128 {
        V128(std::array::from_fn(|i| ((a.0[i] as u16) << (s.0[i] & 15)) as u8))
    }

    /// `simde_mm_cvtepi16_epi32` of host i16 lanes `from..from + 4`
    /// (`unpackhi_epi64(b, b)` first for `from = 4`).
#[cfg(feature = "translated")]
    pub fn unpack_i16(a: V128, from: usize) -> V128 {
        V128::map_u32(a, |_, i| a.s16(from + i) as i32 as u32)
    }

    pub fn add(a: V128, b: V128) -> V128 {
        V128::zip_f32(a, b, |x, y| x + y)
    }
    pub fn sub(a: V128, b: V128) -> V128 {
        V128::zip_f32(a, b, |x, y| x - y)
    }
    pub fn mul(a: V128, b: V128) -> V128 {
        V128::zip_f32(a, b, |x, y| x * y)
    }
    /// `simde_mm_fmadd_ps` without hardware FMA: `a * b + c`.
    pub fn madd(a: V128, b: V128, c: V128) -> V128 {
        let mut r = V128::default();
        for i in 0..4 {
            r.set_u32(i, (a.f32(i) * b.f32(i) + c.f32(i)).to_bits());
        }
        r
    }
    /// `simde_mm_fnmadd_ps` without hardware FMA: `-(a * b) + c`.
    pub fn nmadd(a: V128, b: V128, c: V128) -> V128 {
        let mut r = V128::default();
        for i in 0..4 {
            r.set_u32(i, (-(a.f32(i) * b.f32(i)) + c.f32(i)).to_bits());
        }
        r
    }
    pub fn cmpeq(a: V128, b: V128) -> V128 {
        V128::map_u32(a, |_, i| if a.f32(i) == b.f32(i) { u32::MAX } else { 0 })
    }
    /// `simde_mm_cvtepi32_ps`.
#[cfg(feature = "translated")]
    pub fn cvt_i32(a: V128) -> V128 {
        V128::map_u32(a, |x, _| (x as i32 as f32).to_bits())
    }
    /// `simde_mm_cvtepu32_ps_` (the SDK's bit sequence for the high-bit case).
#[cfg(feature = "translated")]
    pub fn cvt_u32(a: V128) -> V128 {
        V128::map_u32(a, |x, _| {
            if x & 0x8000_0000 != 0 {
                let x1 = x.wrapping_add(127);
                let x0 = (x << 23) >> 31;
                ((x0.wrapping_add(x1) as i32 >> 8) as u32).wrapping_add(0x4F80_0000)
            } else {
                (x as i32 as f32).to_bits()
            }
        })
    }
    /// `simde_mm_round_ps` to nearest (even).
#[cfg(feature = "translated")]
    pub fn round_nearest(a: V128) -> V128 {
        V128::map_u32(a, |x, _| f32::from_bits(x).round_ties_even().to_bits())
    }
    /// `rex::ppc::ppc_vrsqrtefp_bits`, per lane.
    pub fn rsqrte(a: V128) -> V128 {
        V128::map_u32(a, |x, _| rsqrte_bits(x))
    }
    /// `simde_mm_vmsum4fp128_ps`: SIMDe's sequential `dp_ps` fallback (mask
    /// 0xFF), then non-finite → QNaN, denormal → signed zero, splatted.
#[cfg(feature = "translated")]
    pub fn msum4(a: V128, b: V128) -> V128 {
        let mut sum = 0.0f32;
        for i in 0..4 {
            sum += a.f32(i) * b.f32(i);
        }
        let mut bits = sum.to_bits();
        if !sum.is_finite() {
            bits = 0x7FC0_0000;
        } else if (bits >> 23) & 0xFF == 0 && bits & 0x007F_FFFF != 0 {
            bits &= 0x8000_0000;
        }
        set1_u32(bits)
    }

    pub fn rsqrte_bits(bits: u32) -> u32 {
        const TABLE: [u32; 32] = [
            0x0568B4FD, 0x04F3AF97, 0x048DAAA5, 0x0435A618, 0x03E7A1E4, 0x03A29DFE, 0x03659A5C, 0x032E96F8,
            0x02FC93CA, 0x02D090CE, 0x02A88DFE, 0x02838B57, 0x026188D4, 0x02438673, 0x02268431, 0x020B820B,
            0x03D27FFA, 0x03807C29, 0x033878AA, 0x02F97572, 0x02C27279, 0x02926FB7, 0x02666D26, 0x023F6AC0,
            0x021D6881, 0x01FD6665, 0x01E16468, 0x01C76287, 0x01AF60C1, 0x01995F12, 0x01855D79, 0x01735BF4,
        ];
        let sign = bits >> 31;
        let biased_exp = (bits >> 23) & 0xFF;
        let mantissa = bits & 0x007F_FFFF;
        if bits == 0xFF80_0000 {
            return 0x7FC0_0000;
        }
        if biased_exp == 0 {
            return if sign != 0 { 0xFF80_0000 } else { 0x7F80_0000 };
        }
        if biased_exp == 0xFF {
            return if mantissa == 0 { 0 } else { bits | 0x0040_0000 };
        }
        if sign != 0 {
            return 0x7FC0_0000;
        }
        let unbiased_exp = biased_exp as i32 - 127;
        let index = ((((unbiased_exp as u32) << 4) & 16) | (mantissa >> 19)) ^ 16;
        let interp = (mantissa >> 9) & 1023;
        let mut result_exp = (127 - biased_exp as i32) >> 1;
        let entry = TABLE[index as usize];
        let slope = entry >> 16;
        let base = (entry << 10) & 0x3FF_FC00;
        let mut raw = (base as i32).wrapping_sub(interp.wrapping_mul(slope) as i32);
        if raw & (1 << 25) == 0 {
            let val = (raw as u32) & 0x1FF_FFFF;
            let lz = val.leading_zeros() as i32;
            let shift = lz - 6;
            result_exp += 6 - lz;
            raw = raw.wrapping_shl(shift as u32);
        }
        if (raw & 5) != 0 && (raw & 2) != 0 {
            raw = raw.wrapping_add(4);
        }
        let result = ((result_exp << 23) as u32).wrapping_add(0x3F80_0000) | (((raw as u32) >> 2) & 0x7F_FFFF);
        if (result >> 23) & 0xFF == 0 && result & 0x7F_FFFF != 0 { 0 } else { result }
    }
}

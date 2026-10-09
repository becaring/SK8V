//! Ports of the game's math library routines (TU3 `0x82F4xxxx`), shared by
//! the glue and the EA Audio Core DSP.

use crate::ppc::fctid;

/// `0x82FB4E20`.. constants of the game's sine/cosine.
const HALF_PI: f64 = std::f64::consts::FRAC_PI_2;
const INV_PI: f64 = std::f64::consts::FRAC_1_PI;
const LIMIT: f64 = 220_000_000.0;
const PI_HI: f64 = 3.141_592_653_468_251_2;
const PI_LO: f64 = 1.215_420_101_260_793_2e-10;
const POLY: [f64; 7] = [
    -0.166_666_666_666_666_66,
    0.008_333_333_333_333_165,
    -0.000_198_412_698_412_018_4,
    2.755_731_921_015_275_6e-06,
    -2.505_210_679_827_458_3e-08,
    1.605_893_649_037_159e-10,
    -7.642_917_806_891_047e-13,
];
const POLY_TOP: f64 = 2.720_479_095_788_884_7e-15;

/// `fnmsub d, a, c, b` = `-(a·c − b)` fused.
fn fnmsub(a: f64, c: f64, b: f64) -> f64 {
    -a.mul_add(c, -b)
}

/// The shared reduction and polynomial; returns the value before the sign.
fn kernel(abs: f64, n: f64) -> (f64, bool) {
    let r = fnmsub(PI_HI, n, abs);
    let r = fnmsub(PI_LO, n, r);
    let r2 = r * r;
    let mut p = POLY_TOP.mul_add(r2, POLY[6]);
    for c in POLY[..6].iter().rev() {
        p = p.mul_add(r2, *c);
    }
    let p = p.mul_add(r2, 1.0);
    (p * r, false)
}

/// `0x82F4DFB0`: cosine.
pub fn cos(x: f64) -> f64 {
    let abs = x.abs();
    let shifted = HALF_PI + abs;
    let n = fctid(INV_PI * shifted) as f64;
    let m = n - 0.5;
    let odd = (n as i64 as u64) & 1 != 0;
    let (mut v, _) = kernel(abs, m);
    if odd {
        v = -v;
    }
    if abs == 0.0 {
        return 1.0;
    }
    if shifted - LIMIT >= 0.0 { f64::NAN } else { v }
}

/// `0x82F4DED0`: sine.
pub fn sin(x: f64) -> f64 {
    let abs = x.abs();
    let n = fctid(INV_PI * abs) as f64;
    let sign = if x >= 0.0 { 1.0 } else { -1.0 };
    let odd = (n as i64 as u64) & 1 != 0;
    let (mut v, _) = kernel(abs, n);
    if odd {
        v = -v;
    }
    let v = v * sign;
    if abs.to_bits() != 0 {
        if abs - LIMIT >= 0.0 { f64::NAN } else { v }
    } else {
        x
    }
}

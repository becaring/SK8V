//! The 40 AEMS module handlers (table `0x82FD3600`) and the program walk of
//! the control tick (`0x82B1E290`), translated instruction by instruction
//! from the TU3 PowerPC (`local/re/aems_handlers.txt`).
//!
//! Floating point follows the recompiled game: single-precision
//! adds, multiplies and divides are IEEE `f32` operations (identical to the
//! recomp's double-then-round), `fmadds` is a double `fma` rounded to `f32`,
//! `fctiwz` truncates with NaN giving `i32::MIN`. PowerPC float branches test
//! one condition bit, so with NaN `bge`/`ble`/`bne` are taken; the
//! translations keep that by negating the opposite comparison. Traps
//! (`tw*`) only log in the recomp, so division by zero yields 0 and execution
//! continues, as there.

use crate::sine::QUARTER_SINE;
use crate::live::engine::Voices;
use crate::voice;
use crate::world::World;

/// Native handler addresses by op index (`0x82FD3600`, read from the image).
pub const HANDLERS: [u32; 40] = [
    0x82B1_BF68, 0x8283_2BA8, 0x82C8_CDC8, 0x82B1_BF80, 0x82B1_C150, 0x82B1_C210, 0x82B1_C4B8, 0x82B1_C528,
    0x82B1_C598, 0x82B1_C6C8, 0x82B1_C778, 0x82B1_C7E8, 0x82B1_C878, 0x82B1_C8D0, 0x82B1_C910, 0x82B1_CAD8,
    0x82B1_CD28, 0x82B1_CE18, 0x82B1_CE48, 0x82B1_CEA0, 0x82B1_CEF8, 0x82B1_CF50, 0x82B1_D118, 0x82B1_D1A8,
    0x82B1_D1B8, 0x82B1_D1C8, 0x82B1_D200, 0x82B1_D240, 0x82B1_D3D0, 0x82B1_D5D0, 0x82B1_D700, 0x82B1_D790,
    0x82B1_D7B0, 0x82B1_CEE0, 0x82B1_CF38, 0x82B1_D098, 0x82B1_D198, 0x82B1_D7D0, 0x82B1_C2B8, 0x82B1_C450,
];

/// Float constants as the game loads them: 0.5 (`0x8209975C`),
/// 0.0 (`0x82165A10`), 1.0 (`0x8231A844`).
const HALF: f32 = 0.5;
const ZERO: f32 = 0.0;
const ONE: f32 = 1.0;

/// `fctiwz` as the recomp emits it.
pub fn fctiwz(x: f32) -> i32 {
    crate::ppc::fctiwz(f64::from(x)) as i32
}

/// PowerPC `fsel d, a, b, c`: `a >= 0 ? b : c` (NaN takes `c`).
pub fn fsel(a: f32, b: f32, c: f32) -> f32 {
    if a >= 0.0 { b } else { c }
}

/// `fmadds` as the recomp emits it: `float(fma(a, c, b))` in double.
pub fn fmadds(a: f32, c: f32, b: f32) -> f32 {
    (a as f64).mul_add(c as f64, b as f64) as f32
}

/// `fmsubs` as the recomp emits it: `float(fma(a, c, -b))` in double.
pub fn fmsubs(a: f32, c: f32, b: f32) -> f32 {
    (a as f64).mul_add(c as f64, -(b as f64)) as f32
}

/// `fnmsubs` as the recomp emits it: `float(-fma(a, c, -b))` in double.
pub fn fnmsubs(a: f32, c: f32, b: f32) -> f32 {
    (-(a as f64).mul_add(c as f64, -(b as f64))) as f32
}

/// The game's rounding idiom: `x < 0 ? (int)(x - 0.5) : (int)(x + 0.5)`
/// (`fcmpu x, 0.0; blt`), so NaN takes the `+ 0.5` side.
pub fn round(x: f32) -> i32 {
    if x < ZERO { fctiwz(x - HALF) } else { fctiwz(x + HALF) }
}

/// Runs one program from the active list: `code` is the module list, `data`
/// the instance's first module data. Literal walk of `0x82B1E354..E3E4`.
pub fn run(w: &mut World, code: u32, data: u32, vs: &mut Voices) {
    let mut code = code;
    let mut data = data;
    let mut op = w.mem.r8(code);
    if op == 255 {
        return;
    }
    loop {
        let r3 = call(w, op, data, vs);
        let mut wire = code + 4;
        let mut k = 0u32;
        while k < w.mem.r8(code + 1) as u32 {
            let src = w.mem.ri32(wire);
            let dst = w.mem.ri32(wire + 4);
            let to = data.wrapping_add(dst as u32);
            if src == -1 {
                w.mem.w32(to, r3);
            } else {
                let v = w.mem.r32(data.wrapping_add(src as u32));
                w.mem.w32(to, v);
            }
            k += 1;
            wire += 8;
        }
        let advance = w.mem.r32(wire);
        code = wire + 4;
        data = data.wrapping_add(advance);
        op = w.mem.r8(code);
        if op == 255 {
            return;
        }
    }
}

/// Indirect call through the handler table.
pub fn call(w: &mut World, op: u8, d: u32, vs: &mut Voices) -> u32 {
    match op {
        0 => op_take(w, d, 16),
        1 => w.mem.r32(d + 20),
        2 => w.mem.r32(d + 24),
        3 => op_take(w, d, 0),
        4 => op4_end(w, d, vs),
        5 => op5_message(w, d, vs),
        6 => op6_step(w, d),
        7 => op7_random(w, d),
        8 => op8_shuffle(w, d),
        9 => op9_weighted(w, d),
        10 => op10_range_trigger(w, d),
        11 => op11_timer(w, d),
        12 => op12_latch(w, d),
        13 => op13_or(w, d),
        14 => op14_envelope(w, d),
        15 => op15_curve(w, d),
        16 => op16_delay(w, d),
        17 => op17_select(w, d),
        18 => op18_demux(w, d),
        19 => op19_min_n(w, d),
        20 => op20_max_n(w, d),
        21 => op21_product(w, d),
        22 => op22_sum(w, d),
        23 => w.mem.r32(d).wrapping_sub(w.mem.r32(d + 4)),
        24 => w.mem.r32(d + 4).wrapping_mul(w.mem.r32(d)),
        25 => op25_div(w, d),
        26 => op26_rem(w, d),
        27 => voice::op27_player(w, d, vs),
        28 => op28_lfo(w, d),
        29 => op29_slew(w, d),
        30 => op30_sum_capped(w, d),
        31 => op31_sub_floor(w, d),
        32 => op32_mul_cap(w, d),
        33 => {
            let (a, b) = (w.mem.ri32(d), w.mem.ri32(d + 4));
            (if a >= b { b } else { a }) as u32
        }
        34 => {
            let (a, b) = (w.mem.ri32(d), w.mem.ri32(d + 4));
            (if a <= b { b } else { a }) as u32
        }
        35 => op35_scaled_product(w, d),
        36 => w.mem.r32(d + 4).wrapping_add(w.mem.r32(d)),
        37 => {
            let v = w.mem.r8(d + 25);
            w.mem.w8(d + 25, 0);
            v as u32
        }
        38 => op38_child(w, d, vs),
        39 => op39_global(w, d, vs),
        _ => panic!("AEMS op {op} outside the 40-entry handler table"),
    }
}

/// Ops 0 and 3 (`82B1BF68`, `82B1BF80`): return a word and clear it.
fn op_take(w: &mut World, d: u32, off: u32) -> u32 {
    let v = w.mem.r32(d + off);
    w.mem.w32(d + off, 0);
    v
}

/// Op 4 (`82B1C150`). `d` is the instance's `{record, instance, object}`
/// header; on the `+12` trigger the instance leaves its record's list and
/// the active list and is freed (`82B1BF98`).
pub fn op4_end(w: &mut World, d: u32, vs: &mut Voices) -> u32 {
    if w.mem.ri32(d + 12) != 0 {
        let rec = w.mem.r32(d);
        let inst = w.mem.r32(d + 4);
        w.mem.list_remove(rec + 56, inst);
        let active = w.active;
        w.mem.list_remove(active, inst + 8);
        w.free_instance(d, vs);
    }
    0
}

/// Clamp loop shared by ops 5 and 38: `n` values after `vals` against
/// `[min, max]` pairs at `ranges` (`cmpw`: below min → min, else above max →
/// max).
fn clamp_values(w: &mut World, n_at: u32, ranges: u32, vals: u32) {
    let mut r11 = ranges;
    let mut r8 = vals;
    let mut k = 0i32;
    loop {
        let min = w.mem.ri32(r11);
        let mut v = w.mem.ri32(r8 + 4);
        let max = w.mem.ri32(r11 + 4);
        if v < min {
            v = min;
        } else if v > max {
            v = max;
        }
        k += 1;
        r8 += 4;
        w.mem.wi32(r8, v);
        r11 += 8;
        if !(k < w.mem.r8(n_at) as i32) {
            break;
        }
    }
}

/// Op 5 (`82B1C210`): optional clamp, then send the message at `d+0` with
/// the words after the trigger.
fn op5_message(w: &mut World, d: u32, vs: &mut Voices) -> u32 {
    let r5;
    if w.mem.r8(d + 8) != 0 {
        let n = w.mem.r8(d + 9) as i32;
        let r11 = d + 12;
        r5 = r11 + ((n as u32) << 3);
        if n > 0 {
            clamp_values(w, d + 9, r11, r5);
        }
    } else {
        r5 = d + 12;
    }
    if w.mem.ri32(r5) != 0 {
        w.send_message(d, r5 + 4, vs);
    }
    0
}

/// Op 6 (`82B1C4B8`): in-range input passes; otherwise a counter stepping by
/// the s8 at `+12` while `+16 > 0`, wrapping inside `[+0, +4]`.
fn op6_step(w: &mut World, d: u32) -> u32 {
    let r3 = w.mem.ri32(d + 20);
    let r7 = w.mem.ri32(d);
    if !(r3 < r7) {
        let r10 = w.mem.ri32(d + 4);
        if !(r3 > r10) {
            return r3 as u32;
        }
    }
    if w.mem.ri32(d + 16) > 0 {
        let step = w.mem.r8(d + 12) as i8 as i32;
        let r10 = step.wrapping_add(w.mem.ri32(d + 8));
        let r9 = w.mem.ri32(d + 4);
        w.mem.wi32(d + 8, r10);
        if r10 > r9 {
            w.mem.wi32(d + 8, r7);
            return r7 as u32;
        }
        if r10 < r7 {
            w.mem.wi32(d + 8, r9);
        }
    }
    w.mem.r32(d + 8)
}

/// `divwu` with the recomp's zero divisor result.
fn divwu(a: u32, b: u32) -> u32 {
    a.checked_div(b).unwrap_or(0)
}

/// `divw` with the recomp's results for zero and `MIN / -1`.
fn divw(a: i32, b: i32) -> i32 {
    if b != 0 && !(a == i32::MIN && b == -1) { a / b } else { 0 }
}

/// Op 7 (`82B1C528`): on `+12`, `+8 = +0 + rand() % +4`.
fn op7_random(w: &mut World, d: u32) -> u32 {
    if w.mem.ri32(d + 12) == 0 {
        return w.mem.r32(d + 8);
    }
    let r3 = w.rng.next();
    let r11 = w.mem.r32(d + 4);
    let r10 = w.mem.r32(d);
    let r9 = divwu(r3, r11);
    let r11 = r3.wrapping_sub(r9.wrapping_mul(r11));
    let v = r11.wrapping_add(r10);
    w.mem.w32(d + 8, v);
    v
}

/// Op 8 (`82B1C598`): shuffle bag over u8 (`+2 == 1`) or u16 items at `+16`.
fn op8_shuffle(w: &mut World, d: u32) -> u32 {
    let trig = w.mem.r16(d) as u32;
    if w.mem.ri32(d.wrapping_add(trig)) == 0 {
        return w.mem.r32(d + 12);
    }
    let r30 = w.mem.r16(d + 8) as u32;
    let r3 = w.rng.next();
    let r9 = w.mem.r8(d + 3) as i8 as i32 as u32;
    let r10 = w.mem.r16(d + 10) as u32;
    let r7 = r10.wrapping_sub(r9);
    let r6 = r7.wrapping_sub(r30);
    let r5 = divwu(r3, r6);
    let r11 = r30.wrapping_add(r3.wrapping_sub(r5.wrapping_mul(r6)));
    if w.mem.r8(d + 2) == 1 {
        let at = r11.wrapping_add(d);
        let pick = w.mem.r8(at + 16) as u32;
        w.mem.w32(d + 12, pick);
        let other = w.mem.r8(r30 + d + 16);
        w.mem.w8(at + 16, other);
        let v = w.mem.r32(d + 12);
        let cur = w.mem.r16(d + 8) as u32;
        w.mem.w8(cur + d + 16, v as u8);
    } else {
        let at = d.wrapping_add((r11.wrapping_add(8)) << 1);
        let pick = w.mem.r16(at) as u32;
        w.mem.w32(d + 12, pick);
        let other = w.mem.r16(d + ((r30 + 8) << 1));
        w.mem.w16(at, other);
        let v = w.mem.r32(d + 12);
        let cur = w.mem.r16(d + 8) as u32;
        w.mem.w16(d + ((cur + 8) << 1), v as u16);
    }
    let r9 = w.mem.r16(d + 8) as u32 + 1;
    let r10 = w.mem.r32(d + 12);
    let r11 = w.mem.r32(d + 4);
    let r8 = w.mem.r16(d + 10) as u32;
    let r7 = r9 & 0xFFFF;
    let r3 = r11.wrapping_add(r10);
    w.mem.w16(d + 8, r7 as u16);
    w.mem.w32(d + 12, r3);
    if r7 < r8 {
        w.mem.w8(d + 3, 0);
    } else {
        w.mem.w16(d + 8, 0);
        w.mem.w8(d + 3, 1);
    }
    r3
}

/// Op 9 (`82B1C6C8`): on `+16`, weighted pick: `rand() % 100` against
/// cumulative s8 weights at `table+16` (table `+0`, count `+8`), result
/// `+4 + index`.
fn op9_weighted(w: &mut World, d: u32) -> u32 {
    if w.mem.ri32(d + 16) != 0 {
        let mut r31: u32 = 0;
        let r3 = w.rng.next();
        let r9 = w.mem.ri32(d + 8);
        let r10 = w.mem.r32(d);
        // mulhwu by 0x51EB851F then >> 5 is r3 / 100.
        let q = ((r3 as u64 * 0x51EB_851F) >> 32) as u32 >> 5;
        let roll = r3.wrapping_sub(q.wrapping_mul(100));
        let mut r11 = 0i32;
        if r9 > 0 {
            let r8 = r10 + 16;
            loop {
                let weight = w.mem.r8(r8.wrapping_add(r11 as u32)) as i8 as i32;
                r31 = (weight as u32).wrapping_add(r31);
                if r31 > roll {
                    let base = w.mem.r32(d + 4);
                    w.mem.w32(d + 12, base.wrapping_add(r11 as u32));
                    break;
                }
                r11 += 1;
                if !(r11 < w.mem.ri32(d + 8)) {
                    break;
                }
            }
        }
    }
    w.mem.r32(d + 12)
}

/// Op 10 (`82B1C778`): returns 1 once on entering `[+0, +4]`; re-armed when
/// the input enters `[+8, +12]`.
fn op10_range_trigger(w: &mut World, d: u32) -> u32 {
    let r10 = w.mem.ri32(d + 20);
    if !(r10 < w.mem.ri32(d)) && !(r10 > w.mem.ri32(d + 4)) {
        if w.mem.r8(d + 16) == 0 {
            w.mem.w8(d + 16, 1);
            w.mem.w8(d + 17, 1);
            return 1;
        }
    } else if !(r10 < w.mem.ri32(d + 8)) && !(r10 > w.mem.ri32(d + 12)) {
        w.mem.w8(d + 16, 0);
    }
    w.mem.w8(d + 17, 0);
    0
}

/// Op 11 (`82B1C7E8`): ms timer. `+8` resets to 0; counts by the tick time
/// while non-negative; at `>= +12` fires once and parks at -1.
fn op11_timer(w: &mut World, d: u32) -> u32 {
    let running = if w.mem.ri32(d + 8) != 0 {
        w.mem.wf32(d, ZERO);
        true
    } else {
        !(w.mem.rf32(d) < ZERO)
    };
    if running {
        let duration = w.mem.ri32(d + 12) as f32;
        let t = w.mem.rf32(d);
        if !(t < duration) {
            w.mem.w8(d + 4, 1);
            // 0x8216DEE0
            w.mem.wf32(d, -1.0);
            return 1;
        }
        w.mem.wf32(d, t + w.tick_ms);
    }
    w.mem.w8(d + 4, 0);
    0
}

/// Op 12 (`82B1C878`): the first non-zero of `n` inputs (at `+0` offset)
/// selects the stored value at `+8 + 4k` into `+4`.
fn op12_latch(w: &mut World, d: u32) -> u32 {
    let mut r10 = d.wrapping_add(w.mem.r16(d) as u32);
    let mut k = 0i32;
    if w.mem.r8(d + 2) != 0 {
        loop {
            if w.mem.ri32(r10) != 0 {
                let v = w.mem.r32(d + ((k as u32 + 2) << 2));
                w.mem.w32(d + 4, v);
                break;
            }
            k += 1;
            r10 += 4;
            if !(k < w.mem.r8(d + 2) as i32) {
                break;
            }
        }
    }
    w.mem.r32(d + 4)
}

/// Op 13 (`82B1C8D0`): 1 if any of `n` inputs is non-zero.
fn op13_or(w: &mut World, d: u32) -> u32 {
    let n = w.mem.r8(d) as i32;
    for k in 0..n.max(0) as u32 {
        if w.mem.ri32(d + 4 + 4 * k) != 0 {
            return 1;
        }
    }
    0
}

/// Op 14 (`82B1C910`): multi-segment envelope. Gate (at the `+0` offset):
/// 1 attack, 2 hold, 3 release (segment `+18`). `+3` segment index, `+4`
/// time left, `+8` step per tick, `+12` value, `+16` segment count, `+20`
/// start value, segments `{time, target}` from `+24`.
fn op14_envelope(w: &mut World, d: u32) -> u32 {
    let m = &mut w.mem;
    let tick = w.tick_ms;
    let r8 = m.r16(d) as u32;
    let r9 = m.ri32(d + r8);
    let f13 = ZERO;
    'done: {
        if r9 == 1 && m.r8(d + 2) == 0 {
            let f0 = m.rf32(d + 20);
            m.w8(d + 3, 0);
            m.wf32(d + 12, f0);
            let f12 = m.rf32(d + 24);
            m.wf32(d + 4, f12);
            let f11 = m.rf32(d + 28);
            m.wf32(d + 8, (f11 - f0) / f12 * tick);
            break 'done;
        }
        if r9 == 3 && m.r8(d + 2) != 3 {
            let r11 = m.r16(d + 18) as u32;
            let r10 = m.r8(d + 3) as i32;
            if r10 < (r11 as u16 as i16 as i32) {
                let seg = r11 & 0xFF;
                let f12 = m.rf32(d + 12);
                m.w8(d + 3, seg as u8);
                let f11 = m.rf32(d + ((seg + 3) << 3));
                m.wf32(d + 4, f11);
                let f10 = m.rf32(d + ((r11 << 3) & 0x7F8) + 28);
                m.wf32(d + 8, (f10 - f12) / f11 * tick);
                break 'done;
            }
        }
        if r9 == 1 || r9 == 3 {
            let r10 = m.r8(d + 16) as u32;
            let r11 = m.r8(d + 3) as u32;
            if r11 < r10 {
                let f11 = m.rf32(d + 4) - tick;
                m.wf32(d + 4, f11);
                if f11 > f13 {
                    let v = m.rf32(d + 8) + m.rf32(d + 12);
                    m.wf32(d + 12, v);
                    break 'done;
                }
                let f12 = m.rf32(d + (r11 << 3) + 28);
                let next = (r11 + 1) & 0xFF;
                m.w8(d + 3, next as u8);
                m.wf32(d + 12, f12);
                if next >= r10 {
                    m.wf32(d + 12, f13);
                    break 'done;
                }
                let f11 = m.rf32(d + ((next + 3) << 3));
                m.wf32(d + 4, f11);
                let f9 = m.rf32(d + (next << 3) + 28);
                m.wf32(d + 8, (f9 - f12) / f11 * tick);
                break 'done;
            }
        }
        if r9 != 2 {
            m.wf32(d + 12, f13);
        }
    }
    let gate = m.r32(d + r8);
    m.w8(d + 2, gate as u8);
    round(m.rf32(d + 12)) as u32
}

/// Op 15 (`82B1CAD8`): table curve. Input `+12`, last input `+4`, output
/// `+8`, curve `+0`: `{u8 type (1 s8, 2 s16, else s32), u16 count at +2,
/// lo +4, hi +8, f32 step +12, values from +16}`.
fn op15_curve(w: &mut World, d: u32) -> u32 {
    let m = &mut w.mem;
    let mut r10 = m.ri32(d + 12);
    if r10 == m.ri32(d + 4) {
        return m.r32(d + 8);
    }
    let c = m.r32(d);
    m.wi32(d + 4, r10);
    let lo = m.ri32(c + 4);
    let hi = m.ri32(c + 8);
    if r10 < lo {
        r10 = lo;
    } else if r10 > hi {
        r10 = hi;
    }
    let x = r10.wrapping_sub(lo);
    let step = m.rf32(c + 12);
    let kind = m.r8(c);
    let at = |k: i32| -> u32 { c.wrapping_add(k as u32) };
    let v = if step == ONE {
        if kind == 2 {
            m.r16(at((x + 8) << 1)) as i16 as i32
        } else if kind == 1 {
            m.r8(at(x).wrapping_add(16)) as i8 as i32
        } else {
            m.ri32(at((x + 4) << 2))
        }
    } else {
        let f13 = x as f32 * step;
        let i = round(f13 - HALF);
        let count = m.r16(c + 2) as i32;
        let frac = f13 - i as f32;
        let mut j = i.wrapping_add(1);
        if !(j < count) {
            j = count - 1;
        }
        let (a, b) = if kind == 2 {
            (m.r16(at((i + 8) << 1)) as i16 as i32, m.r16(at((j + 8) << 1)) as i16 as i32)
        } else if kind == 1 {
            (m.r8(at(i).wrapping_add(16)) as i8 as i32, m.r8(at(j).wrapping_add(16)) as i8 as i32)
        } else {
            (m.ri32(at((i + 4) << 2)), m.ri32(at((j + 4) << 2)))
        };
        let (a, b) = (a as f32, b as f32);
        round(fmadds(b - a, frac, a))
    };
    m.wi32(d + 8, v);
    v as u32
}

/// Op 16 (`82B1CD28`): delay line. Input at the `+0` offset (`+0` value,
/// `+4` delay ms), `+2` capacity, `+4` write slot, `+6` read slot, `+8` last
/// delay, ring from `+12`.
fn op16_delay(w: &mut World, d: u32) -> u32 {
    let tick = w.tick_ms;
    let m = &mut w.mem;
    let p = d.wrapping_add(m.r16(d) as u32);
    let r10 = m.ri32(p + 4);
    if r10 != m.ri32(d + 8) {
        m.wi32(d + 8, r10);
        if m.ri32(p + 4) < 0 {
            m.w32(p + 4, 0);
        }
        let ms = m.ri32(p + 4);
        let cap = m.r16(d + 2) as i32;
        let mut ticks = fctiwz(ms as f32 / tick + HALF);
        if !(ticks < cap) {
            ticks = cap - 1;
        }
        let read = m.r16(d + 6) as i32;
        m.w16(d + 4, read.wrapping_add(ticks) as u16);
    }
    let cap = m.r16(d + 2) as u32;
    let wr = m.r16(d + 4) as u32;
    if !(wr < cap) {
        m.w16(d + 4, wr.wrapping_sub(cap) as u16);
    }
    if !((m.r16(d + 6) as u32) < cap) {
        m.w16(d + 6, 0);
    }
    let wr = m.r16(d + 4) as u32;
    let v = m.r32(p);
    m.w32(d + ((wr + 3) << 2), v);
    let wr = m.r16(d + 4) as u32;
    let rd = m.r16(d + 6) as u32;
    let out = m.r32(d + ((rd + 3) << 2));
    m.w16(d + 6, (rd + 1) as u16);
    m.w16(d + 4, (wr + 1) as u16);
    out
}

/// Op 17 (`82B1CE18`): 1-based select among `n` inputs from `+8`.
fn op17_select(w: &mut World, d: u32) -> u32 {
    let i = w.mem.ri32(d + 4);
    if i > 0 && !(i > w.mem.r8(d) as i32) {
        return w.mem.r32(d + ((i as u32 + 1) << 2));
    }
    0
}

/// Op 18 (`82B1CE48`): clear the previous output slot, route `+8` to slot
/// `+4` (1-based, from `+12`), remember it at `+2`; returns `+12`.
fn op18_demux(w: &mut World, d: u32) -> u32 {
    let m = &mut w.mem;
    let prev = m.r16(d + 2) as i16 as i32;
    m.w32(d.wrapping_add(((prev + 2) as u32) << 2), 0);
    let i = m.ri32(d + 4);
    if i > 0 && !(i > m.r8(d) as i32) {
        let v = m.r32(d + 8);
        m.w32(d + ((i as u32 + 2) << 2), v);
        let s = m.r32(d + 4);
        m.w16(d + 2, s as u16);
    }
    m.r32(d + 12)
}

/// Op 19 (`82B1CEA0`).
fn op19_min_n(w: &mut World, d: u32) -> u32 {
    let n = w.mem.r8(d) as i32;
    let mut r3 = w.mem.ri32(d + 4);
    for k in 0..(n - 1).max(0) as u32 {
        let v = w.mem.ri32(d + 8 + 4 * k);
        if v < r3 {
            r3 = v;
        }
    }
    r3 as u32
}

/// Op 20 (`82B1CEF8`).
fn op20_max_n(w: &mut World, d: u32) -> u32 {
    let n = w.mem.r8(d) as i32;
    let mut r3 = w.mem.ri32(d + 4);
    for k in 0..(n - 1).max(0) as u32 {
        let v = w.mem.ri32(d + 8 + 4 * k);
        if v > r3 {
            r3 = v;
        }
    }
    r3 as u32
}

/// Op 21 (`82B1CF50`): `round(scale · Π inputs)`, the product taken left to
/// right in `f32` (the unrolled loop keeps that order).
fn op21_product(w: &mut World, d: u32) -> u32 {
    let n = w.mem.r8(d) as i32;
    let mut f0 = w.mem.ri32(d + 8) as f32;
    for k in 1..n.max(1) as u32 {
        f0 *= w.mem.ri32(d + 8 + 4 * k) as f32;
    }
    let f0 = w.mem.rf32(d + 4) * f0;
    round(f0) as u32
}

/// Op 22 (`82B1D118`): wrapping sum of `n` inputs from `+4`.
fn op22_sum(w: &mut World, d: u32) -> u32 {
    let n = w.mem.r8(d) as i32;
    let mut r3 = w.mem.r32(d + 4);
    for k in 1..n.max(1) as u32 {
        r3 = r3.wrapping_add(w.mem.r32(d + 4 + 4 * k));
    }
    r3
}

/// Op 25 (`82B1D1C8`).
fn op25_div(w: &mut World, d: u32) -> u32 {
    let b = w.mem.ri32(d + 4);
    if b == 0 {
        return 0;
    }
    divw(w.mem.ri32(d), b) as u32
}

/// Op 26 (`82B1D200`).
fn op26_rem(w: &mut World, d: u32) -> u32 {
    let b = w.mem.ri32(d + 4);
    if b == 0 {
        return 0;
    }
    let a = w.mem.ri32(d);
    a.wrapping_sub(divw(a, b).wrapping_mul(b)) as u32
}

/// Op 28 (`82B1D3D0`): LFO. `+0` shape (0 sine, 1 square, 2 saw, else
/// triangle), `+4` f32 phase in [0, 1), `+8` period ms, `+12` amplitude.
fn op28_lfo(w: &mut World, d: u32) -> u32 {
    let tick = w.tick_ms;
    let m = &mut w.mem;
    let period = m.ri32(d + 8);
    if !(period > 0) {
        return 0;
    }
    let amp = m.ri32(d + 12) as f32;
    let f9 = tick / period as f32;
    if !(m.rf32(d + 4) < ONE) {
        loop {
            let f11 = m.rf32(d + 4) - ONE;
            m.wf32(d + 4, f11);
            if f11 < ONE {
                break;
            }
        }
    }
    let shape = m.r8(d);
    let phase = m.rf32(d + 4);
    let f0 = match shape {
        0 => {
            // 0x822F8EA4
            let r11 = round(phase * 1024.0);
            let quadrant = (r11 >> 8) & 3;
            let i = (r11 & 0xFF) as usize;
            let s = match quadrant {
                0 => QUARTER_SINE[i] as i32,
                1 => QUARTER_SINE[256 - i] as i32,
                2 => -(QUARTER_SINE[i] as i32),
                _ => -(QUARTER_SINE[256 - i] as i32),
            };
            // 0x82098D0C = 1/65536
            s as f32 * amp * (1.0 / 65536.0)
        }
        1 => {
            if !(phase < HALF) { amp } else { ZERO }
        }
        2 => phase * amp,
        _ => {
            // 0x82060C50 = 2.0
            if !(phase < HALF) { (ONE - phase) * amp * 2.0 } else { phase * amp * 2.0 }
        }
    };
    m.wf32(d + 4, phase + f9);
    round(f0) as u32
}

/// Op 29 (`82B1D5D0`): slew. `+0` f32 current, `+4` f32 rate, `+8` last
/// target, `+12` last time, `+16` time ms, `+20` rate multiplier, `+24`
/// target.
fn op29_slew(w: &mut World, d: u32) -> u32 {
    let tick = w.tick_ms;
    let m = &mut w.mem;
    let target = m.ri32(d + 24);
    let f0 = target as f32;
    let f13 = m.rf32(d);
    if f0 == f13 {
        return target as u32;
    }
    if target != m.ri32(d + 8) || m.ri32(d + 16) != m.ri32(d + 12) {
        let time = m.ri32(d + 16);
        m.wi32(d + 8, target);
        m.wi32(d + 12, time);
        if !(time > 0) {
            m.wf32(d, f0);
            return target as u32;
        }
        // 0x822F890C = 1/4096
        m.wf32(d + 4, (f0 - f13) / time as f32 * tick * (1.0 / 4096.0));
    }
    let rate = m.rf32(d + 4);
    let mul = m.ri32(d + 20) as f32;
    let next = fmadds(mul, rate, f13);
    m.wf32(d, next);
    if rate < ZERO {
        if next < f0 {
            m.wf32(d, f0);
        }
    } else if next > f0 {
        m.wf32(d, f0);
    }
    round(m.rf32(d)) as u32
}

/// Op 30 (`82B1D700`): wrapping sum of `n` inputs from `+8`, capped at `+4`.
fn op30_sum_capped(w: &mut World, d: u32) -> u32 {
    let n = w.mem.r8(d) as i32;
    let mut r3 = w.mem.r32(d + 8);
    for k in 1..n.max(1) as u32 {
        r3 = r3.wrapping_add(w.mem.r32(d + 8 + 4 * k));
    }
    let cap = w.mem.ri32(d + 4);
    if !(r3 as i32 > cap) { r3 } else { cap as u32 }
}

/// Op 31 (`82B1D790`): `max(+4 - +8, +0)`.
fn op31_sub_floor(w: &mut World, d: u32) -> u32 {
    let r3 = w.mem.ri32(d + 4).wrapping_sub(w.mem.ri32(d + 8));
    let floor = w.mem.ri32(d);
    (if r3 >= floor { r3 } else { floor }) as u32
}

/// Op 32 (`82B1D7B0`): `min(+8 · +4, +0)`.
fn op32_mul_cap(w: &mut World, d: u32) -> u32 {
    let r3 = w.mem.ri32(d + 8).wrapping_mul(w.mem.ri32(d + 4));
    let cap = w.mem.ri32(d);
    (if r3 <= cap { r3 } else { cap }) as u32
}

/// Op 35 (`82B1D098`): `round(+0 · (+8 · +4))`.
fn op35_scaled_product(w: &mut World, d: u32) -> u32 {
    let k = w.mem.rf32(d);
    let a = w.mem.ri32(d + 4) as f32;
    let b = w.mem.ri32(d + 8) as f32;
    round(b * a * k) as u32
}

/// Op 38 (`82B1C2B8`): child sound object. `+0` class reference, `+8`
/// object, `+12` has ranges, `+13` value count, ranges from `+16`, then
/// `{create, stop, values...}`. Returns the object's reference count.
fn op38_child(w: &mut World, d: u32, vs: &mut Voices) -> u32 {
    let has_ranges = w.mem.r8(d + 12);
    let r4 = if has_ranges != 0 {
        let n = w.mem.r8(d + 13) as u32;
        (n << 3) + d + 16
    } else {
        d + 16
    };
    let r30 = d + 8;
    if w.mem.ri32(r4 + 4) != 0 {
        let obj = w.mem.r32(d + 8);
        if obj != 0 {
            w.release_object(obj, vs);
            w.mem.w32(r30, 0);
        }
    } else {
        let create = w.mem.ri32(r4);
        let obj = w.mem.r32(d + 8);
        if create != 0 {
            if obj == 0 {
                if has_ranges != 0 && w.mem.r8(d + 13) != 0 {
                    clamp_values(w, d + 13, d + 16, r4 + 4);
                }
                if w.create_object(d, r4 + 8, r30, vs) < 0 {
                    w.mem.w32(r30, 0);
                }
            }
        } else if obj != 0 {
            if has_ranges != 0 && w.mem.r8(d + 13) != 0 {
                clamp_values(w, d + 13, d + 16, r4 + 4);
            }
            let obj = w.mem.r32(r30);
            w.update_object(obj, r4 + 8, vs);
        }
    }
    let obj = w.mem.r32(r30);
    if obj != 0 { w.mem.r32(obj + 4) } else { 0 }
}

/// Op 39 (`82B1C450`): on a changed input (`+20` vs `+16`), set the global
/// parameter `+0` to the input clamped to `[+8, +12]`.
fn op39_global(w: &mut World, d: u32, vs: &mut Voices) -> u32 {
    let input = w.mem.ri32(d + 20);
    if w.mem.ri32(d + 16) != input {
        let min = w.mem.ri32(d + 8);
        let max = w.mem.ri32(d + 12);
        w.mem.wi32(d + 16, input);
        let v = if input < min {
            min
        } else if input > max {
            max
        } else {
            input
        };
        w.set_param(d, v as u32, vs);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_matches_the_game_idiom() {
        assert_eq!(round(2.5), 3);
        assert_eq!(round(-2.5), -3);
        assert_eq!(round(2.49), 2);
        assert_eq!(round(f32::NAN), i32::MIN);
        assert_eq!(fctiwz(3.0e10), i32::MAX);
        assert_eq!(fctiwz(-3.0e10), i32::MIN);
    }

    #[test]
    fn division_follows_the_recomp() {
        assert_eq!(divw(i32::MIN, -1), 0);
        assert_eq!(divw(7, 0), 0);
        assert_eq!(divw(-7, 2), -3);
        assert_eq!(divwu(7, 0), 0);
    }
}

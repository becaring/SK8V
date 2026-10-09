//! The game-side layer above AEMS: Skate's sound-class handlers, which turn a
//! component's packed audio parameters and skater state into AEMS object
//! vectors (`docs/AEMS.md`, "Game glue"). Translated from the TU3 code like
//! the rest of the crate.

pub mod board;
pub mod env;
pub mod foley;
pub mod grind;
pub mod rolling;
pub mod seams;
pub mod speed;
pub mod tables;

use std::collections::HashMap;

use crate::ops::fctiwz;

/// A component's packed parameter array `*(*(this + 12) + 12)`: 16-bit values,
/// two per big-endian word, even ids in the low half.
#[derive(Clone, Copy, Debug, Default)]
pub struct Params {
    /// `None` when either pointer is null (every read is then 0).
    pub words: Option<[u32; 16]>,
}

impl Params {
    fn raw(&self, id: i32) -> Option<i32> {
        let words = self.words.as_ref()?;
        let word = *words.get((id >> 1) as usize)? as i32;
        Some(word >> ((id & 1) * 16))
    }

    /// `0x824C2870` (vtable slot 13): unsigned 16 bits.
    pub fn u16(&self, id: i32) -> i32 {
        self.raw(id).map_or(0, |v| v & 0xFFFF)
    }

    /// `0x824AF240` (slots 15 and 16): 15 bits.
    pub fn u15(&self, id: i32) -> i32 {
        self.raw(id).map_or(0, |v| v & 0x7FFF)
    }

    /// `0x824C5910` (slot 14): signed 16-bit cents → pitch ratio × 4096.
    pub fn pitch(&self, id: i32) -> i32 {
        match self.raw(id) {
            None => 0,
            Some(v) => {
                let mut c = v & 0xFFFF;
                if c & 0x8000 != 0 {
                    c |= 0xFFFF_0000u32 as i32;
                }
                // 0x822F889C = 4096.0
                fctiwz(cents_to_ratio(c) * 4096.0)
            }
        }
    }
}

/// `0x8294B4D8`: cents → frequency ratio. Whole octaves by repeated doubling,
/// then the semitone and cent tables; reciprocals for negative input.
pub fn cents_to_ratio(cents: i32) -> f32 {
    let f = |bits: u32| f32::from_bits(bits);
    let negative = cents < 0;
    let mut c = cents;
    // 0x8231A844 = 1.0, 0x82060C50 = 2.0
    let mut scale = 1.0f32;
    if !(c < 1200) {
        let n = c as u32 / 1200;
        c = c.wrapping_sub((n * 1200) as i32);
        for _ in 0..n {
            scale *= 2.0;
        }
    }
    if !(c > -1200) {
        let n = ((-1200i32).wrapping_sub(c) as u32 / 1200) + 1;
        c = c.wrapping_add((n * 1200) as i32);
        for _ in 0..n {
            scale *= 2.0;
        }
    }
    if !negative {
        let semis = (c / 100) as usize;
        let rest = (c - (c / 100) * 100) as usize;
        f(tables::CENTS[rest]) * f(tables::SEMITONES[semis]) * scale
    } else {
        let m = c.wrapping_neg();
        let semis = (m / 100) as usize;
        let rest = (m - (m / 100) * 100) as usize;
        let a = 1.0 / f(tables::SEMITONES[semis]) / scale;
        a * (1.0 / f(tables::CENTS[rest]))
    }
}

/// Words of the AEMS object wrappers a component owns (`*(this + slot)`),
/// which persist between updates (a handler leaves some words alone).
#[derive(Clone, Debug, Default)]
pub struct Wrappers {
    pub map: HashMap<u32, Vec<i32>>,
}

impl Wrappers {
    pub fn words(&mut self, slot: u32, n: usize) -> &mut Vec<i32> {
        let w = self.map.entry(slot).or_insert_with(|| vec![0; n]);
        w.resize(n, 0);
        w
    }
}

pub use crate::ops::fsel;

/// The handlers' clamp idiom (`cmpwi; bge; li 0 / cmpwi hi; ble; li hi`).
pub fn clamp(v: i32, hi: i32) -> i32 {
    v.clamp(0, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cents_match_powers_of_two() {
        assert_eq!(cents_to_ratio(0), 1.0);
        assert_eq!(cents_to_ratio(1200), 2.0);
        assert_eq!(cents_to_ratio(-1200), 0.5);
        assert!((cents_to_ratio(700) - 2f32.powf(700.0 / 1200.0)).abs() < 1e-6);
        assert!((cents_to_ratio(-350) - 2f32.powf(-350.0 / 1200.0)).abs() < 1e-6);
    }

    #[test]
    fn packed_halves() {
        let mut words = [0; 16];
        words[0] = 0x8001_1234;
        let p = Params { words: Some(words) };
        assert_eq!(p.u16(0), 0x1234);
        assert_eq!(p.u16(1), 0x8001);
        assert_eq!(p.u15(1), 0x0001);
    }
}

//! AEMS's random number generator, `0x82B1F360` (TU3).
//!
//! A six-word add-with-carry generator. The state lives at `0x830775F0` and
//! is never seeded (all zero at startup, observed in the game's image
//! dump), so the sequence is the same from power-on. Literal translation of
//! the PowerPC, including its carry tests and the increment that ripples
//! through the words when the counter word wraps.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rng {
    /// `+0 .. +20`, big-endian words in the game; word 5 is the counter.
    pub s: [u32; 6],
}

impl Rng {
    pub fn next(&mut self) -> u32 {
        let s = &mut self.s;
        let r3 = s[5];
        let r9 = s[4];
        let r10 = r9.wrapping_add(r3);
        // r4 = 1 when r10 < r3, else (r10 >= r9 ? 0 : 1)
        let c1 = if r10 < r3 { 1 } else if r10 >= r9 { 0 } else { 1 };
        let r6 = r10;
        let s3 = s[3];
        let s2 = s[2];
        let s1 = s[1];
        let s0 = s[0];
        let r10 = s3.wrapping_add(r10).wrapping_add(c1);
        s[4] = r6;
        // subfc/subfe: carry is 1 when r10 >= old s3; c2 = 1 - carry.
        let c2 = if r10 >= s3 { 0 } else { 1 };
        s[3] = r10;
        let r9 = s2.wrapping_add(r10).wrapping_add(c2);
        let c3 = if r9 >= s2 { 0 } else { 1 };
        s[2] = r9;
        let r8 = s1.wrapping_add(r9).wrapping_add(c3);
        let c4 = if r8 >= s1 { 0 } else { 1 };
        s[1] = r8;
        let counter = r3.wrapping_add(1);
        s[5] = counter;
        let mut out = s0.wrapping_add(r8).wrapping_add(c4);
        s[0] = out;
        if counter != 0 {
            return out;
        }
        // Counter wrapped: ripple an increment through the words.
        s[4] = r6.wrapping_add(1);
        if s[4] != 0 {
            return out;
        }
        s[3] = r10.wrapping_add(1);
        if s[3] != 0 {
            return out;
        }
        s[2] = r9.wrapping_add(1);
        if s[2] != 0 {
            return out;
        }
        s[1] = r8.wrapping_add(1);
        if s[1] != 0 {
            return out;
        }
        out = out.wrapping_add(1);
        s[0] = out;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_state_sequence_starts_like_the_game() {
        // From all-zero state the first draws are deterministic; the values
        // follow from the recurrence (verified against the PowerPC by hand).
        let mut r = Rng::default();
        let a = r.next();
        assert_eq!(r.s[5], 1);
        assert_eq!(a, 0);
        let b = r.next();
        // s4 = 0+1 = 1, s3 = 0+1 = 1, s2 = 1, s1 = 1, s0 = 1
        assert_eq!(r.s, [1, 1, 1, 1, 1, 2]);
        assert_eq!(b, 1);
    }
}

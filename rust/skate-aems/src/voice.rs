//! The sound player module (op 27, `82B1D240`) and AEMS's voice glue:
//! release `82B1F440`, start `82B1F4C8`, per-tick update `82B1F5F8`,
//! attribute set `82B1BE30`. Below this line is EA Audio Core: the voice
//! factory (`*0x82FD35F8`, its vtable slot 0) and the voice object's vtable,
//! which `Voices` stands for.
//!
//! Player data (instance memory): `+0` bank base (written by the loader),
//! `+4` sample table (`u32 count`, then 12-byte entries), `+8` voice, `+12`
//! s8 state, `+13` previous state, `+14` u8 attribute count, `+15` u8 copy
//! position flag, `+17` u8 copy detail flag, `+20` entry index, `+24` input
//! state, `+28` attributes (12 bytes each: u8 id, `+4` last sent, `+8`
//! value), then the outputs the update copies back.

use crate::live::engine::Voices;
use crate::world::World;

/// `82B1BE30`: attribute ids 0..=8 are clamped per id; 9..=136 pass. Only
/// the ids the dry voices read (0, 2, 6, 7, 8) have an effect.
pub fn set_attribute(vs: &mut Voices, voice: u32, id: u32, value: u32) {
    if voice == 0 {
        return;
    }
    let v = value as i32;
    match id {
        0 | 6 | 7 => vs.set(voice, id, v.clamp(0, 65535) as u32),
        2 | 8 => vs.set(voice, id, v.clamp(0, 32767) as u32),
        _ => {}
    }
}

/// `82B1F440`.
pub fn release(w: &mut World, p: u32, vs: &mut Voices) -> u32 {
    let voice = w.mem.r32(p + 8);
    if voice != 0 {
        vs.release(voice);
        w.mem.w32(p + 8, 0);
    }
    if w.mem.r8(p + 15) != 0 {
        let n = w.mem.r8(p + 14) as u32;
        let at = p + 12 * n;
        w.mem.w32(at + 32, 0);
        w.mem.w32(at + 28, 0);
    }
    0
}

/// `82B1F4C8`: start a voice for the sample table entry at `e`.
pub fn start(w: &mut World, p: u32, e: u32, vs: &mut Voices) -> u32 {
    let voice = vs.create(&w.mem, p, e);
    if voice == 0 {
        release(w, p, vs);
        return 0;
    }
    let n = w.mem.r8(p + 14) as u32;
    for k in 0..n {
        let a = p + 28 + 12 * k;
        let id = w.mem.r8(a) as u32;
        let value = w.mem.r32(a + 8);
        set_attribute(vs, voice, id, value);
        let value = w.mem.r32(a + 8);
        w.mem.w32(a + 4, value);
    }
    voice
}

/// `82B1F5F8`: push changed attributes, poll the voice, copy its outputs.
/// Returns 1 while the voice lives, else releases and returns 0.
pub fn update(w: &mut World, p: u32, vs: &mut Voices) -> u32 {
    let n = w.mem.r8(p + 14) as u32;
    let mut a = p + 28;
    for _ in 0..n {
        let value = w.mem.r32(a + 8);
        if value != w.mem.r32(a + 4) {
            let id = w.mem.r8(a) as u32;
            let voice = w.mem.r32(p + 8);
            set_attribute(vs, voice, id, value);
            let value = w.mem.r32(a + 8);
            w.mem.w32(a + 4, value);
        }
        a += 12;
    }
    let voice = w.mem.r32(p + 8);
    let mut out = [0u32; 11];
    out[0] = vs.alive(voice) as u32;
    if out[0] == 0 {
        return release(w, p, vs);
    }
    let mut at = a;
    if w.mem.r8(p + 15) != 0 {
        w.mem.w32(a + 4, out[2]);
        w.mem.w32(a, out[1]);
        at = a + 8;
    }
    if w.mem.r8(p + 17) != 0 {
        for k in 0..8 {
            w.mem.w32(at + 4 * k, out[3 + k as usize]);
        }
    }
    1
}

/// Op 27 (`82B1D240`): the input state (clamped to 0 stop, 1 play, 2 hold)
/// drives the voice; returns the player state (the update's result while
/// playing).
pub fn op27_player(w: &mut World, p: u32, vs: &mut Voices) -> u32 {
    let input = w.mem.ri32(p + 24);
    let mut state: i32 = input.clamp(0, 2);
    let old = w.mem.r8(p + 12) as i8 as i32;
    if state != old {
        let voice = w.mem.r32(p + 8);
        if state == 0 {
            if voice != 0 {
                vs.release(voice);
                w.mem.w32(p + 8, 0);
                release(w, p, vs);
            }
        } else if state == 1 && voice == 0 && !(old == 2 && w.mem.r8(p + 13) == 1) {
            let table = w.mem.r32(p + 4);
            let count = w.mem.ri32(table);
            let mut i = w.mem.ri32(p + 20);
            if !(i < count) {
                i = count - 1;
            } else {
                // rlwinm sign bit; -1 → and: negative indices become 0.
                i &= ((i as u32 >> 31) as i32).wrapping_sub(1);
            }
            let e = table.wrapping_add(12u32.wrapping_mul(i as u32)) + 4;
            if w.mem.r16(e) == 0xFFFF {
                w.mem.w32(p + 8, 0);
                release(w, p, vs);
            } else {
                let v = start(w, p, e, vs);
                w.mem.w32(p + 8, v);
            }
        }
        let prev = w.mem.r8(p + 12);
        w.mem.w8(p + 12, state as u8);
        w.mem.w8(p + 13, prev);
    }
    if state == 1 && w.mem.r32(p + 8) != 0 {
        state = update(w, p, vs) as i32;
    }
    if w.mem.r32(p + 8) == 0 {
        return 0;
    }
    state as u32
}

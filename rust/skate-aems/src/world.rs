//! The AEMS world: loaded symbol files and banks, sound objects, instances,
//! listener dispatch and the control tick. Each function names the TU3
//! routine it reproduces; the PowerPC is in `local/re/aems_glue.txt` and
//! `local/re/aems_handlers.txt`, the layout in `docs/AEMS.md`.
//!
//! Listener callbacks are stored in memory as the game's own function
//! addresses and dispatched by value with the registers each call site
//! passes (`r3`, `r4`, `r5`), so a callback reached from an unusual list
//! behaves as it would in the game.

use crate::mem::{Kind, Memory};
use crate::ops;
use crate::rng::Rng;
use crate::live::engine::Voices;

/// Callback addresses the game stores in listener nodes.
pub mod cb {
    /// Bank record (`r3` object, `r5` record): instantiate on object create.
    pub const RECORD_ON_CREATE: u32 = 0x82B1_DAD0;
    /// Parameter binding: `*(r4 + 24) = *r3`.
    pub const PARAM_VALUE: u32 = 0x82B1_D7E8;
    /// Event binding: `*(r4 + 16) = 1`.
    pub const EVENT: u32 = 0x82B1_D7F8;
    /// Message binding: copy `n` (byte `r4 + 16`) words from `r3` to `r4 + 20`.
    pub const MESSAGE: u32 = 0x82B1_D808;
    /// Subscription: copy `n` (byte `r4 + 24`) words from `r3` to `r4 + 28`,
    /// then byte `r4 + 25 = 1`.
    pub const SUBSCRIPTION: u32 = 0x82B1_D840;
}

/// Error codes the game's reference checks return.
pub const ERR_STALE: i32 = -3;
pub const ERR_EMPTY: i32 = -4;
pub const ERR_NOT_FOUND: i32 = -5;
pub const ERR_NULL: i32 = -6;

pub struct World {
    pub mem: Memory,
    pub rng: Rng,
    /// Head of the loaded `.csi` list (`0x830BBE50`); nodes at file `+32`.
    csi_head: u32,
    /// Loaded banks in load order.
    pub banks: Vec<u32>,
    /// Address of the active program list head word (`0x83036F4C`).
    pub active: u32,
    /// Bank list head word (`0x83036F54`); nodes at bank `+80`.
    bank_head: u32,
    /// Symbol id counter (`0x83083CD0`, s16).
    symbol_ids: u16,
    /// Bank id counter (`0x830CFFC8`).
    bank_ids: i32,
    /// Tick state (`0x830775DC` last frame time, `0x830775E0` frames per
    /// tick, `0x830775E4` countdown) and the published tick length in ms
    /// (`0x830775D8`).
    last_dt: f32,
    frames_per_tick: i32,
    countdown: i32,
    pub tick_ms: f32,
    /// Port convenience: how many times the programs have run (`tick`).
    pub program_ticks: u64,
    /// Instances freed during a tick. The game's freed block stays readable
    /// until reused, and the program that ended itself (op 4) keeps running
    /// its remaining modules on it; the port releases the memory after the
    /// tick instead.
    freed: Vec<u32>,
}

impl Default for World {
    fn default() -> Self {
        let mut mem = Memory::default();
        let globals = mem.alloc_zeroed(Kind::Symbol, 16);
        World {
            mem,
            rng: Rng::default(),
            csi_head: globals,
            banks: Vec::new(),
            active: globals + 4,
            bank_head: globals + 8,
            symbol_ids: 0,
            bank_ids: 0,
            last_dt: 0.0,
            frames_per_tick: 0,
            countdown: 0,
            tick_ms: 0.0,
            program_ticks: 0,
            freed: Vec::new(),
        }
    }
}

impl World {
    /// `0x828E2818`: load a `.csi`: pointer-ize names, number ids from the
    /// global counter, push on the global list.
    pub fn load_csi(&mut self, file: &[u8]) -> Result<u32, String> {
        if file.len() < 40 || &file[..4] != b"MOIR" {
            return Err("not a MOIR .csi".into());
        }
        let base = self.mem.alloc(Kind::Symbol, file.to_vec());
        let m = &mut self.mem;
        let na = m.r16(base + 10) as u32;
        let nb = m.r16(base + 12) as u32;
        let nc = m.r16(base + 14) as u32;
        let a = base + 40;
        let b = a + 12 * na;
        let c = b + 12 * nb;
        m.w32(base + 20, a);
        m.w32(base + 24, b);
        m.w32(base + 28, c);
        let mut id = self.symbol_ids;
        let number = |id: &mut u16| {
            let mut next = id.wrapping_add(1) as i16;
            if next < 0 {
                next = 1;
            }
            *id = next as u16;
        };
        for (table, count, stride, name, slot) in [(a, na, 12, 4, 10), (b, nb, 12, 4, 10), (c, nc, 16, 8, 14)] {
            for k in 0..count {
                let e = table + stride * k;
                let off = m.r32(e + name);
                m.w32(e + name, off.wrapping_add(base));
                number(&mut id);
                m.w16(e + slot, id);
            }
        }
        self.symbol_ids = id;
        let node = base + 32;
        let first = m.r32(self.csi_head);
        m.w32(node + 4, 0);
        m.w32(node, first);
        if first != 0 {
            m.w32(first + 4, node);
        }
        m.w32(self.csi_head, node);
        Ok(base)
    }

    fn cstr(&self, addr: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut a = addr;
        loop {
            let c = self.mem.r8(a);
            if c == 0 {
                return out;
            }
            out.push(c);
            a += 1;
        }
    }

    /// `0x828E3358` (kind 0 → parameters), `0x828E3250` (kind 1 → classes),
    /// `0x828E3148` (any other kind → messages): walk the `.csi` list,
    /// first matching the module, then any module; match the 16-bit hash,
    /// then the full name. Writes `{entry, id word}` to `out` on success.
    pub fn resolve(&mut self, kind: u8, name: u32, module: u16, hash: u16, out: u32) -> i32 {
        let (table_off, count_off, stride, first, id_off) = match kind {
            0 => (28, 14, 16, 8, 12),
            1 => (24, 12, 12, 4, 8),
            _ => (20, 10, 12, 4, 8),
        };
        let want = self.cstr(name);
        for any_module in [false, true] {
            let mut node = self.mem.r32(self.csi_head);
            while node != 0 {
                let f = node - 32;
                if any_module || self.mem.r16(f + 16) == module {
                    let table = self.mem.r32(f + table_off);
                    let count = self.mem.r16(f + count_off) as u32;
                    for k in 0..count {
                        let e = table + stride * k;
                        if self.mem.r16(e + first + 4) != hash {
                            continue;
                        }
                        if self.cstr(self.mem.r32(e + first)) == want {
                            let id = self.mem.r32(e + id_off);
                            self.mem.w32(out, e);
                            self.mem.w32(out + 4, id);
                            return 0;
                        }
                    }
                }
                node = self.mem.r32(node);
            }
        }
        ERR_NOT_FOUND
    }

    /// Bank load: the caller's part of `0x828DC660` (bank id at `+60`,
    /// sample table pointer at `+64`), then `0x82B1DF50` with the relocation
    /// tables at their file offsets.
    pub fn load_bank(&mut self, file: &[u8]) -> Result<u32, String> {
        if file.len() < 96 || &file[..4] != b"ABKC" {
            return Err("not an ABKC bank".into());
        }
        let bank = self.mem.alloc(Kind::Bank, file.to_vec());
        let m = &mut self.mem;
        // 0x828DC660
        self.bank_ids = self.bank_ids.wrapping_add(1);
        if self.bank_ids < 0 {
            self.bank_ids = 1;
        }
        m.wi32(bank + 60, self.bank_ids);
        let samples = m.r32(bank + 32);
        if samples != 0 {
            m.w32(bank + 64, samples.wrapping_add(bank));
        }
        // 0x82B1DF50: bank list.
        let node = bank + 80;
        let first = m.r32(self.bank_head);
        m.w32(bank + 84, 0);
        m.w32(node, first);
        if first != 0 {
            m.w32(first + 4, node);
        }
        m.w32(self.bank_head, node);
        m.w32(bank + 68, 0);
        // Handler relocations: field = handler - field - 4.
        let fns = bank + m.r32(bank + 48);
        let n = m.ri32(fns);
        for k in 0..n.max(0) as u32 {
            let off = m.r32(fns + 4 + 4 * k);
            let field = off.wrapping_add(bank);
            let index = m.r32(field) as usize;
            let handler = *ops::HANDLERS.get(index).ok_or_else(|| format!("handler index {index} at {off:#x}"))?;
            m.w32(field, handler.wrapping_sub(field).wrapping_sub(4));
        }
        // Pointer relocations: field += bank.
        let ptrs = bank + m.r32(bank + 52);
        let n = m.ri32(ptrs);
        for k in 0..n.max(0) as u32 {
            let off = m.r32(ptrs + 4 + 4 * k);
            let v = m.r32(off.wrapping_add(bank));
            m.w32(off.wrapping_add(bank), v.wrapping_add(bank));
        }
        // Imports: {target, name record, kind}; record {u16 module, u16 hash, name}.
        let imps = bank + m.r32(bank + 56);
        let n = m.ri32(imps);
        for k in 0..n.max(0) as u32 {
            let e = imps + 4 + 12 * k;
            let target = bank.wrapping_add(self.mem.r32(e));
            let rec = bank.wrapping_add(self.mem.r32(e + 4));
            let kind = self.mem.r8(e + 8);
            let module = self.mem.r16(rec);
            let hash = self.mem.r16(rec + 2);
            self.resolve(kind, rec + 4, module, hash, target);
        }
        // Records.
        let m = &mut self.mem;
        let count = m.r16(bank + 10) as u32;
        let mut rec = bank + m.r32(bank + 28);
        for _ in 0..count {
            let code = m.r32(rec + 40).wrapping_add(bank);
            let data = m.r32(rec + 44).wrapping_add(bank);
            m.w32(rec + 24, rec);
            m.w32(rec + 40, code);
            m.w32(rec + 20, cb::RECORD_ON_CREATE);
            m.w32(rec + 44, data);
            let id = m.ri32(rec + 8);
            let class = m.r32(rec + 4);
            if id >= 0 && class != 0 {
                if id != m.ri32(class + 8) {
                    m.wi32(rec + 8, ERR_STALE);
                    m.w32(rec + 4, 0);
                } else {
                    m.list_push(class, rec + 12);
                }
            }
            let objects = m.r8(rec + 36) as u32;
            for k in 0..objects {
                let off = m.r32(rec + 60 + 4 * k);
                let template = m.r32(rec + 44);
                m.w32(off.wrapping_add(template), bank);
            }
            let extra = m.r8(rec + 39) as u32;
            rec += 60 + 4 * (extra + objects);
        }
        m.w32(bank + 72, 0);
        self.banks.push(bank);
        Ok(bank)
    }

    /// An indirect listener call with the game's argument registers.
    fn call(&mut self, f: u32, r3: u32, r4: u32, r5: u32, vs: &mut Voices) {
        let m = &mut self.mem;
        match f {
            cb::PARAM_VALUE => {
                let v = m.r32(r3);
                m.w32(r4 + 24, v);
            }
            cb::EVENT => m.w32(r4 + 16, 1),
            cb::MESSAGE => {
                let mut k = 0u32;
                while k < m.r8(r4 + 16) as u32 {
                    let v = m.r32(r3 + 4 * k);
                    m.w32(r4 + 20 + 4 * k, v);
                    k += 1;
                }
            }
            cb::SUBSCRIPTION => {
                let mut k = 0u32;
                while k < m.r8(r4 + 24) as u32 {
                    let v = m.r32(r3 + 4 * k);
                    m.w32(r4 + 28 + 4 * k, v);
                    k += 1;
                }
                m.w8(r4 + 25, 1);
            }
            cb::RECORD_ON_CREATE => self.record_on_create(r3, r5, vs),
            // Listeners the host registered are not ported: nothing to call.
            _ => {}
        }
    }

    /// Walks the list at `head` calling each listener. `args(ctx)` gives
    /// `(r3, r4, r5)` for a node's context word.
    fn notify(&mut self, head: u32, vs: &mut Voices, args: impl Fn(u32) -> (u32, u32, u32)) {
        let mut node = self.mem.r32(head);
        while node != 0 {
            let f = self.mem.r32(node + 8);
            let ctx = self.mem.r32(node + 12);
            let (r3, r4, r5) = args(ctx);
            self.call(f, r3, r4, r5, vs);
            node = self.mem.r32(node);
        }
    }

    /// The reference check every entry point starts with: `id < 0` returns
    /// it, a null object `-6`, an id mismatch clears the reference and
    /// returns `-3`.
    fn check(&mut self, reference: u32, id_off: u32) -> Result<u32, i32> {
        let id = self.mem.ri32(reference + 4);
        if id < 0 {
            return Err(id);
        }
        let obj = self.mem.r32(reference);
        if obj == 0 {
            return Err(ERR_NULL);
        }
        if id != self.mem.ri32(obj + id_off) {
            self.mem.w32(reference, 0);
            self.mem.wi32(reference + 4, ERR_STALE);
            return Err(ERR_STALE);
        }
        Ok(obj)
    }

    /// `0x828E2B48(classref, params, out)`: create a sound object; class
    /// listeners get `(object, params, ctx)`, then the object's own message
    /// listeners `(params, ctx)`.
    pub fn create_object(&mut self, classref: u32, params: u32, out: u32, vs: &mut Voices) -> i32 {
        self.mem.w32(out, 0);
        let class = match self.check(classref, 8) {
            Ok(c) => c,
            Err(e) => return e,
        };
        let obj = self.mem.alloc_zeroed(Kind::Object, 16);
        self.mem.w32(obj + 4, 1);
        self.mem.w32(obj, class);
        self.notify(class, vs, |ctx| (obj, params, ctx));
        self.notify(obj + 8, vs, |ctx| (params, ctx, 0));
        self.mem.w32(out, obj);
        0
    }

    /// `0x82B1DAD0`: a record hears a new object of its class.
    fn record_on_create(&mut self, obj: u32, rec: u32, vs: &mut Voices) {
        let count = self.mem.r16(rec + 28) as i16;
        let limit = self.mem.r16(rec + 30) as i16;
        if count >= limit {
            return;
        }
        let inst = self.instantiate(obj, rec, vs);
        let node = inst + 8;
        let first = self.mem.r32(self.active);
        self.mem.w32(inst + 12, 0);
        self.mem.w32(inst + 8, first);
        if first != 0 {
            self.mem.w32(first + 4, node);
        }
        self.mem.w32(self.active, node);
    }

    /// `0x82B1D880`: instantiate a record's patch for a sound object.
    fn instantiate(&mut self, obj: u32, rec: u32, vs: &mut Voices) -> u32 {
        let m = &mut self.mem;
        let size = m.r32(rec + 48);
        let template = m.r32(rec + 44);
        let bytes = m.bytes(template, size).to_vec();
        let inst = m.alloc(Kind::Instance, bytes);
        let hdr = inst.wrapping_add(m.r32(rec + 52));
        m.w32(hdr, rec);
        m.w32(hdr + 4, inst);
        m.w32(hdr + 8, obj);
        let first = m.r32(rec + 56);
        m.w32(inst, first);
        m.w32(inst + 4, 0);
        if first != 0 {
            m.w32(first + 4, inst);
        }
        m.w32(rec + 56, inst);
        let mut p = inst + 24;
        let code = m.r32(rec + 40);
        m.w32(inst + 16, code);
        m.w32(inst + 20, p);
        if m.r8(rec + 37) != 0 {
            m.w32(p + 12, p);
            m.w32(p + 8, cb::EVENT);
            m.list_push(obj + 12, p);
            p += 20;
            let rc = m.r32(obj + 4);
            m.w32(obj + 4, rc.wrapping_add(1));
        }
        let mut k = 0u32;
        while k < self.mem.r16(rec + 32) as u32 {
            self.mem.w32(p + 20, p);
            self.mem.w32(p + 16, cb::PARAM_VALUE);
            self.bind_param(p, p + 8, vs);
            k += 1;
            p += 28;
        }
        let m = &mut self.mem;
        let mut q = p;
        if m.r8(rec + 38) != 0 {
            m.w32(p + 12, p);
            m.w32(p + 8, cb::MESSAGE);
            m.list_push(obj + 8, p);
            let rc = m.r32(obj + 4);
            m.w32(obj + 4, rc.wrapping_add(1));
            q = p + 4 * (m.r8(p + 16) as u32 + 5);
        }
        let mut k = 0u32;
        while k < m.r16(rec + 34) as u32 {
            let id = m.ri32(q + 4);
            m.w32(q + 20, q);
            m.w32(q + 16, cb::SUBSCRIPTION);
            let target = m.r32(q);
            if id >= 0 && target != 0 {
                if id != m.ri32(target + 8) {
                    m.wi32(q + 4, ERR_STALE);
                    m.w32(q, 0);
                } else {
                    m.list_push(target, q + 8);
                }
            }
            k += 1;
            q += 4 * (m.r8(q + 24) as u32 + 7);
        }
        let c = m.r16(rec + 28);
        m.w16(rec + 28, c.wrapping_add(1));
        inst
    }

    /// `0x828E2FF8(paramref, node)`: link a parameter binding and push the
    /// current value through it `(param + 4, ctx)`.
    fn bind_param(&mut self, reference: u32, node: u32, vs: &mut Voices) -> i32 {
        let param = match self.check(reference, 12) {
            Ok(p) => p,
            Err(e) => return e,
        };
        self.mem.list_push(param, node);
        let ctx = self.mem.r32(node + 12);
        let f = self.mem.r32(node + 8);
        self.call(f, param + 4, ctx, 0, vs);
        0
    }

    /// `0x828E2F38(paramref, &value)`: store a changed value and call the
    /// listeners `(param + 4, ctx)`.
    pub fn set_param(&mut self, reference: u32, value: u32, vs: &mut Voices) -> i32 {
        let param = match self.check(reference, 12) {
            Ok(p) => p,
            Err(e) => return e,
        };
        if value == self.mem.r32(param + 4) {
            return 0;
        }
        self.mem.w32(param + 4, value);
        self.notify(param, vs, |ctx| (param + 4, ctx, 0));
        0
    }

    /// `0x828E29C0(msgref, words)`: call every listener `(words, ctx)`.
    pub fn send_message(&mut self, reference: u32, words: u32, vs: &mut Voices) -> i32 {
        let msg = match self.check(reference, 8) {
            Ok(m) => m,
            Err(e) => return e,
        };
        if self.mem.r32(msg) == 0 {
            return ERR_EMPTY;
        }
        self.notify(msg, vs, |ctx| (words, ctx, 0));
        0
    }

    /// `0x828E2D18(object, words)`: the object's message listeners.
    pub fn update_object(&mut self, obj: u32, words: u32, vs: &mut Voices) -> i32 {
        self.notify(obj + 8, vs, |ctx| (words, ctx, 0));
        0
    }

    /// `0x828E2C78(object)`: event listeners `(object, ctx)`, then drop the
    /// creator's reference.
    pub fn release_object(&mut self, obj: u32, vs: &mut Voices) -> i32 {
        self.notify(obj + 12, vs, |ctx| (obj, ctx, 0));
        self.unref(obj);
        0
    }

    fn unref(&mut self, obj: u32) {
        let rc = self.mem.r32(obj + 4).wrapping_sub(1);
        self.mem.w32(obj + 4, rc);
        if rc == 0 {
            self.mem.free(obj);
        }
    }

    /// `0x828E2E08` (list `+12`) / `0x828E2EA0` (list `+8`): unlink an
    /// object listener and drop its reference.
    fn unlisten_object(&mut self, obj: u32, list: u32, node: u32) {
        self.mem.list_remove(obj + list, node);
        self.unref(obj);
    }

    /// `0x828E30B8` (parameter, id at `+12`) / `0x828E2D78` (class or
    /// message, id at `+8`): checked unlink of a binding's node.
    fn unlisten_ref(&mut self, reference: u32, id_off: u32, node: u32) -> i32 {
        let target = match self.check(reference, id_off) {
            Ok(t) => t,
            Err(e) => return e,
        };
        self.mem.list_remove(target, node);
        0
    }

    /// `0x82B1BF98(header)`: undo the bindings, stop player voices (voice
    /// slot 0), release child objects, free the instance.
    pub fn free_instance(&mut self, hdr: u32, vs: &mut Voices) {
        let rec = self.mem.r32(hdr);
        let inst = self.mem.r32(hdr + 4);
        let obj = self.mem.r32(hdr + 8);
        let mut p = inst + 24;
        if self.mem.r8(rec + 37) != 0 {
            self.unlisten_object(obj, 12, p);
            p += 20;
        }
        let mut k = 0u32;
        while k < self.mem.r16(rec + 32) as u32 {
            self.unlisten_ref(p, 12, p + 8);
            k += 1;
            p += 28;
        }
        let mut q = p;
        if self.mem.r8(rec + 38) != 0 {
            self.unlisten_object(obj, 8, p);
            q = p + 4 * (self.mem.r8(p + 16) as u32 + 5);
        }
        let mut k = 0u32;
        while k < self.mem.r16(rec + 34) as u32 {
            self.unlisten_ref(q, 8, q + 8);
            k += 1;
            q += 4 * (self.mem.r8(q + 24) as u32 + 7);
        }
        let objects = self.mem.r8(rec + 36) as u32;
        for k in 0..objects {
            let player = self.mem.r32(rec + 60 + 4 * k).wrapping_add(inst);
            let voice = self.mem.r32(player + 8);
            if voice != 0 {
                vs.release(voice);
                self.mem.w32(player + 8, 0);
            }
        }
        let extra = self.mem.r8(rec + 39) as u32;
        for k in 0..extra {
            let at = self.mem.r32(rec + 60 + 4 * (objects + k)).wrapping_add(inst);
            let child = self.mem.r32(at + 8);
            if child != 0 {
                self.release_object(child, vs);
            }
        }
        let c = self.mem.r16(rec + 28);
        self.mem.w16(rec + 28, c.wrapping_sub(1));
        self.freed.push(inst);
    }

    /// `0x82B1E290(dt)`: called once per audio frame with the frame time in
    /// seconds; every `n` frames (first `n` with `n·dt ≥ 1/30 s`) runs every
    /// active program once.
    pub fn tick(&mut self, dt: f32, vs: &mut Voices) {
        let mut countdown;
        let frames;
        if dt != self.last_dt {
            self.last_dt = dt;
            // 0x8231A844 / 0x82FD35F4; accumulator from 0x82165A10.
            let period = 1.0f32 / 30.0f32;
            let mut acc = 0.0f32;
            let mut n = 0i32;
            loop {
                acc += dt;
                n += 1;
                if !(acc + dt < period) {
                    break;
                }
            }
            self.frames_per_tick = n;
            frames = n;
            // 0x82256FE8 = 1000.0
            self.tick_ms = n as f32 * dt * 1000.0;
            countdown = n;
        } else {
            countdown = self.countdown;
            frames = self.frames_per_tick;
        }
        countdown = countdown.wrapping_sub(1);
        self.countdown = countdown;
        if countdown != 0 {
            return;
        }
        self.countdown = frames;
        self.program_ticks += 1;
        let mut node = self.mem.r32(self.active);
        while node != 0 {
            let code = self.mem.r32(node + 8);
            let next = self.mem.r32(node);
            let data = self.mem.r32(node + 12);
            ops::run(self, code, data, vs);
            node = next;
        }
        for inst in std::mem::take(&mut self.freed) {
            self.mem.free(inst);
        }
    }

    /// Port convenience (no game counterpart): find a loaded symbol by name
    /// and write its `{entry, id word}` reference to a new 8-byte cell, the
    /// form every game entry point takes. Kinds as imports: 0 parameter,
    /// 1 class, 2 message.
    pub fn reference(&mut self, kind: u8, name: &str) -> Option<u32> {
        let (table_off, count_off, stride, first, id_off) = match kind {
            0 => (28, 14, 16, 8, 12),
            1 => (24, 12, 12, 4, 8),
            _ => (20, 10, 12, 4, 8),
        };
        let mut node = self.mem.r32(self.csi_head);
        while node != 0 {
            let f = node - 32;
            let table = self.mem.r32(f + table_off);
            for k in 0..self.mem.r16(f + count_off) as u32 {
                let e = table + stride * k;
                if self.cstr(self.mem.r32(e + first)) == name.as_bytes() {
                    let id = self.mem.r32(e + id_off);
                    let cell = self.mem.alloc_zeroed(Kind::Object, 8);
                    self.mem.w32(cell, e);
                    self.mem.w32(cell + 4, id);
                    return Some(cell);
                }
            }
            node = self.mem.r32(node);
        }
        None
    }
}

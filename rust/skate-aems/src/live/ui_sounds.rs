//! Skate 3's frontend sound events during play: the event lookup
//! (`0x824955B8`), the 10-entry queue (`0x82495828`) and its per-frame
//! player (`0x824958F0`), and the combo multiplier trigger in the HUD update
//! (`0x82666BC0`).
//!
//! An event is a collection of class `0x5831CB95F3E90598` keyed by the
//! hash of its name; its `sk8_menu` field is the sound id in splice slot 5
//! and its float field the gain. The HUD posts `multiplyer_2` /
//! `multiplyer_3` when the line multiplier it shows (`+140`, the scoring
//! module's combo multiplier copied by `0x82DA4238` and `0x82775328`)
//! changes to exactly 2 or 3. Frontend sounds are not placed in the world:
//! they play on the non-positional emitter.

use super::frame::SkaterImage;
use super::splices::Splices;
use super::tuning::Tuning;
use super::EMITTER_SPEED;
use crate::splice::Params;

/// Event collection class (`0x82489A20` → `0x82B69B08`).
pub const EVENT_CLASS: u64 = 0x5831_CB95_F3E9_0598;
/// Event gain (layout +4).
const FIELD_GAIN: u64 = 0x875B_A753_41DC_8391;
/// `sk8_menu` sound id (layout +8).
const FIELD_MENU_ID: u64 = 0x8FCC_7EF9_B920_8858;
/// `multiplyer_2`, `multiplyer_3` (`0x82666C30..0x82666C6C`).
pub const MULTIPLYER_2: u64 = 0x1A67_C3E0_088A_C43D;
pub const MULTIPLYER_3: u64 = 0xE7F8_2666_1375_333F;
/// The splice slot `sk8_menu.bnk` loads into (`0x82487A60`).
pub const SLOT_MENU: usize = 5;
/// Host-only skater-image field: Skate's combo multiplier as the HUD shows
/// it (like offset 1000, not a native skater field).
pub const MULTIPLIER_FIELD: usize = 1004;
const QUEUE: usize = 10;
/// The slot 5 and 2 volume (`settings +40`, 0..32767, times `0x822F8898`).
/// Skate's options menu is not ported; the slider stands at its maximum.
fn options_volume() -> f32 {
    32767.0 * f32::from_bits(0x3800_0100)
}

#[derive(Clone, Copy, Default)]
struct Entry {
    /// +57: queued, not yet created.
    pending: bool,
    slot: usize,
    id: i32,
    gain: f32,
    handle: u32,
}

#[derive(Clone, Default)]
pub struct UiSounds {
    queue: [Entry; QUEUE],
    /// The HUD's last shown multiplier (`+52`).
    multiplier: f32,
}

impl UiSounds {
    /// One game frame: the HUD's multiplier check, then the queue.
    pub fn update(&mut self, s: &SkaterImage, t: &Tuning, splices: &mut Splices, dt: f32) {
        self.watch(t, s.rf(MULTIPLIER_FIELD));
        self.service(splices, dt);
    }

    /// `0x82666BC0`: the shown multiplier against the last one.
    fn watch(&mut self, t: &Tuning, m: f32) {
        if m != self.multiplier {
            if m == 2.0 {
                self.post(t, MULTIPLYER_2);
            } else if m == 3.0 {
                self.post(t, MULTIPLYER_3);
            }
        }
        self.multiplier = m;
    }

    /// `0x824955B8`: an event by name hash. Only the menu-bank sound is
    /// ported; the gameplay events carry no HOM id.
    pub fn post(&mut self, t: &Tuning, key: u64) {
        let c = t.collection(EVENT_CLASS, key);
        if c == 0 {
            return;
        }
        let gain = f32::from_bits(t.g32(t.attrib(c, FIELD_GAIN)));
        let id = t.g32(t.attrib(c, FIELD_MENU_ID)) as i32;
        if id != 0 && id > -1 {
            self.enqueue(SLOT_MENU, id, gain);
        }
    }

    /// `0x82495828`: the first free entry, if any; a silent event is dropped.
    fn enqueue(&mut self, slot: usize, id: i32, gain: f32) {
        if !(gain > 0.0) {
            return;
        }
        if let Some(e) = self.queue.iter_mut().find(|e| !e.pending && e.handle == 0) {
            *e = Entry { pending: true, slot, id, gain, handle: 0 };
        }
    }

    /// `0x824958F0`: create and play queued entries, update playing ones,
    /// free finished ones.
    fn service(&mut self, splices: &mut Splices, dt: f32) {
        let volume = options_volume();
        for e in &mut self.queue {
            let p = Params { gain: e.gain * volume, pitch: 1.0, pan: 0.0, dt: 0.0, pan_scale: 1.0, stretch: 1.0 };
            if e.pending {
                e.pending = false;
                e.handle = splices.create(e.slot, e.id);
                splices.set_emitter(e.handle, EMITTER_SPEED);
                splices.play(e.handle, 0, p);
            } else if e.handle != 0 {
                if splices.is_playing(e.handle) {
                    splices.update(e.handle, Params { dt, ..p });
                } else {
                    splices.destroy(e.handle);
                    e.handle = 0;
                }
            }
        }
    }

    /// Skate mode ended: the engine has stopped every splice instance.
    pub fn reset(&mut self) {
        self.queue = Default::default();
    }

    /// Entries queued or sounding.
    #[cfg(test)]
    pub fn busy(&self) -> usize {
        self.queue.iter().filter(|e| e.pending || e.handle != 0).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuning() -> Tuning {
        let mut t = Tuning::default();
        for (key, id, gain) in [(MULTIPLYER_2, 7, 0.5f32), (MULTIPLYER_3, 8, 0.25)] {
            let c = t.set_collection(EVENT_CLASS, key);
            t.set_attrib(c, FIELD_MENU_ID, id);
            t.set_attrib(c, FIELD_GAIN, gain.to_bits());
        }
        t
    }

    fn at(m: f32) -> SkaterImage {
        let mut s = SkaterImage::default();
        s.wf(MULTIPLIER_FIELD, m);
        s
    }

    #[test]
    fn multiplier_posts_only_on_a_change_to_two_or_three() {
        let t = tuning();
        let mut ui = UiSounds::default();
        let mut posted = Vec::new();
        for m in [1.0, 1.5, 2.0, 2.0, 3.0, 3.0, 1.0, 2.0] {
            let before = ui.queue;
            ui.watch(&t, m);
            let new: Vec<_> = ui.queue.iter().zip(before.iter())
                .filter(|(a, b)| a.pending && !b.pending).map(|(a, _)| (a.id, a.gain)).collect();
            posted.extend(new);
        }
        assert_eq!(posted, [(7, 0.5), (8, 0.25), (7, 0.5)]);
        assert_eq!(at(3.0).rf(MULTIPLIER_FIELD), 3.0);
    }

    #[test]
    fn queue_holds_ten_and_drops_silent_or_unknown_events() {
        let t = tuning();
        let mut ui = UiSounds::default();
        ui.post(&t, 0x1234);
        ui.enqueue(SLOT_MENU, 3, 0.0);
        assert_eq!(ui.busy(), 0);
        for _ in 0..12 {
            ui.post(&t, MULTIPLYER_2);
        }
        assert_eq!(ui.busy(), QUEUE);
    }

    #[test]
    fn combo_multiplier_sounds_play_from_the_menu_bank() {
        use std::path::Path;
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/audio-cache");
        let t = match Tuning::load(&dir) { Ok(t) => t, Err(e) => { eprintln!("SKIP {e}"); return } };
        let mut sp = Splices::default();
        sp.load(&dir, SLOT_MENU, "sk8_menu").unwrap();
        let mut ui = UiSounds::default();
        let dt = 1.0 / 60.0;
        for m in [1.0, 2.0, 3.0] {
            ui.update(&at(m), &t, &mut sp, dt);
            sp.service();
            if m == 1.0 {
                assert_eq!(ui.busy(), 0);
                continue;
            }
            assert_eq!(ui.busy(), 1, "multiplier {m}");
            let (mut energy, mut frames, mut now) = (0.0f64, 0, 0.0);
            while ui.busy() > 0 && frames < 600 {
                let mut out = [[0.0; crate::eac::BLOCK]; super::super::EMITTERS];
                sp.render_emitters(now, &mut out);
                assert!(out[super::super::EMITTER_BOARD].iter().all(|x| *x == 0.0));
                energy += out[EMITTER_SPEED].iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
                now += crate::eac::BLOCK as f64 / 48_000.0;
                ui.update(&at(m), &t, &mut sp, dt);
                sp.service();
                frames += 1;
            }
            assert!(energy > 0.0, "multiplier {m} was silent");
            assert_eq!(ui.busy(), 0, "multiplier {m} did not finish");
        }
        // multiplyer_2's second layer starts 0.18 s into its sample: splice
        // start offsets (SndPlayer1 seeks) are not ported yet.
        assert!(sp.problems.iter().all(|p| p.ends_with("start offsets are not ported")), "{:?}", sp.problems);
    }

    #[test]
    fn an_unloaded_bank_frees_the_entry() {
        let t = tuning();
        let mut ui = UiSounds::default();
        let mut splices = Splices::default();
        ui.update(&at(2.0), &t, &mut splices, 1.0 / 60.0);
        assert_eq!(ui.busy(), 0);
    }
}

//! Keys and mouse into a controller, frame by frame.

use crate::controls::{ACTIONS, Action, Controls, Gesture, MouseTrick, Numpad, numpad_of, numpad_vector};

/// A controller as Skate reads it: XInput button bits, triggers 0-255 and
/// sticks from -1 to 1 (x right, y up).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pad {
    pub buttons: u16,
    pub triggers: [u8; 2],
    pub left: [f32; 2],
    pub right: [f32; 2],
}

fn length(v: [f32; 2]) -> f32 {
    v[0].hypot(v[1])
}

impl Pad {
    /// Both inputs at once: buttons and triggers combined, each stick from
    /// whichever is pushed further.
    pub fn merge(self, other: Self) -> Self {
        let stick = |a: [f32; 2], b: [f32; 2]| if length(b) > length(a) { b } else { a };
        Self {
            buttons: self.buttons | other.buttons,
            triggers: [self.triggers[0].max(other.triggers[0]), self.triggers[1].max(other.triggers[1])],
            left: stick(self.left, other.left),
            right: stick(self.right, other.right),
        }
    }

    /// The sticks as XInput's signed axes.
    pub fn axes(&self) -> ([i16; 2], [i16; 2]) {
        let axis = |v: f32| (v.clamp(-1., 1.) * 32767.).round() as i16;
        (self.left.map(axis), self.right.map(axis))
    }

    pub fn is_idle(&self) -> bool {
        self.buttons == 0 && self.triggers == [0, 0] && length(self.left) < 0.01 && length(self.right) < 0.01
    }
}

/// How long a session-marker key holds LB before the D-pad joins it.
const MARKER_DELAY: f32 = 0.05;
/// How long the trick stick stays up after the pop key is let go.
const POP_FLICK_MS: f32 = 120.;

/// Turns held keys and mouse motion into a [`Pad`]: the left stick ramps
/// like a thumb, the trick stick travels between directions at flick speed
/// (so 2-3-6 sweeps round like a real quarter circle), diagonals are lenient
/// the way SF6 reads a keyboard, and one-key tricks play their motion.
#[derive(Debug, Default)]
pub struct Translator {
    left: [f32; 2],
    trick: [f32; 2],
    /// The trick direction keys say now, and since when.
    raw: Numpad,
    raw_since: f32,
    /// Whether the last change of the trick keys only let keys go.
    released: bool,
    trick_keys: Vec<usize>,
    /// The direction the trick stick heads for.
    shown: Numpad,
    gesture: Option<(Gesture, usize, f32)>,
    was_down: Vec<bool>,
    mouse: [f32; 2],
    mouse_still: f32,
    clock: f32,
    view_pressed: bool,
    /// Whether the mouse moved the trick stick last frame.
    mouse_on: bool,
    /// The pop key: held last frame, and the flick after letting go (its
    /// direction and the milliseconds it has left).
    pop_held: bool,
    pop_flick: Option<(Numpad, f32)>,
    /// Seconds each action's keys have been held.
    held_for: Vec<f32>,
}

impl Translator {
    pub fn new() -> Self {
        Self {
            raw: 5,
            shown: 5,
            ..Default::default()
        }
    }

    /// Whether the first/third person key went down this frame.
    pub fn view_pressed(&self) -> bool {
        self.view_pressed
    }

    /// Whether the mouse works the trick stick now (its key held, or always):
    /// otherwise it is free, for the camera.
    pub fn mouse_tricks(&self) -> bool {
        self.mouse_on
    }

    /// Lets go of everything (skating stopped, a menu opened).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// One frame of `dt` seconds. `pressed` says whether a key (by name) is
    /// held; `mouse` is this frame's mouse motion in counts, y down.
    pub fn update(&mut self, controls: &Controls, pressed: impl Fn(&str) -> bool, mouse: [f32; 2], dt: f32) -> Pad {
        let dt = dt.clamp(0., 0.1);
        self.clock += dt;
        let down: Vec<bool> = ACTIONS
            .iter()
            .enumerate()
            .map(|(i, (action, _, _, _))| controls.uses(*action) && controls.keys[i].iter().any(|k| pressed(k)))
            .collect();
        if self.was_down.len() != down.len() {
            self.was_down = vec![false; down.len()];
        }
        if self.held_for.len() != down.len() {
            self.held_for = vec![0.; down.len()];
        }
        for (held, down) in self.held_for.iter_mut().zip(&down) {
            *held = if *down { *held + dt } else { 0. };
        }
        let mut pad = Pad::default();
        let (mut lean, mut trick) = ([false; 4], [false; 4]);
        let (mut manual, mut nose, mut mouse_held, mut pop) = (false, false, false, false);
        for (i, (action, _, _, _)) in ACTIONS.iter().enumerate() {
            if !down[i] {
                continue;
            }
            let pressed_now = !self.was_down[i];
            let flags = |flags: &mut [bool; 4], d: Numpad| {
                // up, down, left, right
                flags[0] |= matches!(d, 7..=9);
                flags[1] |= matches!(d, 1..=3);
                flags[2] |= matches!(d, 1 | 4 | 7);
                flags[3] |= matches!(d, 3 | 6 | 9);
            };
            match *action {
                Action::Stick(d) => flags(&mut lean, d),
                Action::Trick(d) => flags(&mut trick, d),
                Action::Manual => manual = true,
                Action::NoseManual => nose = true,
                Action::Gesture(g) if pressed_now => self.gesture = Some((g, 0, 0.)),
                Action::Gesture(_) => {}
                Action::Button(bit) => pad.buttons |= bit,
                Action::LeftTrigger => pad.triggers[0] = 255,
                Action::RightTrigger => pad.triggers[1] = 255,
                Action::MouseTrick => mouse_held = true,
                Action::Pop => pop = true,
                Action::CameraView => {}
                // Skate's session marker: LB held, then the D-pad, a moment
                // later so the game sees the modifier first.
                Action::Marker(dpad) => {
                    pad.buttons |= crate::controls::LB;
                    if self.held_for[i] >= MARKER_DELAY {
                        pad.buttons |= dpad;
                    }
                }
            }
        }
        self.view_pressed = ACTIONS
            .iter()
            .enumerate()
            .any(|(i, (action, _, _, _))| *action == Action::CameraView && down[i] && !self.was_down[i]);
        self.was_down = down;

        // The left stick ramps towards the keys' lean, and back quicker.
        let target = numpad_vector(numpad_of(lean[0], lean[1], lean[2], lean[3]));
        let rise = if controls.ramp_ms <= 0. { f32::INFINITY } else { dt * 1000. / controls.ramp_ms };
        for axis in 0..2 {
            let step = if target[axis] == 0. { rise * 2. } else { rise };
            let delta = target[axis] - self.left[axis];
            self.left[axis] += delta.clamp(-step, step);
        }

        // Trick keys, with release leniency: letting go of a diagonal's two
        // keys a moment apart does not flick the stick to the cardinal.
        let raw = numpad_of(trick[0], trick[1], trick[2], trick[3]);
        let trick_keys: Vec<usize> = ACTIONS
            .iter()
            .enumerate()
            .filter(|(i, (action, _, _, _))| matches!(action, Action::Trick(_)) && self.was_down.get(*i) == Some(&true))
            .map(|(i, _)| i)
            .collect();
        if raw != self.raw {
            self.released = trick_keys.iter().all(|k| self.trick_keys.contains(k));
            self.raw = raw;
            self.raw_since = self.clock;
        }
        self.trick_keys = trick_keys;
        let buffered = (self.clock - self.raw_since) * 1000. < controls.buffer_ms;
        let part_of_diagonal = |cardinal: Numpad, diagonal: Numpad| {
            matches!(diagonal, 1 | 3 | 7 | 9) && matches!(cardinal, 2 | 4 | 6 | 8) && {
                let [dx, dy] = numpad_vector(diagonal);
                let [cx, cy] = numpad_vector(cardinal);
                dx * cx + dy * cy > 0.5
            }
        };
        if !(buffered && self.released && part_of_diagonal(self.raw, self.shown)) {
            self.shown = self.raw;
        }

        let mouse_on = match controls.mouse {
            MouseTrick::Off => false,
            MouseTrick::Hold => mouse_held,
            MouseTrick::Always => true,
        };
        self.mouse_on = mouse_on;
        // Pop: down while held, then up (toward a held side) a moment.
        if self.pop_held && !pop {
            let side = if trick[2] { 7 } else if trick[3] { 9 } else { 8 };
            self.pop_flick = Some((side, POP_FLICK_MS));
        }
        self.pop_held = pop;
        let flick = self.pop_flick.as_mut().map(|(d, left)| {
            *left -= dt * 1000.;
            (*d, *left)
        });
        if flick.is_some_and(|(_, left)| left <= 0.) {
            self.pop_flick = None;
        }
        let mut direct = None;
        let target = if let Some((d, _)) = flick.filter(|(_, left)| *left > 0.) {
            numpad_vector(d)
        } else if pop {
            numpad_vector(2)
        } else if let Some((gesture, step, elapsed)) = self.gesture.as_mut() {
            let motion = gesture.motion();
            *elapsed += dt * 1000.;
            while *step < motion.len() && *elapsed >= motion[*step].1 as f32 {
                *elapsed -= motion[*step].1 as f32;
                *step += 1;
            }
            match motion.get(*step) {
                Some(&(d, _)) => numpad_vector(d),
                None => {
                    self.gesture = None;
                    [0., 0.]
                }
            }
        } else if mouse_on {
            let moved = mouse[0] != 0. || mouse[1] != 0.;
            if moved {
                self.mouse_still = 0.;
                self.mouse[0] += mouse[0] / controls.mouse_throw;
                self.mouse[1] -= mouse[1] / controls.mouse_throw;
                let l = length(self.mouse);
                if l > 1. {
                    self.mouse = self.mouse.map(|v| v / l);
                }
            } else {
                self.mouse_still += dt;
                if self.mouse_still > 0.06 {
                    // A still mouse lets the stick spring back.
                    let fall = dt * 1000. / controls.flick_ms.max(1.);
                    let l = length(self.mouse);
                    let k = if l > fall { (l - fall) / l } else { 0. };
                    self.mouse = self.mouse.map(|v| v * k);
                }
            }
            direct = Some(self.mouse);
            self.mouse
        } else if self.shown != 5 {
            numpad_vector(self.shown)
        } else if manual {
            [0., 0.5]
        } else if nose {
            [0., -0.5]
        } else {
            [0., 0.]
        };
        if !mouse_on {
            self.mouse = [0., 0.];
        }

        // The stick travels, at flick speed, round the rim when it can.
        match direct {
            Some(v) => self.trick = v,
            None => {
                let before = self.trick;
                let step = 2. * dt * 1000. / controls.flick_ms.max(1.);
                let delta = [target[0] - before[0], target[1] - before[1]];
                let distance = length(delta);
                self.trick = if distance <= step {
                    target
                } else {
                    [before[0] + delta[0] / distance * step, before[1] + delta[1] / distance * step]
                };
                let (lb, lt) = (length(before), length(target));
                let cos = if lb > 0. && lt > 0. { (before[0] * target[0] + before[1] * target[1]) / (lb * lt) } else { -1. };
                let l = length(self.trick);
                if lb > 0.9 && lt > 0.9 && cos > -0.7 && l > 0. {
                    self.trick = self.trick.map(|v| v / l * lt);
                }
            }
        }
        pad.left = self.left;
        pad.right = self.trick;
        pad
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const DT: f32 = 1. / 120.;

    /// Runs frames with `keys` held until `ms`, returning every pad.
    fn run(t: &mut Translator, controls: &Controls, keys: &[&str], ms: f32) -> Vec<Pad> {
        let frames = (ms / (DT * 1000.)).round() as usize;
        (0..frames).map(|_| t.update(controls, |k| keys.contains(&k), [0., 0.], DT)).collect()
    }

    fn angle(v: [f32; 2]) -> f32 {
        v[1].atan2(v[0]).to_degrees()
    }

    #[test]
    fn a_one_key_ollie_pulls_down_then_flicks_up() {
        let controls = Controls::default();
        let mut t = Translator::new();
        let pads = run(&mut t, &controls, &["I"], 30.);
        assert!(pads.last().unwrap().right[1] < -0.9, "pulled down: {:?}", pads.last());
        let pads = run(&mut t, &controls, &[], 200.);
        let top = pads.iter().map(|p| p.right[1]).fold(f32::MIN, f32::max);
        assert!(top > 0.9, "flicked up");
        let first_up = pads.iter().position(|p| p.right[1] > 0.9).unwrap();
        let low = pads[..first_up].iter().rposition(|p| p.right[1] < -0.9).unwrap();
        assert!((first_up - low) as f32 * DT * 1000. <= 40., "a quick flick");
        assert_eq!(pads.last().unwrap().right, [0., 0.], "and back to centre");
    }

    #[test]
    fn a_quarter_circle_sweeps_round_the_rim() {
        let controls = Controls::default();
        let mut t = Translator::new();
        run(&mut t, &controls, &["Num2"], 50.);
        let mut pads = run(&mut t, &controls, &["Num3"], 30.);
        pads.extend(run(&mut t, &controls, &["Num6"], 50.));
        assert!(pads.iter().all(|p| length(p.right) > 0.95), "stays on the rim");
        let angles: Vec<f32> = pads.iter().map(|p| angle(p.right)).collect();
        assert!(angles.windows(2).all(|w| w[1] >= w[0] - 0.01), "turns one way: {angles:?}");
        assert!((angles.last().unwrap() - 0.).abs() < 1., "ends right: {angles:?}");
    }

    #[test]
    fn diagonals_are_lenient_on_release() {
        let controls = Controls::default();
        let mut t = Translator::new();
        run(&mut t, &controls, &["Down", "Right"], 60.);
        // Down lets go 16 ms before right: the stick never shows 6.
        let pads = run(&mut t, &controls, &["Right"], 16.);
        assert!(pads.iter().all(|p| p.right[1] < -0.5), "{pads:?}");
        let pads = run(&mut t, &controls, &[], 60.);
        assert!(pads.iter().all(|p| p.right[0] <= 0.71 + 1e-3));
        // Holding the cardinal past the buffer does show it.
        run(&mut t, &controls, &["Down", "Right"], 60.);
        let pads = run(&mut t, &controls, &["Right"], 100.);
        assert!(pads.last().unwrap().right[0] > 0.99);
    }

    #[test]
    fn a_marker_key_holds_lb_then_the_dpad() {
        use crate::controls::{DPAD_DOWN, LB};
        let controls = Controls::default();
        let mut t = Translator::new();
        let pads = run(&mut t, &controls, &["N"], 100.);
        assert_eq!(pads[0].buttons, LB, "the modifier first");
        assert_eq!(pads.last().unwrap().buttons, LB | DPAD_DOWN);
        let pads = run(&mut t, &controls, &[], 20.);
        assert_eq!(pads[0].buttons, 0);
    }

    #[test]
    fn keys_ramp_the_left_stick_and_press_buttons() {
        let controls = Controls::default();
        let mut t = Translator::new();
        let pads = run(&mut t, &controls, &["W", "Space", "Z"], 45.);
        let first = pads[0];
        assert!(first.left[1] > 0. && first.left[1] < 0.2, "ramps: {first:?}");
        assert!((pads.last().unwrap().left[1] - 0.5).abs() < 0.1);
        assert_eq!(first.buttons, crate::controls::A);
        assert_eq!(first.triggers, [255, 0]);
        let pads = run(&mut t, &controls, &[], 60.);
        assert_eq!(pads.last().unwrap().left, [0., 0.]);
    }

    #[test]
    fn the_mouse_steers_the_trick_stick_while_held() {
        let controls = Controls::default();
        let mut t = Translator::new();
        let pad = t.update(&controls, |k| k == "Mouse5", [0., 30.], DT);
        assert!((pad.right[1] + 0.5).abs() < 1e-5, "mouse down is stick down");
        let pad = t.update(&controls, |k| k == "Mouse5", [0., 100.], DT);
        assert!((length(pad.right) - 1.).abs() < 1e-5, "clamped");
        let held = RefCell::new(true);
        let mut last = pad;
        for _ in 0..40 {
            last = t.update(&controls, |k| *held.borrow() && k == "Mouse5", [0., 0.], DT);
        }
        assert_eq!(last.right, [0., 0.], "springs back when still");
        let pad = t.update(&controls, |_| false, [0., 50.], DT);
        assert_eq!(pad.right, [0., 0.], "no mouse without the key");
    }

    #[test]
    fn merged_inputs_take_the_stronger_stick() {
        let pad = Pad { buttons: 1, triggers: [10, 0], left: [0.2, 0.], right: [0., 0.] };
        let keys = Pad { buttons: 2, triggers: [0, 200], left: [0., 1.], right: [0., 0.] };
        let both = pad.merge(keys);
        assert_eq!(both.buttons, 3);
        assert_eq!(both.triggers, [10, 200]);
        assert_eq!(both.left, [0., 1.]);
        assert_eq!(both.axes().0, [0, 32767]);
        assert!(Pad::default().is_idle());
    }

    #[test]
    fn the_pop_key_crouches_then_flicks_up_and_toward_a_held_side() {
        let mut controls = Controls::default();
        let pop = Controls::action_index("pop").unwrap();
        let left = Controls::action_index("trick_left").unwrap();
        controls.bind(pop, "Space").unwrap();
        let mut t = Translator::new();
        let frame = |t: &mut Translator, keys: &[&str]| t.update(&controls, |k| keys.contains(&k), [0., 0.], 1. / 60.);
        let mut pad = Pad::default();
        for _ in 0..12 {
            pad = frame(&mut t, &["Space"]);
        }
        assert!(pad.right[1] < -0.9, "held: crouched, {:?}", pad.right);
        for _ in 0..3 {
            pad = frame(&mut t, &[]);
        }
        assert!(pad.right[1] > 0.5, "let go: up, {:?}", pad.right);
        for _ in 0..20 {
            pad = frame(&mut t, &[]);
        }
        assert!(pad.right[1].abs() < 0.1, "then back to the middle, {:?}", pad.right);
        // With trick-left held: up-left (a kickflip for a regular skater).
        let key = controls.keys[left][0];
        for _ in 0..12 {
            frame(&mut t, &["Space", key]);
        }
        for _ in 0..3 {
            pad = frame(&mut t, &[key]);
        }
        assert!(pad.right[0] < -0.4 && pad.right[1] > 0.4, "up-left, {:?}", pad.right);
    }
}
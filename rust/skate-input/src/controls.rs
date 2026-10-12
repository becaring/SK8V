//! What each key does on the board, and how the keyboard behaves.

use crate::keys;

/// A directional input in numpad notation: 1 down-left, 2 down, 3
/// down-right, 4 left, 5 neutral, 6 right, 7 up-left, 8 up, 9 up-right.
pub type Numpad = u8;

/// The stick direction of a numpad direction, x right and y up.
pub fn numpad_vector(direction: Numpad) -> [f32; 2] {
    let d = std::f32::consts::FRAC_1_SQRT_2;
    match direction {
        1 => [-d, -d],
        2 => [0., -1.],
        3 => [d, -d],
        4 => [-1., 0.],
        6 => [1., 0.],
        7 => [-d, d],
        8 => [0., 1.],
        9 => [d, d],
        _ => [0., 0.],
    }
}

/// The numpad direction of held up/down/left/right.
pub fn numpad_of(up: bool, down: bool, left: bool, right: bool) -> Numpad {
    let y = i32::from(up) - i32::from(down);
    let x = i32::from(right) - i32::from(left);
    (5 + x + 3 * y) as Numpad
}

/// One-key tricks: right-stick gestures in numpad notation, each direction
/// held for some milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gesture {
    Ollie,
    Nollie,
    Kickflip,
    Heelflip,
    ShoveItBack,
    ShoveItFront,
    Flip360,
}

impl Gesture {
    pub const ALL: [Self; 7] = [
        Self::Ollie,
        Self::Nollie,
        Self::Kickflip,
        Self::Heelflip,
        Self::ShoveItBack,
        Self::ShoveItFront,
        Self::Flip360,
    ];

    /// The motion, as SF6 writes motions: 2 then 8 is an ollie.
    pub fn motion(self) -> &'static [(Numpad, u32)] {
        match self {
            Self::Ollie => &[(2, 90), (8, 60)],
            Self::Nollie => &[(8, 90), (2, 60)],
            Self::Kickflip => &[(2, 90), (7, 60)],
            Self::Heelflip => &[(2, 90), (9, 60)],
            Self::ShoveItBack => &[(2, 70), (1, 35), (4, 60)],
            Self::ShoveItFront => &[(2, 70), (3, 35), (6, 60)],
            Self::Flip360 => &[(2, 70), (1, 30), (4, 30), (7, 60)],
        }
    }

    pub fn notation(self) -> String {
        self.motion().iter().map(|(d, _)| d.to_string()).collect::<Vec<_>>().join("")
    }
}

/// Everything a key can be bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// The left stick: lean, steer, walk.
    Stick(Numpad),
    /// The trick stick (right stick), classic style: hold directions.
    Trick(Numpad),
    /// The trick stick held half up (a manual) or half down (a nose manual).
    Manual,
    NoseManual,
    /// A whole trick on one key, modern style.
    Gesture(Gesture),
    /// A controller button, by its XInput bit.
    Button(u16),
    LeftTrigger,
    RightTrigger,
    /// Hold to steer the trick stick with the mouse.
    MouseTrick,
    /// Hold: the trick stick down (crouch); let go: flicked up (an ollie),
    /// up-left or up-right with a trick-left or -right key held (flip tricks).
    Pop,
    /// Switch between first and third person.
    CameraView,
    /// Skate 3's session marker: LB with this D-pad direction (down
    /// places the marker, up held returns to it).
    Marker(u16),
}

pub const A: u16 = 0x1000;
pub const B: u16 = 0x2000;
pub const X: u16 = 0x4000;
pub const Y: u16 = 0x8000;
pub const LB: u16 = 0x0100;
pub const RB: u16 = 0x0200;
pub const LS: u16 = 0x0040;
pub const RS: u16 = 0x0080;
pub const BACK: u16 = 0x0020;
pub const START: u16 = 0x0010;
pub const DPAD_UP: u16 = 0x0001;
pub const DPAD_DOWN: u16 = 0x0002;
pub const DPAD_LEFT: u16 = 0x0004;
pub const DPAD_RIGHT: u16 = 0x0008;

/// Every action with its id in the controls file, its label and its
/// default keys.
pub const ACTIONS: &[(Action, &str, &str, &[&str])] = &[
    (Action::Stick(8), "lean_forward", "Lean forward / walk", &["W"]),
    (Action::Stick(2), "lean_back", "Lean back", &["S"]),
    (Action::Stick(4), "steer_left", "Steer left", &["A"]),
    (Action::Stick(6), "steer_right", "Steer right", &["D"]),
    (Action::Trick(8), "trick_up", "Trick stick up (8)", &["Up"]),
    (Action::Trick(2), "trick_down", "Trick stick down (2)", &["Down"]),
    (Action::Trick(4), "trick_left", "Trick stick left (4)", &["Left"]),
    (Action::Trick(6), "trick_right", "Trick stick right (6)", &["Right"]),
    (Action::Trick(7), "trick_7", "Trick stick up-left (7)", &["Num7"]),
    (Action::Trick(8), "trick_8", "Trick stick up (8)", &["Num8"]),
    (Action::Trick(9), "trick_9", "Trick stick up-right (9)", &["Num9"]),
    (Action::Trick(4), "trick_4", "Trick stick left (4)", &["Num4"]),
    (Action::Trick(6), "trick_6", "Trick stick right (6)", &["Num6"]),
    (Action::Trick(1), "trick_1", "Trick stick down-left (1)", &["Num1"]),
    (Action::Trick(2), "trick_2", "Trick stick down (2)", &["Num2"]),
    (Action::Trick(3), "trick_3", "Trick stick down-right (3)", &["Num3"]),
    (Action::Manual, "manual", "Manual (stick half up)", &["Num5"]),
    (Action::NoseManual, "nose_manual", "Nose manual (stick half down)", &["Num0"]),
    (Action::Gesture(Gesture::Ollie), "ollie", "Ollie (28)", &["I"]),
    (Action::Gesture(Gesture::Nollie), "nollie", "Nollie (82)", &["K"]),
    (Action::Gesture(Gesture::Kickflip), "kickflip", "Kickflip (27)", &["U"]),
    (Action::Gesture(Gesture::Heelflip), "heelflip", "Heelflip (29)", &["O"]),
    (Action::Gesture(Gesture::ShoveItBack), "shove_back", "Shove-it (214)", &["H"]),
    (Action::Gesture(Gesture::ShoveItFront), "shove_front", "Front shove-it (236)", &["L"]),
    (Action::Gesture(Gesture::Flip360), "flip_360", "360 flip (2147)", &["P"]),
    (Action::Button(A), "a", "A: push", &["Space"]),
    (Action::Button(B), "b", "B", &["LCtrl"]),
    (Action::Button(X), "x", "X", &["LShift"]),
    (Action::Button(Y), "y", "Y: get off / on the board", &["Q"]),
    (Action::LeftTrigger, "lt", "Left trigger: left-hand grab", &["Z"]),
    (Action::RightTrigger, "rt", "Right trigger: right-hand grab", &["C"]),
    (Action::Button(LB), "lb", "Left bumper", &["X"]),
    (Action::Button(RB), "rb", "Right bumper", &["B"]),
    (Action::Button(LS), "ls", "Left stick click", &["LAlt"]),
    (Action::Button(RS), "rs", "Right stick click", &["Mouse3"]),
    (Action::Button(BACK), "back", "Back", &["Backspace"]),
    (Action::Button(DPAD_UP), "dpad_up", "D-pad up", &[]),
    (Action::Button(DPAD_DOWN), "dpad_down", "D-pad down", &[]),
    (Action::Button(DPAD_LEFT), "dpad_left", "D-pad left", &[]),
    (Action::Button(DPAD_RIGHT), "dpad_right", "D-pad right", &[]),
    (Action::MouseTrick, "mouse_trick", "Hold: mouse moves the trick stick", &["Mouse5"]),
    (Action::Pop, "pop", "Hold: crouch, let go: ollie (with trick left/right: flip)", &[]),
    // Skate 3's hippie jump: both push buttons held, then let go.
    (Action::Button(A | X), "hippie", "Hold: hippie jump (both push buttons)", &[]),
    (Action::CameraView, "camera", "Switch first / third person", &["V"]),
    (Action::Marker(DPAD_DOWN), "marker_place", "Place session marker (LB + down)", &["N"]),
    (Action::Marker(DPAD_UP), "marker_return", "Hold: back to session marker (LB + up)", &["M"]),
    (Action::Marker(DPAD_RIGHT), "object_dropper", "Object dropper: build a park (LB + right)", &["Y"]),
    (Action::Marker(DPAD_LEFT), "retry", "Back to the checkpoint (LB + left)", &["Comma"]),
];

/// How the mouse moves the trick stick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseTrick {
    Off,
    /// While the mouse-trick key is held.
    Hold,
    Always,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Controls {
    /// Keys per action, in `ACTIONS` order.
    pub keys: Vec<Vec<&'static str>>,
    /// Hold the trick stick with direction keys.
    pub classic: bool,
    /// Whole tricks on one key.
    pub modern: bool,
    pub mouse: MouseTrick,
    /// Mouse travel (counts) for a full stick throw.
    pub mouse_throw: f32,
    /// Milliseconds the trick stick takes across: a keyboard flick's speed.
    pub flick_ms: f32,
    /// SF6-style leniency: keys pressed or let go within this many
    /// milliseconds of each other count as together (diagonals).
    pub buffer_ms: f32,
    /// Milliseconds the left stick takes to reach full lean from a key.
    pub ramp_ms: f32,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            keys: ACTIONS
                .iter()
                .map(|(_, _, _, keys)| keys.iter().filter_map(|k| keys::canonical(k)).collect())
                .collect(),
            classic: true,
            modern: true,
            mouse: MouseTrick::Hold,
            mouse_throw: 60.,
            flick_ms: 35.,
            buffer_ms: 34.,
            ramp_ms: 90.,
        }
    }
}

fn on_off(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "1" | "true" | "yes" => Some(true),
        "off" | "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn number(value: &str, low: f32, high: f32) -> Option<f32> {
    value.trim().parse::<f32>().ok().filter(|v| v.is_finite() && (low..=high).contains(v))
}

impl Controls {
    pub fn action_index(id: &str) -> Option<usize> {
        ACTIONS.iter().position(|(_, name, _, _)| name.eq_ignore_ascii_case(id.trim()))
    }

    /// Every key some action uses.
    pub fn claimed(&self) -> Vec<&'static str> {
        let mut keys: Vec<&'static str> = self
            .keys
            .iter()
            .enumerate()
            .filter(|(i, _)| self.uses(ACTIONS[*i].0))
            .flat_map(|(_, k)| k.iter().copied())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// Whether an action is in effect with the schemes switched on.
    pub fn uses(&self, action: Action) -> bool {
        match action {
            Action::Trick(_) | Action::Manual | Action::NoseManual => self.classic,
            Action::Gesture(_) => self.modern,
            Action::MouseTrick => self.mouse == MouseTrick::Hold,
            _ => true,
        }
    }

    /// Binds `key` to action `index` alone, taking it from any other action.
    pub fn bind(&mut self, index: usize, key: &str) -> Result<(), String> {
        let key = keys::canonical(key).ok_or_else(|| format!("unknown key {key:?}"))?;
        for keys in &mut self.keys {
            keys.retain(|k| *k != key);
        }
        let slot = self.keys.get_mut(index).ok_or("unknown action")?;
        *slot = vec![key];
        Ok(())
    }

    pub fn unbind(&mut self, index: usize) {
        if let Some(keys) = self.keys.get_mut(index) {
            keys.clear();
        }
    }

    pub fn keys_text(&self, index: usize) -> String {
        match self.keys.get(index) {
            Some(keys) if !keys.is_empty() => keys.join(" / "),
            _ => "-".into(),
        }
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from("; Skate controls on keyboard and mouse. Key names: see the README.\n");
        out.push_str(&format!("classic = {}\n", if self.classic { "on" } else { "off" }));
        out.push_str(&format!("modern = {}\n", if self.modern { "on" } else { "off" }));
        out.push_str(&format!(
            "mouse = {}\n",
            match self.mouse {
                MouseTrick::Off => "off",
                MouseTrick::Hold => "hold",
                MouseTrick::Always => "always",
            }
        ));
        out.push_str(&format!("mouse_throw = {}\n", self.mouse_throw));
        out.push_str(&format!("flick_ms = {}\n", self.flick_ms));
        out.push_str(&format!("buffer_ms = {}\n", self.buffer_ms));
        out.push_str(&format!("ramp_ms = {}\n", self.ramp_ms));
        for (i, (_, id, _, _)) in ACTIONS.iter().enumerate() {
            out.push_str(&format!("bind {id} = {}\n", self.keys[i].join(" ")));
        }
        out
    }

    /// Reads [`Self::to_text`]; lines it cannot use keep the defaults and
    /// are reported.
    pub fn from_text(text: &str) -> (Self, Vec<String>) {
        let mut controls = Self::default();
        let mut problems = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split(';').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                problems.push(format!("line {}: no '='", n + 1));
                continue;
            };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
            let ok = if let Some(id) = key.strip_prefix("bind ") {
                match Self::action_index(id) {
                    Some(index) => {
                        let keys: Option<Vec<&'static str>> = value.split_whitespace().map(keys::canonical).collect();
                        keys.map(|keys| controls.keys[index] = keys).is_some()
                    }
                    None => false,
                }
            } else {
                match key.as_str() {
                    "classic" => on_off(value).map(|v| controls.classic = v).is_some(),
                    "modern" => on_off(value).map(|v| controls.modern = v).is_some(),
                    "mouse" => match value.to_ascii_lowercase().as_str() {
                        "off" => Some(MouseTrick::Off),
                        "hold" => Some(MouseTrick::Hold),
                        "always" => Some(MouseTrick::Always),
                        _ => None,
                    }
                    .map(|v| controls.mouse = v)
                    .is_some(),
                    "mouse_throw" => number(value, 5., 1000.).map(|v| controls.mouse_throw = v).is_some(),
                    "flick_ms" => number(value, 5., 300.).map(|v| controls.flick_ms = v).is_some(),
                    "buffer_ms" => number(value, 0., 150.).map(|v| controls.buffer_ms = v).is_some(),
                    "ramp_ms" => number(value, 0., 1000.).map(|v| controls.ramp_ms = v).is_some(),
                    _ => false,
                }
            };
            if !ok {
                problems.push(format!("line {}: cannot use {line:?}", n + 1));
            }
        }
        (controls, problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numpad_notation() {
        assert_eq!(numpad_of(false, true, false, false), 2);
        assert_eq!(numpad_of(true, false, true, false), 7);
        assert_eq!(numpad_of(true, true, false, true), 6, "up and down cancel");
        assert_eq!(numpad_of(false, false, false, false), 5);
        for d in 1..=9 {
            let [x, y] = numpad_vector(d);
            let expected = if d == 5 { 0. } else { 1. };
            assert!((x.hypot(y) - expected).abs() < 1e-5);
        }
        assert_eq!(Gesture::ShoveItFront.notation(), "236");
    }

    #[test]
    fn controls_round_trip_and_rebind() {
        let mut controls = Controls::default();
        assert!(controls.keys.iter().flatten().all(|k| keys::canonical(k).is_some()));
        let push = Controls::action_index("a").unwrap();
        let ollie = Controls::action_index("ollie").unwrap();
        controls.bind(push, "i").unwrap();
        assert_eq!(controls.keys[push], vec!["I"]);
        assert!(controls.keys[ollie].is_empty(), "a key does one thing");
        controls.modern = false;
        controls.mouse = MouseTrick::Always;
        controls.flick_ms = 50.;
        let (read, problems) = Controls::from_text(&controls.to_text());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(read, controls);
        assert!(!controls.claimed().contains(&"Mouse5"), "mouse-trick key unused when always on");
        let (_, problems) = Controls::from_text("bind ollie = Hyper\nflick_ms = 0\nnonsense\n");
        assert_eq!(problems.len(), 3);
    }
}

//! Keyboard and mouse as a controller (contributed by GTAxSkate).
//!
//! SkateV reads an XInput controller. Without one, the host asks this module
//! each frame for a pad made from the keyboard and mouse (`sv_keyboard_pad`):
//! the `skate_input` crate translates keys into Skate's buttons and sticks,
//! with a fighting-game style trick stick (direction keys typed as motions)
//! and the mouse flicking the stick. The keys are a plain text file beside
//! the game (`SkateVKeys.txt`, written with the defaults on first use, so a
//! player can rebind any action there).
//!
//! Default layout (GTA-like): WASD move, Shift push, Space pop (held = stick
//! down, let go = flick up), arrows / numpad trick stick, E the board (Y:
//! mount, dismount, hold to put away), Q hippie jump, T Skate's X, Ctrl B,
//! the mouse flicks tricks while its trick key is held.

use skate_input::{Controls, Translator};
use std::path::PathBuf;
use std::sync::Mutex;

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
}

#[cfg(not(windows))]
#[allow(non_snake_case)]
unsafe fn GetAsyncKeyState(_key: i32) -> i16 {
    0
}

struct Keyboard {
    controls: Controls,
    translator: Translator,
    path: PathBuf,
}

static KEYBOARD: Mutex<Option<Keyboard>> = Mutex::new(None);

/// The default keys: GTA-like, every key used once.
pub fn default_controls() -> Controls {
    let mut controls = Controls::default();
    let index = |id: &str| Controls::action_index(id).expect("a Skate action");
    // Alt is GTA's character wheel, R its reload, the numpad keeps the trick
    // stick only (no one-key tricks there).
    for id in ["trick_7", "trick_8", "trick_9", "trick_4", "trick_6", "trick_1", "trick_2", "trick_3", "camera", "mouse_trick", "ls"] {
        controls.unbind(index(id));
    }
    // LB + d-pad left/right is SK8V's radio: Comma and Period, as GTA's own
    // radio keys; M stays GTA's interaction menu, so the marker's return is J.
    for (id, key) in [("pop", "Space"), ("a", "LShift"), ("x", "T"), ("b", "LCtrl"), ("hippie", "Q"), ("y", "E"),
                      ("retry", "Comma"), ("object_dropper", "Period"), ("marker_return", "J")] {
        controls.bind(index(id), key).expect("a key");
    }
    controls.modern = false;
    controls.mouse = skate_input::MouseTrick::Off;
    controls
}

/// The keys laid out like a controller in Skate 3: the left hand on W A S D
/// as the left stick, the right hand on I J K L as the trick stick (no numpad
/// needed), Space the A button (push), Shift X, Ctrl B, E Y.
pub fn skate_controls() -> Controls {
    let mut controls = default_controls();
    let index = |id: &str| Controls::action_index(id).expect("a Skate action");
    // U and O the upper diagonals, as on a stick (I J K L held together give the others).
    for (id, key) in [("trick_up", "I"), ("trick_left", "J"), ("trick_down", "K"), ("trick_right", "L"), ("trick_7", "U"), ("trick_9", "O"), ("a", "Space"),
                      ("x", "LShift"), ("b", "LCtrl"), ("y", "E"), ("marker_return", "H")] {
        controls.bind(index(id), key).expect("a key");
    }
    // The ollie comes from the trick stick (K, then I), as on a controller;
    // J went to the stick, so the marker's return is H.
    controls.unbind(index("pop"));
    controls
}

/// Replaces every key with a whole layout ("standard": GTAxSkate's, GTA-like;
/// "skate": like a controller) and saves the key file.
pub fn layout(name: &str) -> Result<(), String> {
    let controls = match name {
        "standard" => default_controls(),
        "skate" => skate_controls(),
        _ => return Err(format!("no layout {name}")),
    };
    let mut guard = KEYBOARD.lock().map_err(|_| "keyboard state poisoned".to_string())?;
    let k = guard.as_mut().ok_or("the keyboard is not configured")?;
    std::fs::write(&k.path, controls.to_text()).map_err(|e| e.to_string())?;
    k.controls = controls;
    k.translator = Translator::new();
    Ok(())
}

/// Loads the keys from `path`, writing the defaults there when it does not
/// exist. Returns problems found in the file (unknown keys or actions).
pub fn configure(path: PathBuf) -> Vec<String> {
    let (controls, problems) = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (mut controls, problems) = Controls::from_text(&text);
            // Files written before the radio keys: the old defaults moved.
            let index = |id: &str| Controls::action_index(id).expect("a Skate action");
            let mut moved = false;
            for (id, old, new) in [("object_dropper", "Y", "Period"), ("marker_return", "M", "J")] {
                if controls.keys[index(id)] == [old] {
                    let _ = controls.bind(index(id), new);
                    moved = true;
                }
            }
            if moved {
                let _ = std::fs::write(&path, controls.to_text());
            }
            (controls, problems)
        }
        Err(_) => {
            let controls = default_controls();
            let _ = std::fs::write(&path, controls.to_text());
            (controls, Vec::new())
        }
    };
    if let Ok(mut k) = KEYBOARD.lock() {
        *k = Some(Keyboard { controls, translator: Translator::new(), path: path.clone() });
    }
    problems
}

fn key_down(name: &str) -> bool {
    skate_input::keys::virtual_key(name).is_some_and(|vk| (unsafe { GetAsyncKeyState(vk as i32) } as u16) & 0x8000 != 0)
}

/// This frame's pad from the keyboard and mouse (`mouse`: the frame's look
/// input, x right and y down), or none when nothing is configured. `active`
/// false (the game has no focus, a menu is open) releases everything.
pub fn pad(mouse: [f32; 2], dt: f32, active: bool) -> Option<skate_input::Pad> {
    let mut guard = KEYBOARD.lock().ok()?;
    let k = guard.as_mut()?;
    if !active {
        k.translator.reset();
        return None;
    }
    Some(k.translator.update(&k.controls, key_down, mouse, dt))
}

/// Whether a pad from `pad` does anything (a key held or the stick moved).
/// The keys' hints for the HUD, one "key	what" line each; empty before
/// `configure`.
pub fn hints(on_foot: bool) -> String {
    let Ok(guard) = KEYBOARD.lock() else { return String::new() };
    let Some(k) = guard.as_ref() else { return String::new() };
    skate_input::hints::keyboard(&k.controls, on_foot)
        .into_iter()
        .map(|(key, what)| format!("{key}	{what}
"))
        .collect()
}

/// The key bound to action `id` (by `Controls::ACTIONS` id), "" when none.
pub fn key_of(id: &str) -> String {
    let Ok(guard) = KEYBOARD.lock() else { return String::new() };
    let (Some(k), Some(i)) = (guard.as_ref(), Controls::action_index(id)) else { return String::new() };
    k.controls.keys.get(i).and_then(|keys| keys.first()).map(|k| k.to_string()).unwrap_or_default()
}

/// Binds Windows virtual key `vk` to action `id` alone (taking it from any
/// other action) and saves the key file. The key's name, or an error.
pub fn bind(id: &str, vk: u32) -> Result<String, String> {
    let name = skate_input::keys::from_virtual_key(vk).ok_or_else(|| format!("key {vk:#x} has no name"))?;
    let mut guard = KEYBOARD.lock().map_err(|_| "keyboard state poisoned".to_string())?;
    let k = guard.as_mut().ok_or("the keyboard is not configured")?;
    let index = Controls::action_index(id).ok_or_else(|| format!("no action {id}"))?;
    k.controls.bind(index, name)?;
    std::fs::write(&k.path, k.controls.to_text()).map_err(|e| e.to_string())?;
    Ok(name.to_string())
}

pub fn is_active(p: &skate_input::Pad) -> bool {
    p.buttons != 0 || p.triggers != [0, 0] || p.left.iter().chain(&p.right).any(|v| v.abs() > 0.01)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_key_files_get_the_radio_keys() {
        let path = std::env::temp_dir().join(format!("skatev-keys-old-{}.txt", std::process::id()));
        let mut old = default_controls();
        let index = |id: &str| Controls::action_index(id).unwrap();
        old.bind(index("object_dropper"), "Y").unwrap();
        old.bind(index("marker_return"), "M").unwrap();
        std::fs::write(&path, old.to_text()).unwrap();
        configure(path.clone());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("bind object_dropper = Period") && text.contains("bind marker_return = J"), "{text}");
        assert!(hints(false).contains("Comma, Period\tRadio"), "{}", hints(false));
    }

    #[test]
    fn the_skate_layout_puts_the_trick_stick_on_ijkl_and_uses_each_key_once() {
        let c = skate_controls();
        let key = |id: &str| c.keys[Controls::action_index(id).unwrap()].clone();
        assert_eq!((key("trick_up"), key("trick_left"), key("trick_down"), key("trick_right")), (vec!["I"], vec!["J"], vec!["K"], vec!["L"]));
        assert_eq!(key("a"), vec!["Space"]);
        assert!(key("pop").is_empty());
        let mut seen = std::collections::HashSet::new();
        for keys in &c.keys {
            for k in keys {
                assert!(seen.insert(*k), "{k} bound twice");
            }
        }
    }

    #[test]
    fn a_key_is_rebound_and_saved() {
        let path = std::env::temp_dir().join(format!("skatev-keys-bind-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        configure(path.clone());
        assert_eq!(bind("hippie", 0x47).unwrap(), "G"); // VK 'G'
        assert_eq!(key_of("hippie"), "G");
        assert!(std::fs::read_to_string(&path).unwrap().contains("bind hippie = G"));
        assert!(bind("no_such_action", 0x47).is_err());
    }

    #[test]
    fn the_hints_name_the_default_keys() {
        configure(std::env::temp_dir().join(format!("skatev-keys-{}.txt", std::process::id())));
        let riding = hints(false);
        assert!(riding.contains("Space	Hold: crouch, let go: ollie"), "{riding}");
        assert!(riding.contains("LShift	Push"), "{riding}");
        assert!(riding.contains("E	Get off"), "{riding}");
        assert!(hints(true).contains("E	Get on the board"));
    }

    #[test]
    fn the_default_keys_round_trip_and_use_every_key_once() {
        let controls = default_controls();
        let (back, problems) = Controls::from_text(&controls.to_text());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(back.to_text(), controls.to_text());
        let mut seen = std::collections::HashSet::new();
        for line in controls.to_text().lines().filter(|l| l.starts_with("bind ")) {
            if let Some((_, key)) = line.split_once(" = ") {
                let key = key.trim();
                assert!(key.is_empty() || seen.insert(key.to_owned()), "{key} bound twice");
            }
        }
    }
}

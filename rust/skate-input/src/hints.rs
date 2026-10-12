//! The controls a HUD shows while skating and in the object dropper, named
//! by what the player has them on: the controller's buttons, or the keys
//! they bound themselves (`Controls`), never the defaults.

use crate::controls::{Controls, MouseTrick};

/// A control as the HUD names it: with a controller in, its button and the
/// player's keys side by side ("Space / A"); without one, the keys alone.
fn label(controls: &Controls, pad: bool, pad_name: &str, ids: &[&str]) -> Option<String> {
    let keys = key_label(controls, ids);
    match (pad, keys, pad_name.is_empty()) {
        (true, Some(keys), false) => Some(format!("{keys} / {pad_name}")),
        (true, None, false) => Some(pad_name.to_owned()),
        (_, keys, _) => keys,
    }
}

/// The player's keys for one or more actions (by `ACTIONS` id).
fn key_label(controls: &Controls, ids: &[&str]) -> Option<String> {
    let keys: Vec<String> = ids
        .iter()
        .filter_map(|id| Controls::action_index(id))
        .filter_map(|i| controls.keys.get(i))
        .map(|keys| keys.first().copied().unwrap_or("-").to_owned())
        .collect();
    if keys.is_empty() || keys.iter().all(|k| k == "-") {
        return None;
    }
    // "W A S D", or "Z, C" for a pair.
    Some(if ids.len() == 4 { keys.join(" ") } else { keys.join(", ") })
}

/// One hint: the control and what it does.
pub type Hint = (String, &'static str);

fn push(out: &mut Vec<Hint>, controls: &Controls, pad: bool, pad_name: &str, ids: &[&str], what: &'static str) {
    if let Some(label) = label(controls, pad, pad_name, ids) {
        out.push((label, what));
    }
}

/// The controls while skating (riding, or walking with the board).
pub fn skating(controls: &Controls, pad: bool, on_foot: bool) -> Vec<Hint> {
    let mut out = Vec::new();
    let c = controls;
    let stick = ["lean_forward", "steer_left", "lean_back", "steer_right"];
    if on_foot {
        push(&mut out, c, pad, "Left stick", &stick, "Walk");
        push(&mut out, c, pad, "Y", &["y"], "Get on the board");
    } else {
        push(&mut out, c, pad, "Left stick", &stick, "Steer / lean");
        push(&mut out, c, pad, "A", &["a"], "Push");
        if pad || c.classic {
            push(&mut out, c, pad, "Right stick", &["trick_up", "trick_left", "trick_down", "trick_right"], "Trick stick (flick)");
        }
        if !pad && c.modern {
            push(&mut out, c, pad, "", &["ollie"], "Ollie");
            push(&mut out, c, pad, "", &["kickflip"], "Kickflip");
            push(&mut out, c, pad, "", &["heelflip"], "Heelflip");
        }
        if !pad && c.mouse == MouseTrick::Hold {
            push(&mut out, c, pad, "", &["mouse_trick"], "Hold: mouse flicks");
        }
        push(&mut out, c, pad, "LT / RT", &["lt", "rt"], "Grabs");
        push(&mut out, c, pad, "RB", &["rb"], "Hold behind a car: skitch");
        push(&mut out, c, pad, "Y", &["y"], "Get off");
    }
    push(&mut out, c, pad, "LB + Down", &["marker_place"], "Set marker");
    push(&mut out, c, pad, "Hold LB + Up", &["marker_return"], "Back to marker");
    push(&mut out, c, pad, "LB + Left", &["retry"], "Checkpoint");
    push(&mut out, c, pad, "LB + Right", &["object_dropper"], "Object dropper");
    push(&mut out, c, pad, "", &["camera"], "First / third person");
    out
}

/// The keyboard controls for a host that has no Skate session tools (no
/// markers, checkpoints or object dropper): what each of the player's keys
/// does while walking with the board or riding it.
pub fn keyboard(controls: &Controls, on_foot: bool) -> Vec<Hint> {
    let mut out = Vec::new();
    let c = controls;
    let stick = ["lean_forward", "steer_left", "lean_back", "steer_right"];
    if on_foot {
        push(&mut out, c, false, "", &stick, "Walk");
        push(&mut out, c, false, "", &["y"], "Get on the board");
        return out;
    }
    push(&mut out, c, false, "", &stick, "Steer / lean");
    push(&mut out, c, false, "", &["a"], "Push");
    push(&mut out, c, false, "", &["pop"], "Hold: crouch, let go: ollie");
    push(&mut out, c, false, "", &["trick_up", "trick_left", "trick_down", "trick_right"], "Trick stick (flick)");
    push(&mut out, c, false, "", &["manual", "nose_manual"], "Manual / nose manual");
    push(&mut out, c, false, "", &["lt", "rt"], "Grabs");
    // Skate's GrabWorld (RB): behind a moving car it holds on (skitching).
    push(&mut out, c, false, "", &["rb"], "Hold behind a car: skitch");
    push(&mut out, c, false, "", &["hippie"], "Hippie jump");
    push(&mut out, c, false, "", &["x"], "X");
    push(&mut out, c, false, "", &["b"], "B");
    push(&mut out, c, false, "", &["y"], "Get off");
    // LB + d-pad: SK8V's radio (left/right) and Skate's session marker (down/up).
    push(&mut out, c, false, "", &["retry", "object_dropper"], "Radio: previous, next station");
    push(&mut out, c, false, "", &["marker_place"], "Set a marker");
    push(&mut out, c, false, "", &["marker_return"], "Hold: back to the marker");
    out
}

/// The object dropper's controls.
pub fn dropper(controls: &Controls, pad: bool) -> Vec<Hint> {
    let mut out = Vec::new();
    let c = controls;
    push(&mut out, c, pad, "Left stick", &["lean_forward", "steer_left", "lean_back", "steer_right"], "Move");
    push(&mut out, c, pad, "Right stick", &["trick_up", "trick_left", "trick_down", "trick_right"], "Turn");
    push(&mut out, c, pad, "LB / RB", &["lb", "rb"], "Previous / next object");
    push(&mut out, c, pad, "LT / RT", &["lt", "rt"], "Lower / raise");
    push(&mut out, c, pad, "A", &["a"], "Drop it");
    push(&mut out, c, pad, "X", &["x"], "Pick up");
    push(&mut out, c, pad, "Y", &["y"], "Pick up all");
    push(&mut out, c, pad, "B", &["b"], "Done");
    out
}

/// The label for one action (the marker prompt and the like).
pub fn one(controls: &Controls, pad: bool, pad_name: &str, id: &str) -> String {
    label(controls, pad, pad_name, &[id]).unwrap_or_else(|| "-".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_name_the_players_own_keys() {
        let mut c = Controls::default();
        let pad = skating(&c, true, false);
        assert!(pad.iter().any(|(k, w)| k == "Space / A" && *w == "Push"), "{pad:?}");
        let keys = skating(&c, false, false);
        assert!(keys.iter().any(|(k, w)| k == "W A S D" && w.starts_with("Steer")), "{keys:?}");
        assert!(keys.iter().any(|(k, w)| k == "Space" && *w == "Push"));
        // A rebound key shows as bound.
        c.bind(Controls::action_index("a").unwrap(), "F").unwrap();
        assert!(skating(&c, false, false).iter().any(|(k, w)| k == "F" && *w == "Push"));
        // An unbound action is left out.
        c.unbind(Controls::action_index("camera").unwrap());
        assert!(!skating(&c, false, false).iter().any(|(_, w)| w.contains("person")));
        let d = dropper(&c, false);
        assert!(d.iter().any(|(k, w)| k == "Z, C" && w.starts_with("Lower")));
        assert_eq!(one(&c, true, "LB + Down", "marker_place"), "N / LB + Down");
        assert_eq!(skating(&c, false, true)[0].1, "Walk");
    }
}

//! Griptapes to choose apart from the deck. Skate 3 has none of its own:
//! every deck's finish is see-through over its grip side, and the game lays
//! one plain black grip under it. These are made from Skate 3's own pieces
//! instead: its grip grain in plain colours, a checkerboard, and grips
//! printed with a deck's graphic, as real print griptapes are.

use crate::{Library, Part, PartRef};

#[derive(Clone, Debug, PartialEq)]
pub enum GripLook {
    /// The grain in one colour.
    Solid([u8; 3]),
    /// Squares of two colours, this many across the board's width.
    Checker([u8; 3], [u8; 3], u32),
    /// A deck's graphic printed on the grain.
    Print(PartRef),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grip {
    /// Short and file-name safe: the models are `sk3_g_<deck>_<id>`.
    pub id: String,
    pub name: String,
    pub look: GripLook,
}

/// Decks whose graphic is printed on a grip: every this many of the new
/// decks, so the prints spread over the catalog.
const PRINT_EVERY: usize = 24;

/// Every griptape on sale, the same for the exporter and the plugin.
pub fn catalog(library: Option<&Library>) -> Vec<Grip> {
    let solid = |id: &str, name: &str, rgb: [u8; 3]| Grip { id: id.into(), name: name.into(), look: GripLook::Solid(rgb) };
    let mut grips = vec![
        solid("black", "Black", [38, 38, 40]),
        solid("grey", "Grey", [110, 112, 116]),
        solid("white", "White", [220, 220, 216]),
        solid("red", "Red", [170, 30, 34]),
        solid("blue", "Blue", [34, 70, 170]),
        solid("green", "Green", [40, 140, 60]),
        solid("yellow", "Yellow", [220, 190, 40]),
        solid("orange", "Orange", [220, 110, 30]),
        solid("purple", "Purple", [110, 50, 150]),
        solid("pink", "Pink", [225, 110, 170]),
        solid("teal", "Teal", [30, 150, 150]),
        Grip { id: "checker".into(), name: "Checker".into(), look: GripLook::Checker([235, 235, 230], [30, 30, 32], 4) },
        Grip { id: "checkerred".into(), name: "Checker Red".into(), look: GripLook::Checker([180, 30, 34], [30, 30, 32], 4) },
        Grip { id: "checkerfine".into(), name: "Checker Fine".into(), look: GripLook::Checker([235, 235, 230], [30, 30, 32], 8) },
    ];
    if let Some(library) = library {
        for item in library.new_items(Part::Deck).into_iter().step_by(PRINT_EVERY) {
            grips.push(Grip {
                id: format!("p{}", item.material.to_ascii_lowercase()),
                name: format!("Print: {}", item.label),
                look: GripLook::Print(item.part_ref()),
            });
        }
    }
    grips
}

/// The deck shape a grip model is cut for: the first four letters of the
/// deck's model, `dflt` for the default skater's deck.
pub fn deck_key(deck: Option<&PartRef>) -> String {
    match deck {
        None => "dflt".to_owned(),
        Some(d) if d.asset == crate::paints::ASSET => crate::paints::DECK_KEY.to_owned(),
        Some(d) => d.asset.chars().take(4).collect::<String>().to_ascii_lowercase(),
    }
}

/// The model of grip `id` on `deck`.
pub fn grip_model(deck: Option<&PartRef>, id: &str) -> String {
    format!("sk3_g_{}_{}", deck_key(deck), id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_painted_deck_takes_its_base_decks_grips() {
        let painted = crate::paints::catalog()[0].part_ref();
        assert_eq!(deck_key(Some(&painted)), crate::paints::DECK_KEY);
        assert_eq!(grip_model(Some(&painted), "black"), format!("sk3_g_{}_black", crate::paints::DECK_KEY));
    }

    #[test]
    fn grips_have_distinct_safe_ids_and_models_per_deck_shape() {
        let grips = catalog(None);
        let mut ids: Vec<&str> = grips.iter().map(|g| g.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), grips.len());
        assert!(grips.iter().all(|g| g.id.chars().all(|c| c.is_ascii_alphanumeric())));
        let deck = PartRef::new("f2c517ee8b8359da", "2c7f381700171a56");
        assert_eq!(grip_model(Some(&deck), "red"), "sk3_g_f2c5_red");
        assert_eq!(grip_model(None, "red"), "sk3_g_dflt_red");
    }
}

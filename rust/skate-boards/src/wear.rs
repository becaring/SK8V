//! Boards wear out by being ridden, as real ones do: the deck from slides,
//! landings and bails, the trucks from grinds, the wheels from the ground
//! they cover and powerslides. A part worn through shows Skate 3's own
//! "Worn" finish of it; a new one of the same part starts fresh.

use std::collections::BTreeMap;

use crate::{BoardChoice, Library, Part, PartRef};

/// Wear at which a part is worn through.
pub const WORN: f32 = 1.0;
/// Wear from which a part shows Skate 3's worn finish: halfway, so a board
/// looks used after about half a session of hard skating (the scuff decals in
/// between cannot be drawn on the skinned board parts).
pub const WORN_LOOK: f32 = 0.5;

/// The scuffs a deck worn `wear` shows: none, then light, worn, battered
/// (1 to 3) on the way to its worn finish.
pub fn scuff_stage(wear: f32) -> Option<u8> {
    match wear {
        w if w < 0.12 => None,
        w if w < 0.35 => Some(1),
        w if w < 0.65 => Some(2),
        _ => Some(3),
    }
}

/// What riding did since the last report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// Metres rolled on the wheels.
    pub rolled: f32,
    /// Seconds grinding on the trucks, sliding on the deck, powersliding.
    pub truck_grind: f32,
    pub deck_slide: f32,
    pub powerslide: f32,
    pub landings: u32,
    pub bails: u32,
    /// Blows struck with the board (as a weapon).
    pub hits: u32,
}

impl Usage {
    /// How much each part wears from this, by [`Part::index`]: about a
    /// session of hard skating wears a deck through.
    pub fn wear(&self) -> [f32; 3] {
        let landings = self.landings as f32;
        let bails = self.bails as f32;
        let hits = self.hits as f32;
        let deck = self.deck_slide * 0.004 + self.truck_grind * 0.0005 + landings * 0.0015 + bails * 0.01 + hits * 0.025;
        let trucks = self.truck_grind * 0.004 + landings * 0.0005 + bails * 0.003 + hits * 0.006;
        let wheels = self.rolled * 0.00025 + self.powerslide * 0.004;
        [deck, trucks, wheels].map(|w| if w.is_finite() { w.max(0.) } else { 0. })
    }
}

/// How worn each part the player has ridden is, by part (`asset:material`
/// of its new finish).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wear {
    parts: BTreeMap<String, f32>,
}

fn key(part: &PartRef) -> String {
    format!("{}:{}", part.asset, part.material)
}

impl Wear {
    /// How worn a part is, 0 (new) to [`WORN`] and past.
    pub fn of(&self, part: &PartRef) -> f32 {
        self.parts.get(&key(part)).copied().unwrap_or(0.)
    }

    /// Wears a part; true when its worn finish starts to show ([`WORN_LOOK`]).
    pub fn add(&mut self, part: &PartRef, amount: f32) -> bool {
        if !amount.is_finite() || amount <= 0. {
            return false;
        }
        let wear = self.parts.entry(key(part)).or_insert(0.);
        let before = *wear;
        *wear = (*wear + amount).min(WORN * 2.);
        // True when the look changes: the worn finish starts to show.
        before < WORN_LOOK && *wear >= WORN_LOOK
    }

    /// Wears the parts of `board` (new finishes) by `usage`; true when one
    /// wore through. The default skater's parts do not wear.
    pub fn ride(&mut self, board: &BoardChoice, usage: &Usage) -> bool {
        let amounts = usage.wear();
        let mut through = false;
        for part in Part::ALL {
            if let Some(wanted) = board.get(part) {
                through |= self.add(wanted, amounts[part.index()]);
            }
        }
        through
    }

    /// A new one of `part`: its wear starts over.
    pub fn renew(&mut self, part: &PartRef) {
        self.parts.remove(&key(part));
    }

    /// The board as it looks after its wear: each part worn halfway or more
    /// in its worn finish, when Skate 3 has one.
    pub fn look(&self, board: &BoardChoice, library: &Library) -> BoardChoice {
        let mut out = board.clone();
        for part in Part::ALL {
            if let Some(wanted) = board.get(part)
                && self.of(wanted) >= WORN_LOOK
                && let Some(worn) = library.worn_of(part, wanted)
            {
                out.set(part, Some(worn));
            }
        }
        out
    }

    /// One `asset:material wear` line each.
    pub fn to_text(&self) -> String {
        self.parts.iter().map(|(k, v)| format!("{k} {v:.4}\n")).collect()
    }

    /// Reads [`Self::to_text`]; damaged lines are skipped.
    pub fn from_text(text: &str) -> Self {
        let mut wear = Self::default();
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let (Some(part), Some(value), None) = (words.next(), words.next(), words.next()) else { continue };
            let Some((asset, material)) = part.split_once(':') else { continue };
            let hex = |s: &str| !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric());
            if !hex(asset) || !hex(material) {
                continue;
            }
            if let Ok(value) = value.parse::<f32>()
                && value.is_finite()
                && value > 0.
            {
                wear.parts.insert(part.to_ascii_lowercase(), value.min(WORN * 2.));
            }
        }
        wear
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decks_scuff_in_stages_and_blows_wear_them() {
        assert_eq!(scuff_stage(0.), None);
        assert_eq!(scuff_stage(0.2), Some(1));
        assert_eq!(scuff_stage(0.5), Some(2));
        assert_eq!(scuff_stage(0.9), Some(3));
        let blows = Usage { hits: 10, ..Default::default() };
        assert!(blows.wear()[0] > 0.2, "ten blows scuff a deck: {:?}", blows.wear());
    }

    use super::*;

    #[test]
    fn riding_wears_each_part_its_own_way() {
        let usage = Usage { rolled: 1000., truck_grind: 60., deck_slide: 0., powerslide: 0., landings: 100, bails: 2, hits: 0 };
        let [deck, trucks, wheels] = usage.wear();
        assert!(trucks > deck && trucks > 0.2, "grinding wears the trucks most: {deck} {trucks} {wheels}");
        assert!((wheels - 0.25).abs() < 1e-4);
        let mut board = BoardChoice::default();
        board.set(Part::Deck, Some(PartRef::new("d1", "m1")));
        let mut wear = Wear::default();
        assert!(!wear.ride(&board, &usage));
        let slides = Usage { deck_slide: 300., ..Default::default() };
        assert!(wear.ride(&board, &slides), "five minutes of boardslides wear a deck through");
        assert!(!wear.ride(&board, &slides), "worn through only once");
        assert_eq!(wear.of(&PartRef::new("t1", "m5")), 0.);
        let back = Wear::from_text(&wear.to_text());
        assert!((back.of(&PartRef::new("d1", "m1")) - wear.of(&PartRef::new("d1", "m1"))).abs() < 1e-3);
        assert_eq!(Wear::from_text("z-z:1 0.5\nd1:m1 nan\nd1:m1\n"), Wear::default());
        wear.renew(&PartRef::new("d1", "m1"));
        assert_eq!(wear, Wear::default());
    }

    #[test]
    fn a_worn_through_part_shows_its_worn_finish() {
        const LIBRARY: &str = r#"{"models":{"d1":{"slot":"SkateBoard","name":"Deck","materials":["a1","a2","b1"],"scene":"x.glb"}},
            "materials":{"a1":{"name":"Flames","diffuse":"a.png"},"a2":{"name":"Flames (Worn)","diffuse":"a.png"},"b1":{"name":"Anchor","diffuse":"b.png"}}}"#;
        let library = Library::from_json(std::path::Path::new("/x"), LIBRARY).unwrap();
        let mut board = BoardChoice::default();
        board.set(Part::Deck, Some(PartRef::new("d1", "a1")));
        let mut wear = Wear::default();
        assert_eq!(wear.look(&board, &library), board);
        wear.add(&PartRef::new("d1", "a1"), 1.0);
        assert_eq!(wear.look(&board, &library).get(Part::Deck), Some(&PartRef::new("d1", "a2")));
        board.set(Part::Deck, Some(PartRef::new("d1", "b1")));
        wear.add(&PartRef::new("d1", "b1"), 5.0);
        assert_eq!(wear.look(&board, &library), board, "no worn Anchor: it stays as it is");
    }
}

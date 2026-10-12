//! Stickers: Skate 3's graphics-editor logos stuck on a deck's underside
//! (made by `skate-ped --weapon --game`, listed in
//! `private/boards/stickers.json`).

use serde::Deserialize;

use crate::PartRef;

/// One sticker as a store lists it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Sticker {
    pub id: String,
    pub title: String,
    pub group: String,
}

#[derive(Deserialize)]
struct List {
    stickers: Vec<Sticker>,
}

/// The stickers listed in `stickers.json` text.
pub fn parse(text: &str) -> Vec<Sticker> {
    serde_json::from_str::<List>(text).map(|l| l.stickers).unwrap_or_default()
}

/// The model of sticker `id`. One for every deck: the middle of the
/// underside, where it sits, is alike on Skate's deck shapes, and a model
/// per shape (672 in all) took GTA past what it loads (test 10-10 09:29:
/// no part models at all).
pub fn sticker_model(_deck: Option<&PartRef>, id: &str) -> String {
    format!("sk3_k_{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stickers_are_read_from_their_list() {
        let list = parse(r#"{"stickers":[{"id":"dc_colour","title":"DC (colour)","group":"Brand logos"}]}"#);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "DC (colour)");
        assert!(parse("not json").is_empty());
        assert_eq!(sticker_model(None, "dc_colour"), "sk3_k_dc_colour");
    }
}

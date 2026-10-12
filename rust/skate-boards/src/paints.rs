//! Wheels in a colour and finish of the player's choosing, as cars are
//! painted: Skate 3's plain white wheel ("Whitey") in each colour, matte,
//! gloss or metallic. The finish sets the urethane too: matte wheels are
//! soft and grip, gloss ones are medium, metallic ones hard and slide.
//! They ride as a wheel part (`PartRef` asset `paint`), so the store, the
//! garage and the models (`sk3_w_paint_<id>`) take them as any wheel.

use crate::PartRef;

/// The asset name painted wheels go by.
pub const ASSET: &str = "paint";
/// Skate 3's plain wheel the paints are laid on.
pub const BASE: &str = "Whitey";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    Matte,
    Gloss,
    Metallic,
}

impl Finish {
    pub fn label(self) -> &'static str {
        match self {
            Self::Matte => "Matte",
            Self::Gloss => "Gloss",
            Self::Metallic => "Metallic",
        }
    }

    /// How hard the wheels are: 0 soft (grip, quiet) to 1 hard (slide).
    pub fn hardness(self) -> f32 {
        match self {
            Self::Matte => 0.25,
            Self::Gloss => 0.6,
            Self::Metallic => 0.95,
        }
    }

    /// The durometer printed on them.
    pub fn durometer(self) -> u32 {
        match self {
            Self::Matte => 78,
            Self::Gloss => 92,
            Self::Metallic => 101,
        }
    }

    /// Shine for the models' specular map (0 to 255).
    pub fn shine(self) -> u8 {
        match self {
            Self::Matte => 4,
            Self::Gloss => 140,
            Self::Metallic => 255,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Paint {
    pub id: String,
    pub name: String,
    pub rgb: [u8; 3],
    pub finish: Finish,
}

impl Paint {
    pub fn part_ref(&self) -> PartRef {
        PartRef::new(ASSET, &format!("paint_{}", self.id))
    }
}

const COLOURS: [(&str, &str, [u8; 3]); 14] = [
    ("white", "White", [235, 235, 230]),
    ("black", "Black", [30, 30, 32]),
    ("grey", "Grey", [130, 132, 136]),
    ("red", "Red", [190, 30, 34]),
    ("orange", "Orange", [230, 110, 30]),
    ("yellow", "Yellow", [235, 200, 40]),
    ("lime", "Lime", [150, 210, 40]),
    ("green", "Green", [40, 150, 60]),
    ("teal", "Teal", [30, 160, 160]),
    ("blue", "Blue", [40, 80, 190]),
    ("navy", "Navy", [25, 35, 90]),
    ("purple", "Purple", [120, 55, 165]),
    ("pink", "Pink", [235, 115, 175]),
    ("gold", "Gold", [205, 165, 70]),
];

/// Every paint on sale: each colour in each finish.
pub fn catalog() -> Vec<Paint> {
    let mut out = Vec::new();
    for finish in [Finish::Gloss, Finish::Matte, Finish::Metallic] {
        for (id, name, rgb) in COLOURS {
            let tag = match finish {
                Finish::Matte => "m",
                Finish::Gloss => "g",
                Finish::Metallic => "x",
            };
            out.push(Paint { id: format!("{id}{tag}"), name: format!("{name} {}", finish.label()), rgb, finish });
        }
    }
    out
}

/// The truck the paints are laid on (the plainest, near white).
pub const TRUCK_BASE: &str = "Matte White";

/// The plain deck painted for one-colour decks (Skate's stampable deck's
/// white), and its shape's key (`crate::grips::deck_key`): a painted deck
/// takes that shape's griptapes and scuffs.
pub const DECK_BASE: &str = "White 1";
pub const DECK_KEY: &str = "a841";

/// Where a truck's paint goes: the baseplate on the deck, or the hanger and
/// axle under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TruckPart {
    Base,
    Hanger,
}

impl TruckPart {
    pub fn label(self) -> &'static str {
        match self {
            Self::Base => "baseplate",
            Self::Hanger => "hanger",
        }
    }

    /// Model names: `sk3_tb_<paint>`, `sk3_th_<paint>`.
    pub fn model(self, paint: &str) -> String {
        format!("sk3_{}_{}", if self == Self::Base { "tb" } else { "th" }, paint)
    }

    /// The truck's baseplate: within this far of its deck-side face (m).
    pub const BASE_DEPTH: f32 = 0.012;
}

/// A paint by its id.
pub fn by_id(id: &str) -> Option<Paint> {
    catalog().into_iter().find(|p| p.id == id)
}

/// The paint a wheel part is, if it is one.
pub fn of(part: &PartRef) -> Option<Paint> {
    if part.asset != ASSET {
        return None;
    }
    let id = part.material.strip_prefix("paint_")?;
    catalog().into_iter().find(|p| p.id == id)
}

/// The painted wheel's texture from the plain one: its shading kept, its
/// white made the paint's colour (metallic: more contrast, as polished).
pub fn tint(rgba: &mut [u8], paint: &Paint) {
    // The plain wheel's light, as a share of its brightest (its body).
    let light = |p: &[u8]| (f32::from(p[0]) + f32::from(p[1]) + f32::from(p[2])) / (3. * 255.);
    let top = rgba.chunks_exact(4).map(light).fold(0.05f32, f32::max);
    for p in rgba.chunks_exact_mut(4) {
        let l = (light(p) / top).clamp(0., 1.);
        // Its shading kept but soft: the faint marks on the plain wheel do
        // not turn into dark prints on a colour.
        let shade = match paint.finish {
            Finish::Metallic => 0.55 + 0.6 * l * l,
            Finish::Gloss => 0.78 + 0.27 * l,
            Finish::Matte => 0.82 + 0.18 * l,
        };
        for (c, base) in p.iter_mut().take(3).zip(paint.rgb) {
            *c = (f32::from(base) * shade).round().clamp(0., 255.) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paints_are_wheels_with_distinct_ids_and_a_finish_hardness() {
        let all = catalog();
        assert_eq!(all.len(), 42);
        let mut ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        let red = all.iter().find(|p| p.id == "redx").unwrap();
        assert_eq!(of(&red.part_ref()), Some(red.clone()));
        assert!(of(&PartRef::new("87d0fa0ea0366b23", "2c7f381700171ac6")).is_none());
        assert!(Finish::Matte.hardness() < 0.5 && Finish::Metallic.hardness() > 0.5);
        let mut px = [255, 255, 255, 255, 128, 128, 128, 255];
        tint(&mut px, &all[3]);
        assert!(px[0] > px[1] && px[4] < px[0], "red, shading kept: {px:?}");
    }
}

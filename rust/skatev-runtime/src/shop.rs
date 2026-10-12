//! Skateboards for sale in GTA V's street clothing stores, beside the
//! clothes: Skate 3's catalog. Binco, the Discount Store and Suburban sell
//! complete boards and every part on its own, so a board can be built part
//! by part in any of them; Binco and the Discount Store are cheap, Suburban
//! asks street prices. Ponsonbys, a boutique, sells none. Every part is sold new: parts
//! wear by being ridden (`skate_boards::wear`), and buying the one you ride
//! again gets a new one. What the player owns and rides is kept in a small
//! text file.

use std::collections::BTreeSet;

use skate_boards::{BoardChoice, Item, Library, Part, PartRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Binco and the Discount Store.
    Low,
    /// Suburban.
    Mid,
    /// Ponsonbys.
    High,
}

impl Tier {
    pub fn store(self) -> &'static str {
        match self {
            Self::Low => "Binco & Discount Store",
            Self::Mid => "Suburban",
            Self::High => "Ponsonbys",
        }
    }

    /// The interior names of GTA V's clothing stores, by chain.
    pub const INTERIORS: [(&'static str, Tier); 3] =
        [("v_clotheslo", Tier::Low), ("v_clothesmid", Tier::Mid), ("v_clotheshi", Tier::High)];

    /// Street stores sell boards; the boutique does not.
    pub fn sells_boards(self) -> bool {
        self != Self::High
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "low" | "binco" | "discount" => Some(Self::Low),
            "mid" | "suburban" => Some(Self::Mid),
            "high" | "ponsonbys" => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Goods {
    Part(Part, PartRef),
    Complete(BoardChoice),
    /// A griptape (`skate_boards::grips`), laid on whatever deck is ridden.
    Grip(String),
    /// A sticker (`skate_boards::stickers`): one of Skate's logos under the
    /// ridden deck.
    Sticker(String),
    /// A truck's baseplate or hanger painted (`skate_boards::paints`).
    TruckPaint(skate_boards::paints::TruckPart, String),
}

/// The part an offer is about, for the store's part view (Skate shows a
/// part alone: wheels in their pack, trucks, the deck); none for a
/// complete board.
pub fn focus_of(goods: &Goods) -> Option<Part> {
    match goods {
        Goods::Part(part, _) => Some(*part),
        Goods::Grip(_) | Goods::Sticker(_) => Some(Part::Deck),
        Goods::TruckPaint(..) => Some(Part::Trucks),
        Goods::Complete(_) => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub title: String,
    pub goods: Goods,
    pub price: u32,
    /// The finish whose texture previews the offer.
    pub preview: String,
    /// The group it is listed under within its section (a part's maker),
    /// none for a plain list.
    pub group: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: String,
    pub offers: Vec<Offer>,
}

fn fnv(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// What is priced: each kind of part has its own street price range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Deck,
    Trucks,
    Wheels,
    Grip,
    Sticker,
    /// A truck's baseplate or hanger painted.
    Paint,
}

impl Kind {
    /// A skate shop's street price range in dollars (a mid-range store),
    /// cheap brand to pro model: a deck 45-95, a pair of trucks 40-90, a set
    /// of wheels 25-60, a sheet of griptape 10-20, a sticker 5, a truck part
    /// painted 20-30. Real prices' proportions, in whole $5 steps.
    fn range(self) -> (u32, u32) {
        match self {
            Kind::Deck => (45, 95),
            Kind::Trucks => (40, 90),
            Kind::Wheels => (25, 60),
            Kind::Grip => (10, 20),
            Kind::Sticker => (5, 5),
            Kind::Paint => (20, 30),
        }
    }

    pub fn of(part: Part) -> Kind {
        match part {
            Part::Deck => Kind::Deck,
            Part::Trucks => Kind::Trucks,
            Part::Wheels => Kind::Wheels,
        }
    }
}

/// A part's steady price (the same each visit) in its kind's range, in $5
/// steps; the low-end stores (Binco, Discount Store) ask about a fifth less.
pub fn price_of(tier: Tier, kind: Kind, key: &str) -> u32 {
    let (lo, hi) = kind.range();
    let base = lo + (fnv(key) % ((hi - lo) / 5 + 1)) * 5;
    match tier {
        Tier::Low => ((base * 4 / 5).div_ceil(5) * 5).max(5),
        Tier::Mid => base,
        Tier::High => base * 3,
    }
}

/// A deck's price (the old single range's stand-in for tests and callers).
pub fn price(tier: Tier, key: &str) -> u32 {
    price_of(tier, Kind::Deck, key)
}

/// A complete board: its parts' prices, a tenth off, in $5 steps.
fn bundle(prices: &[u32]) -> u32 {
    let sum: u32 = prices.iter().sum();
    (sum * 9 / 10).div_ceil(5) * 5
}

fn part_offer(tier: Tier, item: &Item) -> Offer {
    Offer {
        title: item.title(),
        goods: Goods::Part(item.part, item.part_ref()),
        price: price_of(tier, Kind::of(item.part), &format!("{}:{}", item.asset, item.material)),
        preview: item.material.clone(),
        group: Some(item.brand.clone()),
    }
}

/// What a store of `tier` sells: complete boards first, then each part on
/// its own to build a board from.
pub fn stock(library: &Library, tier: Tier) -> Vec<Section> {
    let mut sections = Vec::new();
    if !tier.sells_boards() {
        return sections;
    }
    let (decks, trucks, wheels) = (library.new_items(Part::Deck), library.new_items(Part::Trucks), library.new_items(Part::Wheels));
    let completes: Vec<Offer> = decks
        .iter()
        .map(|deck| {
            let key = format!("{}:{}", deck.asset, deck.material);
            let h = fnv(&key);
            let mut board = BoardChoice::default();
            board.set(Part::Deck, Some(deck.part_ref()));
            let truck = (!trucks.is_empty()).then(|| trucks[(h as usize / 7) % trucks.len()]);
            let wheel = (!wheels.is_empty()).then(|| wheels[(h as usize / 13) % wheels.len()]);
            board.set(Part::Trucks, truck.map(Item::part_ref));
            board.set(Part::Wheels, wheel.map(Item::part_ref));
            let mut title = format!("{} complete", deck.label);
            if let (Some(t), Some(w)) = (truck, wheel) {
                title = format!("{title} ({} / {})", t.label, w.label);
            }
            let prices: Vec<u32> = [Some(*deck), truck, wheel]
                .into_iter()
                .flatten()
                .map(|item| part_offer(tier, item).price)
                .collect();
            Offer {
                title,
                goods: Goods::Complete(board),
                price: bundle(&prices),
                preview: deck.material.clone(), group: None,
            }
        })
        .collect();
    if !completes.is_empty() {
        sections.push(Section { title: "Complete boards".into(), offers: completes });
    }
    for &part in &Part::ALL {
        let chosen = library.new_items(part);
        if !chosen.is_empty() {
            sections.push(Section {
                title: format!("Build: {}", part.label()),
                offers: chosen.into_iter().map(|item| part_offer(tier, item)).collect(),
            });
        }
        // The griptape after the deck, painted wheels after Skate's.
        if part == Part::Deck {
            sections.push(Section { title: "Build: Deck paint".into(), offers: deck_paint_offers(tier) });
            sections.push(Section { title: "Build: Griptape".into(), offers: grip_offers(Some(library), tier) });
            let stickers = sticker_offers(library, tier);
            if !stickers.is_empty() {
                sections.push(Section { title: "Build: Sticker".into(), offers: stickers });
            }
        }
        if part == Part::Trucks {
            for side in [skate_boards::paints::TruckPart::Base, skate_boards::paints::TruckPart::Hanger] {
                sections.push(Section { title: format!("Build: Truck {} paint", side.label()), offers: truck_paint_offers(tier, side) });
            }
        }
        if part == Part::Wheels {
            sections.push(Section { title: "Build: Wheel paint".into(), offers: paint_offers(tier) });
        }
    }
    sections
}

/// Painted wheels: every colour matte (soft), gloss or metallic (hard).
pub fn paint_offers(tier: Tier) -> Vec<Offer> {
    skate_boards::paints::catalog()
        .into_iter()
        .map(|paint| {
            let part = paint.part_ref();
            Offer {
                title: format!("{} wheels ({}a)", paint.name, paint.finish.durometer()),
                price: price_of(tier, Kind::Wheels, &format!("{}:{}", part.asset, part.material)),
                preview: part.material.clone(),
                goods: Goods::Part(Part::Wheels, part),
                // Listed by paint type first, then the colours, like GTA's car paint.
                group: Some(paint.finish.label().to_owned()),
            }
        })
        .collect()
}

/// One-colour decks: Skate's plain deck in every colour and finish.
pub fn deck_paint_offers(tier: Tier) -> Vec<Offer> {
    skate_boards::paints::catalog()
        .into_iter()
        .map(|paint| {
            let part = paint.part_ref();
            Offer {
                title: format!("{} deck", paint.name),
                price: price_of(tier, Kind::Deck, &format!("deck:{}:{}", part.asset, part.material)),
                preview: part.material.clone(),
                goods: Goods::Part(Part::Deck, part),
                group: Some(paint.finish.label().to_owned()),
            }
        })
        .collect()
}

/// Truck paint for the baseplate or the hanger ($20-30).
pub fn truck_paint_offers(tier: Tier, side: skate_boards::paints::TruckPart) -> Vec<Offer> {
    skate_boards::paints::catalog()
        .into_iter()
        .map(|paint| Offer {
            title: format!("{} {}", paint.name, side.label()),
            price: price_of(tier, Kind::Paint, &format!("truck{}:{}", side.label(), paint.id)),
            preview: paint.id.clone(),
            group: Some(paint.finish.label().to_owned()),
            goods: Goods::TruckPaint(side, paint.id),
        })
        .collect()
}

/// "baseplate" or "hanger" as a truck part.
fn truck_side(word: &str) -> Option<skate_boards::paints::TruckPart> {
    match word {
        "baseplate" => Some(skate_boards::paints::TruckPart::Base),
        "hanger" => Some(skate_boards::paints::TruckPart::Hanger),
        _ => None,
    }
}

/// Skate's logos as stickers ($5), by brand or team.
pub fn sticker_offers(library: &Library, tier: Tier) -> Vec<Offer> {
    library
        .stickers()
        .iter()
        .map(|s| Offer {
            title: format!("{} sticker", s.title),
            price: price_of(tier, Kind::Sticker, &format!("sticker:{}", s.id)),
            preview: s.id.clone(),
            // Brand logos by their maker (its black-and-white and colour
            // versions together, B97); team logos as one group.
            group: Some(sticker_group(s)),
            goods: Goods::Sticker(s.id.clone()),
        })
        .collect()
}

/// The group a sticker is listed under: a brand logo's maker (its title
/// before the version in brackets), else the sticker's own group.
fn sticker_group(s: &skate_boards::stickers::Sticker) -> String {
    match s.title.split_once(" (") {
        Some((maker, _)) if s.group == "Brand logos" => maker.to_owned(),
        _ => s.group.clone(),
    }
}

/// The griptapes on sale ($10-15).
/// The griptape id for "none": the deck's own grip, as Skate 3 draws it.
pub const DECK_GRIP: &str = "";

pub fn grip_offers(library: Option<&Library>, tier: Tier) -> Vec<Offer> {
    // First the deck's own grip (no griptape laid on it): free, always owned.
    let own = Offer {
        title: "The deck's own grip (no griptape)".into(),
        price: 0,
        preview: String::new(),
        group: None,
        goods: Goods::Grip(DECK_GRIP.into()),
    };
    std::iter::once(own)
        .chain(skate_boards::grips::catalog(library).into_iter().map(|grip| Offer {
            title: format!("{} griptape", grip.name),
            price: price_of(tier, Kind::Grip, &format!("grip:{}", grip.id)),
            preview: grip.id.clone(),
            group: None,
            goods: Goods::Grip(grip.id),
        }))
        .collect()
}

/// What the player owns and the board they ride.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Garage {
    pub owned: BTreeSet<(Part, PartRef)>,
    pub riding: BoardChoice,
    /// Griptapes bought, and the one on the board (none: Skate's own).
    pub grips: BTreeSet<String>,
    pub grip: Option<String>,
    /// Stickers bought, and the one under the deck (none: no sticker).
    pub stickers: BTreeSet<String>,
    pub sticker: Option<String>,
    /// Truck paints bought, and those on the trucks (none: the truck's own).
    pub truck_paints: BTreeSet<(skate_boards::paints::TruckPart, String)>,
    pub truck_base: Option<String>,
    pub truck_hanger: Option<String>,
}

impl Garage {
    pub fn owns(&self, goods: &Goods) -> bool {
        match goods {
            Goods::Part(part, value) => self.owned.contains(&(*part, value.clone())),
            Goods::Complete(board) => Part::ALL
                .iter()
                .all(|&part| board.get(part).is_none_or(|value| self.owned.contains(&(part, value.clone())))),
            Goods::Grip(id) => id == DECK_GRIP || self.grips.contains(id),
            Goods::Sticker(id) => self.stickers.contains(id),
            Goods::TruckPaint(side, id) => self.truck_paints.contains(&(*side, id.clone())),
        }
    }

    /// The paint on a truck part, if any.
    pub fn truck_paint(&self, side: skate_boards::paints::TruckPart) -> Option<&str> {
        match side {
            skate_boards::paints::TruckPart::Base => self.truck_base.as_deref(),
            skate_boards::paints::TruckPart::Hanger => self.truck_hanger.as_deref(),
        }
    }

    pub fn rides(&self, goods: &Goods) -> bool {
        match goods {
            Goods::Part(part, value) => self.riding.get(*part) == Some(value),
            Goods::Complete(board) => self.riding == *board,
            Goods::Grip(id) if id == DECK_GRIP => self.grip.is_none(),
            Goods::Grip(id) => self.grip.as_deref() == Some(id.as_str()),
            Goods::Sticker(id) => self.sticker.as_deref() == Some(id.as_str()),
            Goods::TruckPaint(side, id) => self.truck_paint(*side) == Some(id.as_str()),
        }
    }

    pub fn add(&mut self, goods: &Goods) {
        match goods {
            Goods::Part(part, value) => {
                self.owned.insert((*part, value.clone()));
            }
            Goods::Complete(board) => {
                for part in Part::ALL {
                    if let Some(value) = board.get(part) {
                        self.owned.insert((part, value.clone()));
                    }
                }
            }
            Goods::Grip(id) => {
                self.grips.insert(id.clone());
            }
            Goods::Sticker(id) => {
                self.stickers.insert(id.clone());
            }
            Goods::TruckPaint(side, id) => {
                self.truck_paints.insert((*side, id.clone()));
            }
        }
    }

    pub fn ride(&mut self, goods: &Goods) {
        match goods {
            Goods::Part(part, value) => self.riding.set(*part, Some(value.clone())),
            Goods::Complete(board) => self.riding = board.clone(),
            Goods::Grip(id) => self.grip = (id != DECK_GRIP).then(|| id.clone()),
            Goods::Sticker(id) => self.sticker = Some(id.clone()),
            Goods::TruckPaint(skate_boards::paints::TruckPart::Base, id) => self.truck_base = Some(id.clone()),
            Goods::TruckPaint(skate_boards::paints::TruckPart::Hanger, id) => self.truck_hanger = Some(id.clone()),
        }
    }

    /// Goes back to the default skater's board (and Skate's own grip).
    pub fn ride_default(&mut self) {
        self.riding = BoardChoice::default();
        self.grip = None;
        self.sticker = None;
        self.truck_base = None;
        self.truck_hanger = None;
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from("# Skate 3 for GTA V: skateboards you bought and the one you ride.\n");
        for (part, value) in &self.owned {
            out.push_str(&format!("owned {} {}:{}\n", part.label().to_lowercase(), value.asset, value.material));
        }
        for id in &self.grips {
            out.push_str(&format!("owned grip {id}\n"));
        }
        out.push_str(&format!("riding {}\n", self.riding.to_text()));
        if let Some(id) = &self.grip {
            out.push_str(&format!("grip {id}\n"));
        }
        for id in &self.stickers {
            out.push_str(&format!("owned sticker {id}\n"));
        }
        if let Some(id) = &self.sticker {
            out.push_str(&format!("sticker {id}\n"));
        }
        for (side, id) in &self.truck_paints {
            out.push_str(&format!("owned truckpaint {} {id}\n", side.label()));
        }
        for side in [skate_boards::paints::TruckPart::Base, skate_boards::paints::TruckPart::Hanger] {
            if let Some(id) = self.truck_paint(side) {
                out.push_str(&format!("truckpaint {} {id}\n", side.label()));
            }
        }
        out
    }

    pub fn from_text(text: &str) -> Self {
        let mut garage = Self::default();
        for line in text.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("owned ") {
                let mut words = rest.split_whitespace();
                let part = match words.next() {
                    Some("deck") => Part::Deck,
                    Some("trucks") => Part::Trucks,
                    Some("wheels") => Part::Wheels,
                    Some("grip") => {
                        if let Some(id) = words.next() {
                            garage.grips.insert(id.to_owned());
                        }
                        continue;
                    }
                    Some("sticker") => {
                        if let Some(id) = words.next() {
                            garage.stickers.insert(id.to_owned());
                        }
                        continue;
                    }
                    Some("truckpaint") => {
                        if let (Some(side), Some(id)) = (words.next().and_then(truck_side), words.next()) {
                            garage.truck_paints.insert((side, id.to_owned()));
                        }
                        continue;
                    }
                    _ => continue,
                };
                if let Some((asset, material)) = words.next().and_then(|w| w.split_once(':')) {
                    garage.owned.insert((part, PartRef::new(asset, material)));
                }
            } else if let Some(rest) = line.strip_prefix("riding") {
                garage.riding = BoardChoice::from_text(rest.trim());
            } else if let Some(rest) = line.strip_prefix("grip ") {
                garage.grip = Some(rest.trim().to_owned()).filter(|id| !id.is_empty());
            } else if let Some(rest) = line.strip_prefix("sticker ") {
                garage.sticker = Some(rest.trim().to_owned()).filter(|id| !id.is_empty());
            } else if let Some(rest) = line.strip_prefix("truckpaint ") {
                let mut words = rest.split_whitespace();
                match (words.next().and_then(truck_side), words.next()) {
                    (Some(skate_boards::paints::TruckPart::Base), Some(id)) => garage.truck_base = Some(id.to_owned()),
                    (Some(skate_boards::paints::TruckPart::Hanger), Some(id)) => garage.truck_hanger = Some(id.to_owned()),
                    _ => {}
                }
            }
        }
        if garage.grip.as_ref().is_some_and(|id| !garage.grips.contains(id)) {
            garage.grip = None;
        }
        if garage.sticker.as_ref().is_some_and(|id| !garage.stickers.contains(id)) {
            garage.sticker = None;
        }
        use skate_boards::paints::TruckPart;
        if garage.truck_base.as_ref().is_some_and(|id| !garage.truck_paints.contains(&(TruckPart::Base, id.clone()))) {
            garage.truck_base = None;
        }
        if garage.truck_hanger.as_ref().is_some_and(|id| !garage.truck_paints.contains(&(TruckPart::Hanger, id.clone()))) {
            garage.truck_hanger = None;
        }
        // Only what was bought can be ridden.
        for part in Part::ALL {
            if let Some(value) = garage.riding.get(part).cloned()
                && !garage.owned.contains(&(part, value))
            {
                garage.riding.set(part, None);
            }
        }
        garage
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Up,
    Down,
    Left,
    Right,
    Accept,
    Back,
    /// Back to the default board.
    Reset,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Close,
    Buy(Offer),
    Ride(Offer),
    RideDefault,
}

/// The store's menu.
pub struct Menu {
    pub tier: Tier,
    pub sections: Vec<Section>,
    pub section: usize,
    pub row: usize,
    /// The group opened in a section listed by groups (a maker), and the
    /// row it was opened from.
    pub group: Option<(String, usize)>,
}

/// One line of the menu: a group to open, with how many it holds, or an
/// offer.
#[derive(Clone, Debug, PartialEq)]
pub enum Line<'a> {
    Group(String, usize),
    Offer(&'a Offer),
}

/// Rows the menu shows at once.
pub const VISIBLE: usize = 10;

impl Menu {
    pub fn new(tier: Tier, sections: Vec<Section>) -> Self {
        Self {
            tier,
            sections,
            section: 0,
            row: 0,
            group: None,
        }
    }

    pub fn offers(&self) -> &[Offer] {
        self.sections.get(self.section).map_or(&[], |s| s.offers.as_slice())
    }

    /// Whether the current section is listed by groups (makers).
    fn grouped(&self) -> bool {
        self.offers().iter().any(|o| o.group.is_some())
    }

    /// What the menu lists now: a grouped section's groups (alphabetical,
    /// with how many offers each), the opened group's offers, or a plain
    /// section's offers.
    pub fn lines(&self) -> Vec<Line<'_>> {
        if !self.grouped() {
            return self.offers().iter().map(Line::Offer).collect();
        }
        match &self.group {
            Some((group, _)) => self.offers().iter().filter(|o| o.group.as_ref() == Some(group)).map(Line::Offer).collect(),
            None => {
                let mut groups: Vec<(String, usize)> = Vec::new();
                for o in self.offers() {
                    let g = o.group.clone().unwrap_or_default();
                    match groups.iter_mut().find(|(n, _)| *n == g) {
                        Some(entry) => entry.1 += 1,
                        None => groups.push((g, 1)),
                    }
                }
                groups.sort_by_key(|(n, _)| n.to_lowercase());
                groups.into_iter().map(|(n, c)| Line::Group(n, c)).collect()
            }
        }
    }

    pub fn current(&self) -> Option<&Offer> {
        match self.lines().get(self.row) {
            Some(Line::Offer(o)) => Some(*o),
            _ => None,
        }
    }

    /// The first row shown, keeping the current row in view.
    pub fn first_visible(&self) -> usize {
        self.row.saturating_sub(VISIBLE / 2).min(self.lines().len().saturating_sub(VISIBLE))
    }

    pub fn handle(&mut self, input: Input, garage: &Garage) -> Action {
        let count = self.lines().len();
        match input {
            Input::Up if count > 0 => self.row = (self.row + count - 1) % count,
            Input::Down if count > 0 => self.row = (self.row + 1) % count,
            Input::Left | Input::Right if !self.sections.is_empty() => {
                let n = self.sections.len();
                self.section = if input == Input::Left { (self.section + n - 1) % n } else { (self.section + 1) % n };
                self.row = 0;
                self.group = None;
            }
            // A group opens; back closes it again, on the row it was.
            Input::Accept if matches!(self.lines().get(self.row), Some(Line::Group(..))) => {
                if let Some(Line::Group(name, _)) = self.lines().get(self.row).cloned() {
                    self.group = Some((name, self.row));
                    self.row = 0;
                }
            }
            Input::Back if self.group.is_some() => {
                if let Some((_, row)) = self.group.take() {
                    self.row = row;
                }
            }
            Input::Accept => {
                if let Some(offer) = self.current() {
                    // Owned, it is ridden; ridden already, a new one is
                    // bought to replace the worn one.
                    return if garage.owns(&offer.goods) && !garage.rides(&offer.goods) {
                        Action::Ride(offer.clone())
                    } else {
                        Action::Buy(offer.clone())
                    };
                }
            }
            Input::Back => return Action::Close,
            Input::Reset => return Action::RideDefault,
            _ => {}
        }
        Action::None
    }
}

/// Where boards go on show in a store: a flat stretch of wall 1.5 to 6 m
/// from `eye` (about chest height), found with lines `probe` casts (it
/// gives where a line from the first point to the second meets
/// something). The wall must stand upright (met at the same distance at
/// knee and head height) and run on straight 0.9 m to either side. Returns
/// a point on it at `eye`'s height and its normal into the room, the
/// stretch nearest 3 m away.
pub fn display_wall(eye: bevy_math::Vec3, mut probe: impl FnMut(bevy_math::Vec3, bevy_math::Vec3) -> Option<bevy_math::Vec3>) -> Option<(bevy_math::Vec3, bevy_math::Vec3)> {
    use bevy_math::{Vec2, Vec3};
    const DIRECTIONS: usize = 24;
    let mut best: Option<(f32, Vec3, Vec3)> = None;
    for k in 0..DIRECTIONS {
        let dir = Vec2::from_angle(k as f32 / DIRECTIONS as f32 * std::f32::consts::TAU).extend(0.);
        let Some(hit) = probe(eye, eye + dir * 7.) else { continue };
        let d = (hit - eye).truncate().length();
        if !(1.2..=8.).contains(&d) {
            continue;
        }
        let mut along = |from: Vec3| probe(from, from + dir * (d + 1.)).map(|h| (h - from).truncate().length());
        let upright = [-0.5, 0.5].iter().all(|dz| along(eye + Vec3::Z * *dz).is_some_and(|e| (e - d).abs() < 0.15));
        let side = dir.cross(Vec3::Z).normalize();
        let (left, right) = (eye - side * 0.9, eye + side * 0.9);
        let (Some(l), Some(r)) = (probe(left, left + dir * (d + 1.)), probe(right, right + dir * (d + 1.))) else { continue };
        let tangent = (r - l).truncate();
        // Straight across: both ends near the middle's line.
        let straight = tangent.length() > 1.5 && {
            let mid = (l + r) * 0.5;
            (mid.truncate() - hit.truncate()).length() < 0.15
        };
        if !upright || !straight {
            continue;
        }
        let mut normal = tangent.perp().normalize().extend(0.);
        if normal.dot(eye - hit) < 0. {
            normal = -normal;
        }
        // Clear in front where the boards would stand: nothing hanging or
        // standing there (clothes, bags, caps) across their width and
        // height, met only by the wall itself.
        let along = tangent.normalize().extend(0.);
        let clear = [-0.8, -0.4, 0., 0.4, 0.8].iter().all(|o| {
            [-1.1, -0.75, -0.4].iter().all(|dz| {
                let from = hit + normal * 0.7 + along * *o + Vec3::Z * *dz;
                probe(from, from - normal * 1.0).is_some_and(|h| ((h - from).truncate().length() - 0.7).abs() < 0.08)
            })
        });
        if !clear {
            continue;
        }
        let score = (d - 3.).abs();
        if best.as_ref().is_none_or(|b| score < b.0) {
            best = Some((score, hit, normal));
        }
    }
    best.map(|(_, at, normal)| (at, normal))
}

/// `count` boards on show along a wall (`at` on it, `normal` into the
/// room, the floor at `floor`): standing on their tails 0.35 m apart, the
/// graphic facing out. Each in the board models' frame (length along Z
/// from the grip, the deck facing -X).
pub fn display_places(at: bevy_math::Vec3, normal: bevy_math::Vec3, floor: f32, count: usize) -> Vec<bevy_math::Mat4> {
    use bevy_math::{Mat3, Mat4, Vec3};
    let along = Vec3::Z.cross(normal).normalize();
    // X (the wheels' side) out of the wall, Z up.
    let turn = Mat3::from_cols(normal, Vec3::Z.cross(normal), Vec3::Z);
    (0..count)
        .map(|i| {
            let offset = (i as f32 - (count as f32 - 1.) / 2.) * 0.35;
            // The tail 0.1 m behind the grip on the floor, the deck off the wall.
            let base = Vec3::new(at.x, at.y, floor + 0.1) + along * offset + normal * 0.08;
            Mat4::from_translation(base) * Mat4::from_mat3(turn)
        })
        .collect()
}

/// The wall behind a store's till: the clerk (at `clerk`, facing
/// `forward`, toward the counter) stands before it. Lines cast with
/// `probe` straight back from the clerk and 0.6 m to either side must meet
/// one flat wall within 3 m. Returns a point on it at the clerk's height and
/// its normal (toward the clerk).
pub fn wall_behind(clerk: bevy_math::Vec3, forward: bevy_math::Vec3, mut probe: impl FnMut(bevy_math::Vec3, bevy_math::Vec3) -> Option<bevy_math::Vec3>) -> Option<(bevy_math::Vec3, bevy_math::Vec3)> {
    use bevy_math::Vec3;
    let back = -forward.truncate().normalize_or_zero().extend(0.);
    if back == Vec3::ZERO {
        return None;
    }
    let hit = probe(clerk, clerk + back * 3.)?;
    let d = (hit - clerk).truncate().length();
    if !(0.15..=3.).contains(&d) {
        return None;
    }
    let side = back.cross(Vec3::Z).normalize();
    let (l, r) = (clerk - side * 0.6, clerk + side * 0.6);
    let (l, r) = (probe(l, l + back * (d + 1.))?, probe(r, r + back * (d + 1.))?);
    let tangent = (r - l).truncate();
    let mid = (l + r) * 0.5;
    if tangent.length() < 1. || (mid.truncate() - hit.truncate()).length() > 0.15 {
        return None;
    }
    let mut normal = tangent.perp().normalize().extend(0.);
    if normal.dot(clerk - hit) < 0. {
        normal = -normal;
    }
    Some((hit, normal))
}

/// Boards hung behind the till like a skate shop's wall rack: lying
/// level, length along the wall, graphic out, two rows of two either side
/// of the clerk (the middle left free for them), high enough (1.45 and 1.8 m
/// over the floor) to hang above the shelves on that wall. In the board
/// models' frame (length along Z from the grip, the deck facing -X).
pub fn till_places(at: bevy_math::Vec3, normal: bevy_math::Vec3, floor: f32) -> Vec<bevy_math::Mat4> {
    use bevy_math::{Mat3, Mat4, Vec3};
    let along = Vec3::Z.cross(normal).normalize();
    // X (the graphic's side) out of the wall, Z along it, Y down: a rotation.
    let turn = Mat3::from_cols(normal, along.cross(normal), along);
    // The model runs from 0.1 m behind the grip to about 0.7 m ahead of it:
    // its middle 0.3 m from the origin.
    const MIDDLE: f32 = 0.3;
    [(-0.8f32, 1.45f32), (0.8, 1.45), (-0.8, 1.8), (0.8, 1.8)]
        .iter()
        .map(|(offset, height)| {
            let middle = Vec3::new(at.x, at.y, floor + height) + along * *offset + normal * 0.1;
            Mat4::from_translation(middle - along * MIDDLE) * Mat4::from_mat3(turn)
        })
        .collect()
}

/// Where a store shows its boards, kept for good: the store (its
/// interior's place, to the metre), the wall stretch and the floor, and
/// whether they hang behind the till (else they stand along a wall).
#[derive(Clone, Debug, PartialEq)]
pub struct ShowCase {
    pub store: (i32, i32),
    pub at: bevy_math::Vec3,
    pub normal: bevy_math::Vec3,
    pub floor: f32,
    pub till: bool,
}

impl ShowCase {
    /// Where each board goes.
    pub fn places(&self, count: usize) -> Vec<bevy_math::Mat4> {
        if self.till {
            till_places(self.at, self.normal, self.floor)
        } else {
            display_places(self.at, self.normal, self.floor, count)
        }
    }
}

/// The store key of an interior at `place`.
pub fn store_key(place: bevy_math::Vec3) -> (i32, i32) {
    (place.x.round() as i32, place.y.round() as i32)
}

/// `count` boards for a store's show on game day `day` (picked from its
/// complete boards): the same all day (looking round, stepping out and
/// back), new ones the next day, and each store its own.
pub fn show_picks<T: Clone>(boards: &[T], store: (i32, i32), day: u32, count: usize) -> Vec<T> {
    if boards.is_empty() {
        return Vec::new();
    }
    let seed = ((store.0.unsigned_abs() as usize).wrapping_mul(7919) ^ (store.1.unsigned_abs() as usize).wrapping_mul(104_729))
        .wrapping_add(day as usize * 131);
    (0..count).map(|i| boards[(seed + i * 37) % boards.len()].clone()).collect()
}

/// The shows, one line each: `show3 x y  ax ay az  nx ny  floor  till|wall`.
pub fn shows_to_text(shows: &[ShowCase]) -> String {
    let mut out = String::from("# Skate 3 for GTA V: where each store shows its boards (delete a line to place them anew).\n");
    for s in shows {
        out.push_str(&format!(
            "show3 {} {}  {:.3} {:.3} {:.3}  {:.4} {:.4}  {:.3}  {}\n",
            s.store.0, s.store.1, s.at.x, s.at.y, s.at.z, s.normal.x, s.normal.y, s.floor, if s.till { "till" } else { "wall" }
        ));
    }
    out
}

pub fn shows_from_text(text: &str) -> Vec<ShowCase> {
    text.lines()
        .filter_map(|line| {
            // Version 3: behind the till where there is a clerk (older lines
            // are placed anew).
            let mut words = line.strip_prefix("show3 ")?.split_whitespace();
            let mut number = || words.next()?.parse::<f32>().ok();
            let store = (number()? as i32, number()? as i32);
            let at = bevy_math::Vec3::new(number()?, number()?, number()?);
            let normal = bevy_math::Vec3::new(number()?, number()?, 0.);
            let floor = number()?;
            let till = words.next()? == "till";
            (normal.length() > 0.5).then_some(ShowCase { store, at, normal, floor, till })
        })
        .collect()
}

/// The preview board turned by the mouse as a trackball, in the view's own
/// axes (X ahead, Y up, Z right): sideways spins it about the up axis,
/// up and down about the right one, round and round in every direction.
pub fn trackball(orient: bevy_math::Quat, sideways: f32, up_down: f32) -> bevy_math::Quat {
    use bevy_math::Quat;
    (Quat::from_rotation_y(sideways * TURN_SPEED) * Quat::from_rotation_z(up_down * TURN_SPEED) * orient).normalize()
}

/// Radians per unit of GTA's look input.
const TURN_SPEED: f32 = 0.35;

/// The preview's first look: turned over about its length to show its
/// graphic at three quarters.
pub fn preview_start() -> bevy_math::Quat {
    bevy_math::Quat::from_rotation_z(std::f32::consts::PI - 0.7)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_decks_own_grip_is_free_owned_and_takes_the_griptape_off() {
        let offers = super::grip_offers(None, super::Tier::Mid);
        assert_eq!((offers[0].price, offers[0].goods.clone()), (0, super::Goods::Grip(super::DECK_GRIP.into())));
        let mut g = super::Garage::default();
        assert!(g.owns(&offers[0].goods) && g.rides(&offers[0].goods));
        g.ride(&super::Goods::Grip("red".into()));
        assert!(!g.rides(&offers[0].goods));
        g.ride(&offers[0].goods);
        assert_eq!(g.grip, None);
    }

    #[test]
    fn stickers_are_grouped_by_maker() {
        let sticker = |group: &str, title: &str| skate_boards::stickers::Sticker { id: "x".into(), title: title.into(), group: group.into() };
        assert_eq!(super::sticker_group(&sticker("Brand logos", "Adidas (bw)")), "Adidas");
        assert_eq!(super::sticker_group(&sticker("Brand logos", "Adidas (colour)")), "Adidas");
        assert_eq!(super::sticker_group(&sticker("Team logos", "Baker (team)")), "Team logos");
    }

    #[test]
    fn the_part_view_shows_the_part_an_offer_is_about() {
        use super::{Goods, focus_of};
        use skate_boards::{BoardChoice, Part, PartRef};
        let wheel = PartRef { asset: "m".into(), material: "w".into() };
        assert_eq!(focus_of(&Goods::Part(Part::Wheels, wheel)), Some(Part::Wheels));
        assert_eq!(focus_of(&Goods::Grip("black".into())), Some(Part::Deck));
        assert_eq!(focus_of(&Goods::TruckPaint(skate_boards::paints::TruckPart::Base, "redg".into())), Some(Part::Trucks));
        assert_eq!(focus_of(&Goods::Complete(BoardChoice::default())), None);
    }

    #[test]
    fn truck_paints_are_kept_per_part() {
        use skate_boards::paints::TruckPart;
        let mut garage = super::Garage::default();
        let base = super::Goods::TruckPaint(TruckPart::Base, "bluex".into());
        let hanger = super::Goods::TruckPaint(TruckPart::Hanger, "blackm".into());
        for g in [&base, &hanger] {
            garage.add(g);
            garage.ride(g);
        }
        let back = super::Garage::from_text(&garage.to_text());
        assert_eq!(back.truck_paint(TruckPart::Base), Some("bluex"));
        assert_eq!(back.truck_paint(TruckPart::Hanger), Some("blackm"));
        assert!(back.rides(&base) && back.owns(&hanger));
        assert_eq!(super::Garage::from_text("truckpaint baseplate redg\n").truck_base, None, "not bought, not on");
    }

    #[test]
    fn stickers_are_owned_ridden_and_kept() {
        let mut garage = super::Garage::default();
        let dc = super::Goods::Sticker("dc_colour".into());
        garage.add(&dc);
        garage.ride(&dc);
        assert!(garage.owns(&dc) && garage.rides(&dc));
        let back = super::Garage::from_text(&garage.to_text());
        assert_eq!(back.sticker.as_deref(), Some("dc_colour"));
        assert!(back.stickers.contains("dc_colour"));
        // Not bought, not stuck on; the default board has none.
        assert_eq!(super::Garage::from_text("sticker zero_bw\n").sticker, None);
        let mut reset = back.clone();
        reset.ride_default();
        assert_eq!(reset.sticker, None);
        assert_eq!(super::focus_of(&dc), Some(skate_boards::Part::Deck));
    }

    #[test]
    fn griptapes_are_sold_owned_ridden_and_kept() {
        let offers = super::grip_offers(None, super::Tier::Low);
        assert!(offers.len() >= 10);
        let red = offers.iter().find(|o| o.goods == super::Goods::Grip("red".into())).unwrap().clone();
        assert!(red.price > 0);
        let mut garage = super::Garage::default();
        assert!(!garage.owns(&red.goods));
        garage.add(&red.goods);
        garage.ride(&red.goods);
        assert!(garage.owns(&red.goods) && garage.rides(&red.goods));
        let back = super::Garage::from_text(&garage.to_text());
        assert_eq!(back.grip.as_deref(), Some("red"));
        assert!(back.grips.contains("red"));
        let mut reset = back.clone();
        reset.ride_default();
        assert_eq!(reset.grip, None);
        // A grip not bought is not ridden.
        assert_eq!(super::Garage::from_text("grip blue\n").grip, None);
    }

    #[test]
    fn boards_hang_behind_the_till_either_side_of_the_clerk() {
        use bevy_math::Vec3;
        // The clerk at (0, 0, 1) faces +y (the counter); the wall is at y = -0.8.
        let probe = |a: Vec3, b: Vec3| -> Option<Vec3> {
            let d = b - a;
            (d.y < -1e-6).then(|| (-0.8 - a.y) / d.y).filter(|t| (0. ..=1.).contains(t)).map(|t| a + d * t)
        };
        let (at, normal) = super::wall_behind(Vec3::new(0., 0., 1.), Vec3::Y, probe).unwrap();
        assert!((at.y + 0.8).abs() < 1e-4 && normal.y > 0.99, "{at} {normal}");
        let places = super::till_places(at, normal, 0.);
        assert_eq!(places.len(), 4);
        // Each board's middle (0.3 m along it from the origin) clear of the clerk.
        assert!(places.iter().all(|m| (m.w_axis.x + m.z_axis.x * 0.3).abs() > 0.5), "the middle stays free for the clerk");
        assert!(places.iter().all(|m| m.w_axis.z >= 1.44 && m.x_axis.y > 0.99), "high on the wall, graphic out");
        assert!(places.iter().all(|m| m.z_axis.z.abs() < 1e-4), "lying level along the wall");
        assert!(places.iter().all(|m| (m.determinant() - 1.).abs() < 1e-4), "a rotation, not a mirror");
        // No wall behind: none.
        assert!(super::wall_behind(Vec3::new(0., 0., 1.), Vec3::Y, |_, _| None).is_none());
    }

    #[test]
    fn boards_go_on_show_against_a_flat_wall() {
        use bevy_math::Vec3;
        // A room: walls at x = 4 (flat) and y = 2.5 (with a doorway: no
        // wall from x 1 to 2.5), the player at the origin.
        let probe = |a: Vec3, b: Vec3| -> Option<Vec3> {
            let d = b - a;
            let mut best: Option<(f32, Vec3)> = None;
            if d.x > 1e-6 {
                let t = (4. - a.x) / d.x;
                if (0. ..=1.).contains(&t) {
                    best = Some((t, a + d * t));
                }
            }
            if d.y > 1e-6 {
                let t = (2.5 - a.y) / d.y;
                let p = a + d * t;
                if (0. ..=1.).contains(&t) && !(1. ..2.5).contains(&p.x) && best.is_none_or(|(bt, _)| t < bt) {
                    best = Some((t, p));
                }
            }
            best.map(|(_, p)| p)
        };
        // Clothes on a rail before the x wall (x 3.6..3.8, y -1..1, up to 1.4):
        // the boards go elsewhere.
        let cluttered = |a: Vec3, b: Vec3| -> Option<Vec3> {
            let d = b - a;
            if d.x > 1e-6 {
                let t = (3.6 - a.x) / d.x;
                let p = a + d * t;
                if (0. ..=1.).contains(&t) && p.y.abs() < 1. && p.z < 1.4 {
                    return Some(p);
                }
            }
            probe(a, b)
        };
        let (at, _) = super::display_wall(Vec3::new(0., 0., 1.3), cluttered).unwrap();
        assert!(!((at.x - 4.).abs() < 0.3 && at.y.abs() < 1.9), "not in front of the clothes: {at}");
        let (at, normal) = super::display_wall(Vec3::new(0., 0., 1.), probe).unwrap();
        let on_x = (at.x - 4.).abs() < 1e-3 && normal.x < -0.99;
        let on_y = (at.y - 2.5).abs() < 1e-3 && normal.y < -0.99 && (at.x + 0.9 < 1. || at.x - 0.9 >= 2.5);
        assert!(on_x || on_y, "a flat wall facing the player, clear of the doorway: {at} {normal}");
        let places = super::display_places(at, normal, 0., 5);
        assert_eq!(places.len(), 5);
        for m in &places {
            let p = m.w_axis.truncate();
            assert!(((p - at).dot(normal) - 0.08).abs() < 1e-3 && (p.z - 0.1).abs() < 1e-3, "off the wall, on the floor: {p}");
            assert!(m.z_axis.z > 0.99, "standing up");
            assert!(m.x_axis.truncate().dot(normal) > 0.99, "the graphic (the wheels' side) faces the room");
        }
    }

    #[test]
    fn shows_are_kept_and_pick_the_same_decks_every_visit() {
        use bevy_math::Vec3;
        let decks: Vec<PartRef> = (0..50).map(|i| PartRef::new("deckmodel", &format!("finish{i}"))).collect();
        let a = super::show_picks(&decks, (-1188, -773), 12, 5);
        assert_eq!(a, super::show_picks(&decks, (-1188, -773), 12, 5), "the same store and day, the same decks");
        assert_ne!(a, super::show_picks(&decks, (418, -808), 12, 5), "another store, others");
        assert_ne!(a, super::show_picks(&decks, (-1188, -773), 13, 5), "the next day, others");
        let show = super::ShowCase { store: (-1188, -773), at: Vec3::new(-1187.913, -773.07, 18.712), normal: Vec3::new(-0.5967, 0.8025, 0.), floor: 17.6, till: true };
        let back = super::shows_from_text(&super::shows_to_text(std::slice::from_ref(&show)));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].store, show.store);
        assert!(back[0].till);
        assert!(back[0].at.distance(show.at) < 1e-3 && back[0].normal.distance(show.normal) < 1e-3);
        assert!(super::shows_from_text("# nothing\nshow3 broken\nshow2 1 2  3 4 5  1 0  4").is_empty(), "broken and old lines are dropped");
    }

    #[test]
    fn the_preview_turns_all_the_way_round_both_ways() {
        use bevy_math::{Quat, Vec3};
        let start = Quat::IDENTITY;
        let turns = (2. * std::f32::consts::PI / super::TURN_SPEED).round() as usize;
        let mut sideways = start;
        let mut up_down = start;
        for _ in 0..turns {
            sideways = super::trackball(sideways, 1., 0.);
            up_down = super::trackball(up_down, 0., 1.);
        }
        assert!(sideways.angle_between(start) < 0.1 && up_down.angle_between(start) < 0.1, "a full turn comes round");
        let half = super::trackball(start, 0., std::f32::consts::PI / super::TURN_SPEED);
        assert!((half * Vec3::X).dot(Vec3::X) < -0.99, "up and down turns it over");
        assert_eq!(super::trackball(start, 0., 0.), start, "no mouse, no turn");
    }

    use super::*;
    use std::path::Path;

    const LIBRARY: &str = r#"{"models":{
      "d1":{"slot":"SkateBoard","name":"Deck","materials":["a","b","c"],"scene":"x"},
      "e1":{"slot":"SkateTruck","name":"Truck","materials":["e","ef"],"scene":"x"},
      "f1":{"slot":"SkateWheel","name":"Wheel","materials":["f"],"scene":"x"}},
     "materials":{
      "a":{"name":"Flames","diffuse":"a.png"},
      "b":{"name":"Flames (Worn)","diffuse":"a.png"},
      "c":{"name":"Anchor","diffuse":"c.png"},
      "e":{"name":"Raw","diffuse":"t.png"},
      "ef":{"name":"Raw (Worn)","diffuse":"t.png"},
      "f":{"name":"White","diffuse":"w.png"}}}"#;

    fn library() -> Library {
        Library::from_json(Path::new("/x"), LIBRARY).unwrap()
    }

    #[test]
    fn every_store_sells_complete_boards_and_every_part() {
        let library = library();
        assert!(stock(&library, Tier::High).is_empty(), "the boutique sells no boards");
        for tier in [Tier::Low, Tier::Mid] {
            let sections = stock(&library, tier);
            assert_eq!(
                sections.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
                ["Complete boards", "Build: Deck", "Build: Deck paint", "Build: Griptape", "Build: Trucks", "Build: Truck baseplate paint", "Build: Truck hanger paint", "Build: Wheels", "Build: Wheel paint"],
                "{tier:?}"
            );
            assert_eq!(sections[0].offers.len(), 2, "a complete board per new deck");
            let Goods::Complete(board) = &sections[0].offers[0].goods else { panic!() };
            assert!(Part::ALL.iter().all(|&p| board.get(p).is_some()));
            assert_eq!(sections[1].offers.len(), 2, "new decks");
            assert!(sections.iter().flat_map(|s| &s.offers).all(|o| !o.title.ends_with("(Worn)")), "parts wear by riding");
        }
        for tier in [Tier::Low, Tier::Mid] {
            for section in stock(&library, tier).into_iter().skip(1) {
                for offer in section.offers {
                    if offer.goods == Goods::Grip(DECK_GRIP.into()) {
                        continue; // the deck's own grip: free
                    }
                    // Street prices: nothing over a deck's 95, nothing under a sticker's 5.
                    assert!((5..=95).contains(&offer.price) && offer.price % 5 == 0, "{tier:?} {} {}", offer.title, offer.price);
                    if section.title == "Build: Griptape" {
                        assert!(offer.price <= 20, "griptape {}", offer.price);
                    }
                }
            }
        }
        // A complete board costs its parts less a tenth, and a deck more than a set of wheels.
        for (i, deck) in ["a", "b", "c", "d", "e"].iter().enumerate() {
            assert!(price_of(Tier::Mid, Kind::Deck, deck) > price_of(Tier::Mid, Kind::Wheels, &i.to_string()));
        }
        let low = stock(&library, Tier::Low);
        let mid = stock(&library, Tier::Mid);
        assert!(low[0].offers.iter().all(|o| o.price % 5 == 0));
        assert!(mid[0].offers.iter().map(|o| o.price).sum::<u32>() > low[0].offers.iter().map(|o| o.price).sum::<u32>(), "Suburban asks more");
        assert_eq!(price(Tier::Mid, "x"), price(Tier::Mid, "x"), "prices hold still");
    }

    #[test]
    fn buying_owns_and_owning_rides() {
        let library = library();
        let mut garage = Garage::default();
        let mut menu = Menu::new(Tier::Low, stock(&library, Tier::Low));
        let Action::Buy(offer) = menu.handle(Input::Accept, &garage) else { panic!() };
        garage.add(&offer.goods);
        garage.ride(&offer.goods);
        assert!(garage.owns(&offer.goods) && garage.rides(&offer.goods));
        assert_eq!(garage.owned.len(), 3);
        assert!(matches!(menu.handle(Input::Accept, &garage), Action::Buy(_)), "the one ridden: a new one");
        menu.handle(Input::Down, &garage);
        let Action::Buy(other) = menu.handle(Input::Accept, &garage) else { panic!() };
        garage.add(&other.goods);
        assert!(matches!(menu.handle(Input::Accept, &garage), Action::Ride(_)), "owned, not ridden: ride it");
        menu.handle(Input::Up, &garage);
        assert_eq!(menu.handle(Input::Up, &garage), Action::None);
        assert_eq!(menu.row, menu.offers().len() - 1, "wraps around");
        assert_eq!(menu.handle(Input::Back, &garage), Action::Close);
        assert_eq!(menu.handle(Input::Reset, &garage), Action::RideDefault);

        let text = garage.to_text();
        assert_eq!(Garage::from_text(&text), garage);
        let mut forged = text.clone();
        forged.push_str("riding d1:ff,,\n");
        assert!(Garage::from_text(&forged).riding.is_default(), "an unbought deck is not ridden");
    }

    #[test]
    fn the_list_scrolls_with_the_row() {
        let offers: Vec<Offer> = (0..30)
            .map(|i| Offer { title: i.to_string(), goods: Goods::Part(Part::Deck, PartRef::new("a", "b")), price: 1, preview: String::new(), group: None })
            .collect();
        let mut menu = Menu::new(Tier::Mid, vec![Section { title: "Deck".into(), offers }]);
        let garage = Garage::default();
        assert_eq!(menu.first_visible(), 0);
        for _ in 0..12 {
            menu.handle(Input::Down, &garage);
        }
        let first = menu.first_visible();
        assert!(first <= 12 && 12 < first + VISIBLE, "{first}");
        menu.handle(Input::Up, &garage);
        for _ in 0..20 {
            menu.handle(Input::Down, &garage);
        }
        assert_eq!(menu.row, 1, "wrapped");
        assert_eq!(menu.first_visible(), 0);
        assert_eq!(Tier::parse("Ponsonbys"), Some(Tier::High));
    }

    #[test]
    fn parts_are_listed_by_maker_and_a_maker_opens_and_closes() {
        let offer = |title: &str, group: &str| Offer {
            title: title.into(),
            goods: Goods::Grip(title.into()),
            price: 10,
            preview: String::new(),
            group: Some(group.into()),
        };
        let offers = vec![offer("Bailout", "Zero"), offer("Baked", "Baker"), offer("Koston", "Girl"), offer("Strapped", "Zero")];
        let mut menu = Menu::new(Tier::Mid, vec![Section { title: "Deck".into(), offers }]);
        let garage = Garage::default();
        assert_eq!(menu.lines(), [Line::Group("Baker".into(), 1), Line::Group("Girl".into(), 1), Line::Group("Zero".into(), 2)]);
        assert_eq!(menu.current(), None, "a maker is no offer");
        menu.handle(Input::Down, &garage);
        menu.handle(Input::Down, &garage);
        assert_eq!(menu.handle(Input::Accept, &garage), Action::None, "opening buys nothing");
        let titles: Vec<String> = menu.lines().iter().map(|l| match l { Line::Offer(o) => o.title.clone(), Line::Group(g, _) => g.clone() }).collect();
        assert_eq!(titles, ["Bailout", "Strapped"]);
        assert!(matches!(menu.handle(Input::Accept, &garage), Action::Buy(_)));
        assert_eq!(menu.handle(Input::Back, &garage), Action::None, "back closes the maker");
        assert_eq!(menu.row, 2, "on the maker it was");
        assert_eq!(menu.handle(Input::Back, &garage), Action::Close);
    }
}

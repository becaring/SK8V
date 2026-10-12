//! Skate 3's skateboard catalog: the decks, trucks and wheels the game's
//! Create-a-Skater offers, read from the board library the Skate 3 converter
//! writes into the player's own converted Skate 3 data, and the board meshes
//! built from a chosen set of parts.
//!
//! Nothing here ships Skate 3 content. The library, its part meshes and its
//! textures exist only on the player's machine, converted from their disc.

pub mod convert;
pub mod glb;
pub mod grips;
pub mod paints;
pub mod preview;
pub mod stickers;
pub mod wear;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Where the board library lives inside the Skate 3 data folder.
pub const LIBRARY_DIR: &str = "private/boards";
const LIBRARY_FILE: &str = "library.json";

pub fn library_path(assets: &Path) -> PathBuf {
    assets.join(LIBRARY_DIR).join(LIBRARY_FILE)
}

/// The player's own decks (`skate-ped --import-decks`), beside Skate's:
/// `custom.json` and their textures, relative to the assets.
pub const CUSTOM_DIR: &str = "private/boards/custom";

pub fn custom_dir(assets: &Path) -> PathBuf {
    assets.join(CUSTOM_DIR)
}

/// `custom.json`: new finishes of one of Skate's models.
#[derive(Deserialize)]
struct RawCustom {
    model: String,
    materials: BTreeMap<String, RawMaterial>,
    #[serde(default)]
    decks: Vec<String>,
}

/// A part of the board, as Skate 3's catalog sorts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Part {
    Deck,
    Trucks,
    Wheels,
}

impl Part {
    pub const ALL: [Self; 3] = [Self::Deck, Self::Trucks, Self::Wheels];

    /// The catalog's component slot.
    pub fn slot(self) -> &'static str {
        match self {
            Self::Deck => "SkateBoard",
            Self::Trucks => "SkateTruck",
            Self::Wheels => "SkateWheel",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Deck => "Deck",
            Self::Trucks => "Trucks",
            Self::Wheels => "Wheels",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// The part a material or slot name belongs to.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|part| name.contains(part.slot()))
    }
}

/// One catalog entry: a part model in one of its finishes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct PartRef {
    pub asset: String,
    pub material: String,
}

impl PartRef {
    pub fn new(asset: &str, material: &str) -> Self {
        Self {
            asset: asset.to_owned(),
            material: material.to_owned(),
        }
    }

    fn to_text(&self) -> String {
        format!("{}:{}", self.asset, self.material)
    }

    fn from_text(text: &str) -> Option<Self> {
        let (asset, material) = text.split_once(':')?;
        // Skate's ids are hex; painted parts are `paint:paint_<colour>`.
        let id = |s: &str| !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        (id(asset) && id(material)).then(|| Self::new(&asset.to_ascii_lowercase(), &material.to_ascii_lowercase()))
    }
}

/// A board: a chosen deck, trucks and wheels, each part left out keeping the
/// default skater's own.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct BoardChoice {
    pub parts: [Option<PartRef>; 3],
}

impl BoardChoice {
    pub fn get(&self, part: Part) -> Option<&PartRef> {
        self.parts[part.index()].as_ref()
    }

    pub fn set(&mut self, part: Part, value: Option<PartRef>) {
        self.parts[part.index()] = value;
    }

    pub fn is_default(&self) -> bool {
        self.parts.iter().all(Option::is_none)
    }

    /// `deck,trucks,wheels`, each `asset:material` or empty for the default.
    pub fn to_text(&self) -> String {
        if self.is_default() {
            return String::new();
        }
        self.parts
            .iter()
            .map(|part| part.as_ref().map(PartRef::to_text).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Reads [`Self::to_text`]; anything unreadable is the default board.
    pub fn from_text(text: &str) -> Self {
        let mut choice = Self::default();
        let text = text.trim();
        if text.is_empty() {
            return choice;
        }
        let fields: Vec<&str> = text.split(',').collect();
        if fields.len() != 3 {
            return choice;
        }
        for (slot, field) in choice.parts.iter_mut().zip(fields) {
            *slot = PartRef::from_text(field.trim());
        }
        choice
    }
}

#[derive(Deserialize)]
struct RawLibrary {
    models: BTreeMap<String, RawModel>,
    materials: BTreeMap<String, RawMaterial>,
}

#[derive(Deserialize)]
struct RawModel {
    slot: String,
    name: String,
    #[serde(default)]
    materials: Vec<String>,
    scene: String,
}

#[derive(Deserialize)]
struct RawMaterial {
    name: String,
    diffuse: String,
    #[serde(default)]
    tint: Option<[f32; 3]>,
    /// The finish has an opacity map (merged into the diffuse's alpha).
    /// Without one, Skate draws the part solid and its alpha means nothing.
    #[serde(default)]
    alpha: bool,
    /// Its normal map (tangent space; `normal_map` reads Skate's layouts).
    #[serde(default)]
    normal: Option<String>,
    #[serde(default)]
    metallic: Option<f32>,
    #[serde(default)]
    roughness: Option<f32>,
    /// Skate 3's catalog flags: `ManufacturerID` (the brand), `Cost`, ...
    #[serde(default)]
    flags: HashMap<String, String>,
}

/// A brand's name as a shop shows it, from Skate 3's `ManufacturerID`
/// (lower case, spelt in a few ways: "zoo_york", "zooyork", "zoo york").
pub fn brand_name(id: &str) -> String {
    let key: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    let known = [
        ("alien", "Alien Workshop"),
        ("alienworkshop", "Alien Workshop"),
        ("antihero", "Anti Hero"),
        ("blacklabel", "Black Label"),
        ("dgk", "DGK"),
        ("hifi", "Hi-Fi"),
        ("mightyhealthy", "Mighty Healthy"),
        ("nobrand", "Unbranded"),
        ("planb", "Plan B"),
        ("santacruz", "Santa Cruz"),
        ("skate", "skate."),
        ("skatemental", "Skate Mental"),
        ("zooyork", "Zoo York"),
    ];
    if key.is_empty() {
        return "Unbranded".into();
    }
    if let Some((_, name)) = known.iter().find(|(k, _)| *k == key) {
        return (*name).into();
    }
    let mut name = key;
    name[..1].make_ascii_uppercase();
    name
}

/// An entry of the catalog as the player browses it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub part: Part,
    pub asset: String,
    pub material: String,
    /// The finish's name, as Skate 3's customiser shows it.
    pub label: String,
    /// A used ("Worn") finish rather than a new one. Shops and editors
    /// offer only new parts; a part turns worn by being ridden (see
    /// [`wear`]).
    pub worn: bool,
    /// The finish's own name, before any telling apart.
    pub finish: String,
    /// The maker, as Skate 3's catalog has it (see [`brand_name`]).
    pub brand: String,
}

impl Item {
    pub fn part_ref(&self) -> PartRef {
        PartRef::new(&self.asset, &self.material)
    }

    /// The label with "(Worn)" for used finishes.
    pub fn title(&self) -> String {
        if self.worn {
            format!("{} (Worn)", self.label)
        } else {
            self.label.clone()
        }
    }
}

/// The board library: every deck, truck and wheel on the player's disc.
pub struct Library {
    assets: PathBuf,
    items: [Vec<Item>; 3],
    scenes: HashMap<String, String>,
    textures: HashMap<String, (String, [f32; 3], bool)>,
    lightings: HashMap<String, Lighting>,
    stickers: Vec<stickers::Sticker>,
}

/// How a finish takes light, beside its colour: its normal map (relative
/// to the assets) and Skate's metallic and roughness (0 to 1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lighting {
    pub normal: Option<String>,
    pub metallic: f32,
    pub roughness: f32,
}

impl Library {
    pub fn exists(assets: &Path) -> bool {
        library_path(assets).is_file()
    }

    pub fn load(assets: &Path) -> Result<Self, String> {
        let path = library_path(assets);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Self::from_json(assets, &text)
    }

    pub fn from_json(assets: &Path, text: &str) -> Result<Self, String> {
        let mut raw: RawLibrary =
            serde_json::from_str(text).map_err(|error| format!("board library: {error}"))?;
        // The player's own decks, added to the model they are finishes of.
        if let Ok(text) = std::fs::read_to_string(custom_dir(assets).join("custom.json"))
            && let Ok(custom) = serde_json::from_str::<RawCustom>(&text)
            && raw.models.contains_key(&custom.model)
        {
            let order = if custom.decks.is_empty() { custom.materials.keys().cloned().collect() } else { custom.decks.clone() };
            for (id, material) in custom.materials {
                raw.materials.entry(id).or_insert(material);
            }
            if let Some(model) = raw.models.get_mut(&custom.model) {
                for id in order {
                    if !model.materials.contains(&id) {
                        model.materials.push(id);
                    }
                }
            }
        }
        let mut items: [Vec<Item>; 3] = Default::default();
        let mut scenes = HashMap::new();
        let mut names = HashMap::new();
        for part in Part::ALL {
            let mut models: Vec<_> = raw.models.iter().filter(|(_, m)| m.slot == part.slot()).collect();
            models.sort_by(|a, b| a.1.name.cmp(&b.1.name).then(a.0.cmp(b.0)));
            for (id, model) in models {
                scenes.insert(id.clone(), model.scene.clone());
                names.insert(id.clone(), model.name.clone());
                for mid in &model.materials {
                    let Some(material) = raw.materials.get(mid) else {
                        continue;
                    };
                    let worn = material.name.ends_with(" (Worn)");
                    let label = material.name.trim_end_matches(" (Worn)").trim();
                    let label = if label.is_empty() { "Original".to_owned() } else { label.to_owned() };
                    items[part.index()].push(Item {
                        part,
                        asset: id.clone(),
                        material: mid.clone(),
                        finish: label.clone(),
                        label,
                        worn,
                        brand: brand_name(material.flags.get("ManufacturerID").map_or("", String::as_str)),
                    });
                }
            }
        }
        for list in &mut items {
            // Finishes of different models can share a name; tell them apart
            // by the model, then by the finish itself.
            for pass in 0..2 {
                let mut seen = HashMap::<(String, bool), usize>::new();
                for item in list.iter() {
                    *seen.entry((item.label.clone(), item.worn)).or_default() += 1;
                }
                for item in list.iter_mut() {
                    if seen[&(item.label.clone(), item.worn)] > 1 {
                        let extra = if pass == 0 {
                            names.get(&item.asset).cloned().unwrap_or_default()
                        } else {
                            item.material.chars().take(6).collect()
                        };
                        if !extra.is_empty() {
                            item.label = format!("{} ({extra})", item.label);
                        }
                    }
                }
            }
            list.sort_by(|a, b| {
                a.title()
                    .to_lowercase()
                    .cmp(&b.title().to_lowercase())
                    .then(a.asset.cmp(&b.asset))
            });
        }
        let lightings = raw
            .materials
            .iter()
            .map(|(id, m)| (id.clone(), Lighting { normal: m.normal.clone(), metallic: m.metallic.unwrap_or(0.), roughness: m.roughness.unwrap_or(0.72) }))
            .collect();
        let textures = raw
            .materials
            .into_iter()
            .map(|(id, m)| (id, (m.diffuse, m.tint.unwrap_or([1.0; 3]), m.alpha)))
            .collect();
        Ok(Self {
            assets: assets.to_path_buf(),
            items,
            scenes,
            textures,
            lightings,
            stickers: std::fs::read_to_string(assets.join(LIBRARY_DIR).join("stickers.json")).map(|t| stickers::parse(&t)).unwrap_or_default(),
        })
    }

    /// The stickers made for the store (empty until `skate-ped` made them).
    pub fn stickers(&self) -> &[stickers::Sticker] {
        &self.stickers
    }

    pub fn items(&self, part: Part) -> &[Item] {
        &self.items[part.index()]
    }

    pub fn find(&self, part: Part, wanted: &PartRef) -> Option<&Item> {
        self.items(part)
            .iter()
            .find(|item| item.asset == wanted.asset && item.material == wanted.material)
    }

    /// The new parts a shop or editor offers: no worn finishes.
    pub fn new_items(&self, part: Part) -> Vec<&Item> {
        self.items(part).iter().filter(|item| !item.worn).collect()
    }

    /// The worn finish of a new part, when the game has one.
    pub fn worn_of(&self, part: Part, wanted: &PartRef) -> Option<PartRef> {
        let item = self.find(part, wanted).filter(|item| !item.worn)?;
        self.items(part)
            .iter()
            .find(|other| other.worn && other.asset == item.asset && other.finish == item.finish)
            .map(Item::part_ref)
    }

    /// The new part a worn finish wears out from (or the part itself).
    pub fn fresh_of(&self, part: Part, wanted: &PartRef) -> PartRef {
        self.find(part, wanted)
            .filter(|item| item.worn)
            .and_then(|item| {
                self.items(part)
                    .iter()
                    .find(|other| !other.worn && other.asset == item.asset && other.finish == item.finish)
            })
            .map_or_else(|| wanted.clone(), Item::part_ref)
    }

    /// `choice` with every part taken back to new (worn picks saved before
    /// parts wore out on their own).
    pub fn fresh(&self, choice: &BoardChoice) -> BoardChoice {
        let mut out = choice.clone();
        for part in Part::ALL {
            if let Some(wanted) = choice.get(part) {
                out.set(part, Some(self.fresh_of(part, wanted)));
            }
        }
        out
    }

    /// The image file of a finish (its diffuse texture).
    pub fn texture_path(&self, material: &str) -> Option<PathBuf> {
        let (path, ..) = self.textures.get(material)?;
        Some(self.assets.join(path))
    }

    /// How a finish takes light (`Surface`); none for an unknown finish.
    pub fn lighting(&self, material: &str) -> Option<&Lighting> {
        self.lightings.get(material)
    }

    /// A finish's normal map in GTA's (and Direct3D's) layout: x in red,
    /// y in green, z in blue. Skate keeps them three ways (see
    /// [`normal_from_skate`]); the library's own `_normal.png` lost x for the
    /// trucks, so the decoded original beside it is read when there is one.
    pub fn normal_map(&self, material: &str, limit: u32) -> Option<Texture> {
        let stored = self.lightings.get(material)?.normal.as_ref()?;
        let file = Path::new(stored).file_name()?.to_str()?;
        let id = file.strip_suffix("_normal.png").unwrap_or(file.trim_end_matches(".png"));
        let decoded = self.assets.join(LIBRARY_DIR).join("decoded").join(format!("{id}.png"));
        let path = if decoded.is_file() { decoded } else { self.assets.join(stored) };
        let texture = decode_png(&std::fs::read(path).ok()?).ok()?;
        Some(normal_from_skate(texture).shrink(limit))
    }

    /// A finish's texture, tinted as Skate 3 tints it, its alpha as Skate
    /// draws it: solid without an opacity map, cut at half with one.
    pub fn texture(&self, material: &str, limit: u32) -> Result<Texture, String> {
        let (path, tint, alpha) = self.textures.get(material).ok_or("unknown board finish")?;
        let path = self.assets.join(path);
        let bytes = std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let mut texture = decode_png(&bytes)?;
        if tint.iter().any(|&t| (t - 1.0).abs() > 1e-3) {
            for pixel in texture.rgba.chunks_exact_mut(4) {
                for c in 0..3 {
                    pixel[c] = (f32::from(pixel[c]) * tint[c].clamp(0.0, 1.0)).round() as u8;
                }
            }
        }
        let mut texture = texture.shrink(limit);
        if *alpha {
            texture.cut_at_half();
        } else {
            texture.make_opaque();
        }
        Ok(texture)
    }

    /// A finish's texture as a part shows it: solid. Where an opacity map
    /// cuts the finish out, Skate's part shows its own material under it
    /// (a deck's bare maple, a truck's or wheel's own colour), never what
    /// is behind the board.
    pub fn solid_texture(&self, part: Part, material: &str, limit: u32) -> Result<Texture, String> {
        let mut texture = self.texture(material, limit)?;
        if texture.rgba.chunks_exact(4).any(|p| p[3] < 255) {
            match part {
                Part::Deck => texture.print_over(&Texture { width: 1, height: 1, rgba: vec![214, 178, 132, 255] }),
                Part::Trucks | Part::Wheels => texture.fill_under(),
            }
        }
        Ok(texture)
    }

    /// The mesh of one catalog part in one finish.
    pub fn part_mesh(&self, part: Part, wanted: &PartRef, limit: u32) -> Result<Board, String> {
        self.find(part, wanted)
            .ok_or_else(|| format!("the {} is not in the board library", part.label().to_lowercase()))?;
        let scene = self.scenes.get(&wanted.asset).ok_or("unknown board part")?;
        let path = self.assets.join(scene);
        let bytes = std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let texture = self.solid_texture(part, &wanted.material, limit)?;
        let material = format!("iw4l_skate/board/{}", wanted.material);
        read_board(&bytes, |_| Some((part, material.clone())), |_, _| Ok(texture.clone()))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Texture {
    /// Every pixel opaque, its colour kept.
    pub fn make_opaque(&mut self) {
        for pixel in self.rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }

    /// Alpha cut at a half, as Skate's alpha-masked parts are drawn: each
    /// pixel is there or not, never see-through.
    pub fn cut_at_half(&mut self) {
        for pixel in self.rgba.chunks_exact_mut(4) {
            pixel[3] = if pixel[3] >= 128 { 255 } else { 0 };
        }
    }

    /// Fills what is cut out with the image's own average colour.
    pub fn fill_under(&mut self) {
        let (mut sum, mut n) = ([0f64; 3], 0f64);
        for p in self.rgba.chunks_exact(4).filter(|p| p[3] >= 128) {
            for c in 0..3 {
                sum[c] += f64::from(p[c]);
            }
            n += 1.;
        }
        let fill = if n > 0. { sum.map(|s| (s / n).round() as u8) } else { [150; 3] };
        self.print_over(&Texture { width: 1, height: 1, rgba: vec![fill[0], fill[1], fill[2], 255] });
    }

    /// Lays this image over `base` (stretched to this size) where it is
    /// see-through, leaving it opaque.
    pub fn print_over(&mut self, base: &Texture) {
        let (w, h) = (self.width as usize, self.height as usize);
        if w == 0 || h == 0 || base.width == 0 || base.height == 0 || self.rgba.len() < w * h * 4 || base.rgba.len() < (base.width * base.height * 4) as usize {
            return;
        }
        let (bw, bh) = (base.width as usize, base.height as usize);
        for y in 0..h {
            let by = (y * bh / h).min(bh - 1);
            for x in 0..w {
                let bx = (x * bw / w).min(bw - 1);
                let at = (y * w + x) * 4;
                let from = (by * bw + bx) * 4;
                let a = f32::from(self.rgba[at + 3]) / 255.;
                for c in 0..3 {
                    let top = f32::from(self.rgba[at + c]);
                    let under = f32::from(base.rgba[from + c]);
                    self.rgba[at + c] = (top * a + under * (1. - a)).round().clamp(0., 255.) as u8;
                }
                self.rgba[at + 3] = 255;
            }
        }
    }

    /// Box-filters the image to fit within `limit` on both sides, keeping its
    /// aspect ratio. Smaller images pass through.
    pub fn shrink(self, limit: u32) -> Self {
        let (width, height) = (self.width, self.height);
        if width <= limit && height <= limit {
            return self;
        }
        let scale = (f64::from(limit) / f64::from(width)).min(f64::from(limit) / f64::from(height));
        let out_w = ((f64::from(width) * scale).round() as u32).max(1);
        let out_h = ((f64::from(height) * scale).round() as u32).max(1);
        let mut out = Vec::with_capacity((out_w * out_h * 4) as usize);
        for y in 0..out_h {
            let y0 = y * height / out_h;
            let y1 = ((y + 1) * height / out_h).max(y0 + 1);
            for x in 0..out_w {
                let x0 = x * width / out_w;
                let x1 = ((x + 1) * width / out_w).max(x0 + 1);
                let mut sum = [0u32; 4];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let at = ((sy * width + sx) * 4) as usize;
                        for (c, total) in sum.iter_mut().enumerate() {
                            *total += u32::from(self.rgba[at + c]);
                        }
                    }
                }
                let n = (y1 - y0) * (x1 - x0);
                out.extend(sum.map(|s| ((s + n / 2) / n) as u8));
            }
        }
        Self {
            width: out_w,
            height: out_h,
            rgba: out,
        }
    }

    /// The colour at texture coordinate `uv`, wrapping like a repeat sampler.
    pub fn sample(&self, uv: [f32; 2]) -> [u8; 4] {
        if self.width == 0 || self.height == 0 {
            return [255; 4];
        }
        let wrap = |v: f32, size: u32| -> u32 {
            let f = v - v.floor();
            ((f * size as f32) as u32).min(size - 1)
        };
        let x = wrap(uv[0], self.width);
        let y = wrap(uv[1], self.height);
        let at = ((y * self.width + x) * 4) as usize;
        [self.rgba[at], self.rgba[at + 1], self.rgba[at + 2], self.rgba[at + 3]]
    }
}

/// A Skate 3 normal map as decoded from the disc, in GTA's layout. Skate
/// has three: x and y in red and green with z in blue (most trucks), the
/// same with blue left blank (z rebuilt), and grey ones with x in alpha and
/// y in green (the deck's grain, the wheels: DXT5 normal maps).
pub fn normal_from_skate(mut texture: Texture) -> Texture {
    let pixels = (texture.rgba.len() / 4).max(1);
    let grey = texture.rgba.chunks_exact(4).filter(|p| p[0].abs_diff(p[2]) <= 2 && p[1].abs_diff(p[2]) <= 2).count() * 10 >= pixels * 9
        && texture.rgba.chunks_exact(4).any(|p| p[3] != 255);
    for p in texture.rgba.chunks_exact_mut(4) {
        let x = f32::from(if grey { p[3] } else { p[0] }) / 127.5 - 1.;
        let y = f32::from(p[1]) / 127.5 - 1.;
        let z = (1. - x * x - y * y).max(0.).sqrt();
        let byte = |v: f32| ((v.clamp(-1., 1.) + 1.) * 127.5).round() as u8;
        p.copy_from_slice(&[byte(x), byte(y), byte(z), 255]);
    }
    texture
}

pub fn decode_png(bytes: &[u8]) -> Result<Texture, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| format!("board texture: {error}"))?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or("board texture too large")?];
    let frame = reader.next_frame(&mut pixels).map_err(|error| format!("board texture: {error}"))?;
    let channels = frame.color_type.samples();
    let rgba = pixels[..frame.buffer_size()]
        .chunks_exact(channels)
        .flat_map(|p| match channels {
            1 => [p[0], p[0], p[0], 255],
            2 => [p[0], p[0], p[0], p[1]],
            3 => [p[0], p[1], p[2], 255],
            _ => [p[0], p[1], p[2], p[3]],
        })
        .collect();
    Ok(Texture {
        width: frame.width,
        height: frame.height,
        rgba,
    })
}

/// A skeleton bone a board's vertices follow, with the inverse of its bind
/// transform in the mesh's own (Skate 3 converter) space.
#[derive(Clone, Debug, PartialEq)]
pub struct Joint {
    pub name: String,
    pub inverse_bind: [f32; 16],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u32; 4],
    pub weights: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub part: Part,
    pub material: String,
    pub texture: usize,
    pub vertices: Vec<Vertex>,
    /// Triangles, as the converter wound them.
    pub indices: Vec<u32>,
}

/// A skinned board mesh with its textures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    pub joints: Vec<Joint>,
    pub surfaces: Vec<Surface>,
    pub textures: Vec<Texture>,
}

impl Board {
    /// The default skater's board, from the converted skater model.
    pub fn default_board(assets: &Path, limit: u32) -> Result<Self, String> {
        let path = assets.join("private").join("skater.glb");
        let bytes = std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        read_board(
            &bytes,
            |name| Part::from_name(name).map(|part| (part, format!("iw4l_skate/{name}"))),
            |glb, material| {
                let texture = glb::index(
                    &glb.json["materials"][material]["pbrMetallicRoughness"]["baseColorTexture"]["index"],
                )?;
                let image = glb::index(&glb.json["textures"][texture]["source"])?;
                Ok(decode_png(glb.image_bytes(image)?)?.shrink(limit))
            },
        )
    }

    /// The default skater's body: everything in the converted skater model
    /// but the board, for showing the skater holding a board.
    pub fn default_skater(assets: &Path, limit: u32) -> Result<Self, String> {
        let path = assets.join("private").join("skater.glb");
        let bytes = std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        read_board(
            &bytes,
            |name| Part::from_name(name).is_none().then(|| (Part::Deck, format!("iw4l_skate/{name}"))),
            |glb, material| {
                let Ok(texture) = glb::index(
                    &glb.json["materials"][material]["pbrMetallicRoughness"]["baseColorTexture"]["index"],
                ) else {
                    // An untextured part: plain grey.
                    return Ok(Texture { width: 1, height: 1, rgba: vec![150, 150, 150, 255] });
                };
                let image = glb::index(&glb.json["textures"][texture]["source"])?;
                let mut texture = decode_png(glb.image_bytes(image)?)?.shrink(limit);
                // Solid unless the material is cut out (hair, laces): the
                // alpha of most of the skater's textures holds a mask, and
                // as see-through the skater looked half transparent.
                match glb.json["materials"][material]["alphaMode"].as_str() {
                    Some("MASK" | "BLEND") => texture.cut_at_half(),
                    _ => texture.make_opaque(),
                }
                Ok(texture)
            },
        )
    }

    /// Only the surfaces of `part`, with the joints and textures they use.
    pub fn only(&self, part: Part) -> Self {
        let mut out = Self::default();
        let mut joints = HashMap::new();
        let mut textures = HashMap::new();
        for surface in self.surfaces.iter().filter(|s| s.part == part) {
            let mut surface = surface.clone();
            surface.texture = *textures.entry(surface.texture).or_insert_with(|| {
                out.textures.push(self.textures[surface.texture].clone());
                out.textures.len() - 1
            });
            for vertex in &mut surface.vertices {
                for joint in &mut vertex.joints {
                    *joint = *joints.entry(*joint).or_insert_with(|| {
                        out.joints.push(self.joints[*joint as usize].clone());
                        (out.joints.len() - 1) as u32
                    });
                }
            }
            out.surfaces.push(surface);
        }
        out
    }

    pub fn append(&mut self, other: Self) {
        let joints = self.joints.len() as u32;
        let textures = self.textures.len();
        self.joints.extend(other.joints);
        self.textures.extend(other.textures);
        for mut surface in other.surfaces {
            surface.texture += textures;
            for vertex in &mut surface.vertices {
                for joint in &mut vertex.joints {
                    *joint += joints;
                }
            }
            self.surfaces.push(surface);
        }
    }

    pub fn has(&self, part: Part) -> bool {
        self.surfaces.iter().any(|s| s.part == part)
    }
}

/// Where a board surface's texture is in GTA V: the texture dictionary
/// the skater pack carries for its part, and the texture's name there.
/// Library finishes keep their id; the default skater's parts are named
/// after their material.
pub fn gta_texture(part: Part, material: &str) -> (String, String) {
    let dict = match part {
        Part::Deck => "skate3_board_deck",
        Part::Trucks => "skate3_board_trucks",
        Part::Wheels => "skate3_board_wheels",
    };
    match material.strip_prefix("iw4l_skate/board/") {
        Some(id) => (dict.to_owned(), id.to_ascii_lowercase()),
        None => {
            let name: String = material
                .trim_start_matches("iw4l_skate/")
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
                .collect();
            ("skate3_board_default".to_owned(), format!("default_{name}"))
        }
    }
}

/// The board for `choice`: chosen parts from the library, the rest from
/// `defaults` (the default skater's board).
pub fn assemble(library: Option<&Library>, defaults: &Board, choice: &BoardChoice, limit: u32) -> Result<Board, String> {
    let mut board = Board::default();
    for part in Part::ALL {
        match (library, choice.get(part)) {
            (Some(library), Some(wanted)) => board.append(library.part_mesh(part, wanted, limit)?),
            (None, Some(_)) => return Err("the board library has not been prepared".into()),
            _ => board.append(defaults.only(part)),
        }
    }
    if board.surfaces.is_empty() {
        return Err("the board has no surfaces".into());
    }
    // Parts from the library come as Skate draws them (see
    // `Library::texture`); the default skater's board is solid.
    let chosen: std::collections::HashSet<usize> = board
        .surfaces
        .iter()
        .filter(|s| choice.get(s.part).is_some() && library.is_some())
        .map(|s| s.texture)
        .collect();
    for (index, texture) in board.textures.iter_mut().enumerate() {
        if !chosen.contains(&index) {
            texture.make_opaque();
        }
    }
    Ok(board)
}

/// Every joint of the converted skater model's skin (`private/skater.glb`),
/// with its inverse bind matrix (Skate space, column-major).
pub fn skater_joints(assets: &Path) -> Result<Vec<Joint>, String> {
    let path = assets.join("private").join("skater.glb");
    let bytes = std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let glb = glb::Glb::parse(&bytes)?;
    let skin = &glb.json["skins"][0];
    let skin_joints = skin["joints"].as_array().ok_or("the skater model has no skin")?;
    let inverse = glb.floats(glb::index(&skin["inverseBindMatrices"])?)?;
    if inverse.len() != skin_joints.len() * 16 {
        return Err("the skater model's bind matrices do not match its joints".into());
    }
    skin_joints
        .iter()
        .enumerate()
        .map(|(i, joint)| {
            let node = glb::index(joint)?;
            let name = glb.json["nodes"][node]["name"].as_str().ok_or_else(|| format!("joint node {node} has no name"))?;
            let mut inverse_bind = [0f32; 16];
            inverse_bind.copy_from_slice(&inverse[i * 16..i * 16 + 16]);
            Ok(Joint { name: name.to_owned(), inverse_bind })
        })
        .collect()
}

/// Reads the skinned primitives `pick` names (by material name) from a GLB:
/// their part and the material name to give them.
fn read_board(
    bytes: &[u8],
    pick: impl Fn(&str) -> Option<(Part, String)>,
    texture: impl Fn(&glb::Glb, usize) -> Result<Texture, String>,
) -> Result<Board, String> {
    let glb = glb::Glb::parse(bytes)?;
    let skin = &glb.json["skins"][0];
    let skin_joints = skin["joints"].as_array().ok_or("the board model has no skin")?;
    let inverse = glb.floats(glb::index(&skin["inverseBindMatrices"])?)?;
    if inverse.len() != skin_joints.len() * 16 {
        return Err("the board model's bind matrices do not match its joints".into());
    }
    let joint_name = |i: usize| -> Result<String, String> {
        let node = glb::index(&skin_joints[i])?;
        glb.json["nodes"][node]["name"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("joint node {node} has no name"))
    };
    let mut board = Board::default();
    let mut used = HashMap::<u32, u32>::new();
    let mut textures = HashMap::<usize, usize>::new();
    let meshes = glb.json["meshes"].as_array().cloned().unwrap_or_default();
    for mesh in &meshes {
        for primitive in mesh["primitives"].as_array().into_iter().flatten() {
            let material_index = glb::index(&primitive["material"])?;
            let material_name = glb.json["materials"][material_index]["name"].as_str().unwrap_or("");
            let Some((part, material)) = pick(material_name) else {
                continue;
            };
            let attributes = &primitive["attributes"];
            let positions = glb.floats(glb::index(&attributes["POSITION"])?)?;
            let count = positions.len() / 3;
            let normals = match attributes.get("NORMAL") {
                Some(accessor) => glb.floats(glb::index(accessor)?)?,
                None => vec![0.0; count * 3],
            };
            let uvs = glb.floats(glb::index(&attributes["TEXCOORD_0"])?)?;
            let bones = glb.integers(glb::index(&attributes["JOINTS_0"])?)?;
            let weights = glb.floats(glb::index(&attributes["WEIGHTS_0"])?)?;
            if normals.len() != count * 3 || uvs.len() != count * 2 || bones.len() != count * 4 || weights.len() != count * 4 {
                return Err(format!("{material_name}: vertex streams disagree"));
            }
            let indices = glb.integers(glb::index(&primitive["indices"])?)?;
            if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= count) {
                return Err(format!("{material_name}: broken triangles"));
            }
            let mut vertices = Vec::with_capacity(count);
            for v in 0..count {
                let mut joints = [0u32; 4];
                let mut w = [0f32; 4];
                for k in 0..4 {
                    w[k] = weights[v * 4 + k];
                    let bone = bones[v * 4 + k];
                    if w[k] == 0.0 {
                        continue;
                    }
                    if bone as usize >= skin_joints.len() {
                        return Err(format!("{material_name}: a vertex follows a missing joint"));
                    }
                    joints[k] = match used.get(&bone) {
                        Some(&j) => j,
                        None => {
                            let at = bone as usize * 16;
                            board.joints.push(Joint {
                                name: joint_name(bone as usize)?,
                                inverse_bind: inverse[at..at + 16].try_into().unwrap(),
                            });
                            let j = (board.joints.len() - 1) as u32;
                            used.insert(bone, j);
                            j
                        }
                    };
                }
                let total: f32 = w.iter().sum();
                if total <= 0.0 || !total.is_finite() {
                    return Err(format!("{material_name}: a vertex has no weights"));
                }
                vertices.push(Vertex {
                    position: [positions[v * 3], positions[v * 3 + 1], positions[v * 3 + 2]],
                    normal: [normals[v * 3], normals[v * 3 + 1], normals[v * 3 + 2]],
                    uv: [uvs[v * 2], uvs[v * 2 + 1]],
                    joints,
                    weights: w.map(|x| x / total),
                });
            }
            let texture = match textures.get(&material_index) {
                Some(&t) => t,
                None => {
                    board.textures.push(texture(&glb, material_index)?);
                    let t = board.textures.len() - 1;
                    textures.insert(material_index, t);
                    t
                }
            };
            board.surfaces.push(Surface {
                part,
                material,
                texture,
                vertices,
                indices,
            });
        }
    }
    if board.surfaces.is_empty() {
        return Err("the model has no board surfaces".into());
    }
    if board.joints.is_empty() {
        // Every weight was zero-indexed to one joint; still give it a bone.
        board.joints.push(Joint {
            name: joint_name(0)?,
            inverse_bind: inverse[..16].try_into().unwrap(),
        });
    }
    Ok(board)
}

/// A board that suits a class: its look picked by the kind of weapon the
/// class carries (a sniper rides dark, a riot shield steel, a sub-machine
/// gun something bright), and where the catalog has no such finish, one
/// picked from `seed` (the class) so each class still rides its own.
pub fn fitting_board(items: impl Fn(Part) -> Vec<Item>, primary: &str, seed: &str) -> BoardChoice {
    let theme = weapon_theme(primary);
    let mut board = BoardChoice::default();
    for part in Part::ALL {
        let mut offered: Vec<Item> = items(part).into_iter().filter(|item| !item.worn).collect();
        if offered.is_empty() {
            continue;
        }
        offered.sort_by(|a, b| (&a.asset, &a.material).cmp(&(&b.asset, &b.material)));
        let fits: Vec<&Item> = offered
            .iter()
            .filter(|item| {
                let label = item.label.to_lowercase();
                theme.iter().any(|word| label.contains(word))
            })
            .collect();
        let pool: Vec<&Item> = if fits.is_empty() { offered.iter().collect() } else { fits };
        let pick = pool[(stable_hash(&format!("{seed}/{}", part.label())) % pool.len() as u64) as usize];
        board.set(part, Some(pick.part_ref()));
    }
    board
}

/// Words in finish names that suit a weapon, by MW2 weapon name.
fn weapon_theme(primary: &str) -> &'static [&'static str] {
    let w = primary.to_lowercase();
    let any = |names: &[&str]| names.iter().any(|n| w.contains(n));
    if any(&["cheytac", "barrett", "wa2000", "m21", "m40", "dragunov", "sniper"]) {
        &["black", "dark", "camo", "night", "shadow", "grey", "gray", "smoke"]
    } else if any(&["riotshield", "shield"]) {
        &["silver", "chrome", "steel", "white", "metal"]
    } else if any(&["rpd", "sa80", "mg4", "m240", "aug", "l86", "lmg"]) {
        &["red", "skull", "fire", "flame", "heavy", "black"]
    } else if any(&["spas", "aa12", "striker", "ranger", "model1887", "m1014", "shotgun"]) {
        &["skull", "red", "rust", "wood", "brown", "orange"]
    } else if any(&["mp5k", "ump45", "kriss", "p90", "uzi", "tmp", "smg"]) {
        &["green", "neon", "yellow", "lime", "pink", "bright"]
    } else {
        &["blue", "army", "olive", "classic", "stripe", "navy"]
    }
}

/// FNV-1a: the same pick on every machine and every run.
fn stable_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests;

//! Project-owned player for Skate 3's Hall of Meat HUD (the APT `homscoring`
//! movie), on the donor's VM, movie timeline and scene traversal. The movie's
//! own ActionScript does all layout and animation; this module supplies the
//! natives it calls (`HUDComponents.HOM_*` from the WipeoutScorer's HUD data,
//! `FELanguage` formatting) and drives `SetState`.
use crate::apt_movie::Movie;
use crate::apt_vm::{Host, ObjectKind, Value, Vm};
use crate::layout::Layout;
use crate::movie::{EdgeScreen, MoviePlayer};
use crate::player::{Assets, Frame};

/// `homscoring.SetState` arguments (movie script).
pub mod state {
    pub const SHOW: i32 = 1;
    pub const HIDE: i32 = 2;
    pub const UPDATE_SCORING: i32 = 3;
    pub const CHALLENGE: i32 = 999;
    pub const NORMAL: i32 = 1000;
}

/// Slots per bonus kind in the collision score arrays.
pub const HIT_SLOTS: usize = 10;

/// The WipeoutScorer's HUD publication (the packed data the `HOM_*` natives
/// return). Speeds in m/s, distances in m, times in s, angles in degrees.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomData {
    pub score: f32,
    pub speed_points: f32,
    pub drop_points: f32,
    pub air_points: f32,
    pub ragdoll_points: f32,
    pub spin_points: f32,
    pub speed: f32,
    pub max_drop: f32,
    pub max_air_seconds: f32,
    pub ragdoll_seconds: f32,
    pub max_spin_degrees: f32,
    /// Force a metric panel regardless of its points threshold: speed, drop,
    /// air, ragdoll time, rotation.
    pub metric_flags: [bool; 5],
    pub car_hits: u32,
    pub dmo_hits: u32,
    pub ped_hits: u32,
    pub skater_hits: u32,
    pub tweak_scored: bool,
    pub tweak_points: f32,
    pub car_scores: [f32; HIT_SLOTS],
    pub dmo_scores: [f32; HIT_SLOTS],
    pub ped_scores: [f32; HIT_SLOTS],
    pub skater_scores: [f32; HIT_SLOTS],
}

/// Left and bottom edges of the movie's score box at rest, movie units (with
/// no edge offset; measured from the rendered movie). The bonus and metric
/// panels stack upwards from it.
pub const SCORE_BOX_LEFT: f32 = 100.7;
pub const SCORE_BOX_BOTTOM: f32 = 670.0;
/// Top of the highest panel when all eight bonus/metric panels show.
pub const STACK_TOP: f32 = 255.3;
/// Free space between the top of GTA's minimap and the score box, in
/// minimap heights: a small gap above the radar.
pub const FEED_ROOM: f32 = 0.25;

/// GTA's minimap in output pixels (x0, y0, x1, y1): anchored to the
/// bottom-left corner of the safe area, a quarter of the screen height wide
/// and 1/5.674 of it tall (the community-documented GTA V minimap anchor).
pub fn minimap(layout: &Layout) -> [f32; 4] {
    let h = layout.height as f32;
    let [x0, _, _, y1] = layout.safe;
    [x0, y1 - h / 5.674, x0 + h / 4.0, y1]
}

/// Placement of the Hall of Meat movie: the score box's left edge on
/// the minimap's left edge, its bottom `FEED_ROOM` minimap heights above
/// the minimap, so neither the radar nor GTA's notifications are covered;
/// scaled down about that corner only as far as the full panel stack needs
/// to stay inside the safe area. The movie's own edge offset is not applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Native pixel position of the score box's bottom-left corner.
    pub from: [f32; 2],
    /// Where it goes.
    pub to: [f32; 2],
    pub scale: f32,
}

impl Placement {
    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.to[0] + (p[0] - self.from[0]) * self.scale,
            self.to[1] + (p[1] - self.from[1]) * self.scale,
        ]
    }
}

pub fn placement(layout: &Layout) -> Placement {
    let map = minimap(layout);
    let from = layout.to_pixels([SCORE_BOX_LEFT, SCORE_BOX_BOTTOM]);
    let to = [map[0], map[1] - FEED_ROOM * (map[3] - map[1])];
    let stack = (SCORE_BOX_BOTTOM - STACK_TOP) * layout.scale;
    let room = to[1] - layout.safe[1];
    let scale = if stack > 0.0 && room > 0.0 { (room / stack).min(1.0) } else { 1.0 };
    Placement { from, to, scale }
}

pub(crate) struct Bindings {
    pub(crate) movie: Movie,
    pub(crate) data: HomData,
}

fn array(vm: &mut Vm, values: impl IntoIterator<Item = Value>) -> Result<Value, String> {
    let object = vm.object(ObjectKind::Plain);
    let mut n = 0;
    for value in values {
        vm.set(object, n.to_string(), value)?;
        n += 1;
    }
    vm.set(object, "length", Value::Number(n as f64))?;
    Ok(Value::Object(object))
}

fn number(v: f32) -> Value {
    Value::Number(if v.is_finite() { v as f64 } else { 0.0 })
}

/// Whole number with thousands separators (FELanguage
/// GetLocalizedFormatNumberString, English).
pub fn format_number(value: f64) -> String {
    let value = if value.is_finite() { value.round() as i64 } else { 0 };
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    if value < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Expands `{0}`, `{0:f.N}` and the unit placeholders `{0:speed}` (km/h) and
/// `{0:distance}` (metres, one decimal) of an English language string.
pub fn format_custom(template: &str, value: &str, language: &std::collections::BTreeMap<String, String>) -> String {
    let n: f64 = value.trim().parse().unwrap_or(f64::NAN);
    let n = if n.is_finite() { n } else { 0.0 };
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{0") {
        out.push_str(&rest[..start]);
        let Some(len) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let spec = &rest[start + 2..start + len];
        let spec = spec.strip_prefix(':').unwrap_or(spec);
        out.push_str(&match spec {
            "" => value.to_string(),
            "speed" => {
                let unit = language.get("ID_COMMON_KMPH").map_or("km/h", String::as_str);
                format!("{} {unit}", format_number(n * 3.6))
            }
            "distance" => match language.get("ID_COMMON_METRIC_METER_FLOAT") {
                Some(t) if !t.contains("{0:distance}") => format_custom(t, value, language),
                _ => format!("{n:.1} m"),
            },
            s => match s.strip_prefix("f.").and_then(|d| d.parse::<usize>().ok()) {
                Some(d) => format!("{n:.d$}"),
                None => value.to_string(),
            },
        });
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

impl Host for Bindings {
    fn property_changed(&mut self, vm: &mut Vm, object: usize, key: &str) -> Result<(), String> {
        if key == "text" || key == "autoSize" {
            self.movie.text_changed(vm, object)?;
        }
        Ok(())
    }
    fn call(&mut self, vm: &mut Vm, object: usize, method: &str, args: Vec<Value>) -> Result<Value, String> {
        // A missing frame label is a no-op in Flash (chyron's
        // gotoAndPlay("hidden") on a root that has no such label).
        match self.movie.method(vm, object, method, &args) {
            Ok(true) => return Ok(Value::Undefined),
            Ok(false) => {}
            Err(e) if e.contains("lacks label") => return Ok(Value::Undefined),
            Err(e) => return Err(e),
        }
        let native = match vm.objects.get(object).map(|o| &o.kind) {
            Some(ObjectKind::Native(name)) => name.as_str(),
            _ => "global",
        };
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        let d = &self.data;
        match (native, method) {
            ("Math", "floor") => Ok(Value::Number(arg(0).number().floor())),
            // Enumeration flags only; the movie never enumerates prototypes.
            ("global", "ASSetPropFlags") => Ok(Value::Undefined),
            // Frontend sounds and screen registration have no GTA counterpart.
            ("Audio", "PlaySound" | "ChyronIntroDone" | "ChyronOutroDone") | ("ScreenManager", "OnLoaded") => {
                Ok(Value::Undefined)
            }
            ("FELanguage", "GetLanguage") => Ok(Value::Text("english".into())),
            ("FELanguage", "GetLocalizedFormatNumberString") => Ok(Value::Text(format_number(arg(0).number()))),
            ("FELanguage", "GetLocaleText") => {
                let id = arg(0).text();
                Ok(Value::Text(self.movie.text_assets.language.get(&id).cloned().unwrap_or(id)))
            }
            ("FELanguage", "GetLocalizedTime") => {
                let s = arg(0).number();
                Ok(Value::Text(format!("{:.1}", if s.is_finite() { s } else { 0.0 })))
            }
            ("FELanguage", "GetLocalizedFormatStringCustom") => {
                let language = &self.movie.text_assets.language;
                let id = arg(0).text();
                let template = language.get(&id).cloned().unwrap_or(id);
                Ok(Value::Text(format_custom(&template, &arg(1).text(), language)))
            }
            ("HUDComponents", "HOM_GetScore") => Ok(number(d.score)),
            ("HUDComponents", "HOM_GetBonusScores") => {
                let v = [d.speed_points, d.drop_points, d.air_points, d.ragdoll_points, d.spin_points];
                array(vm, v.map(number))
            }
            ("HUDComponents", "HOM_GetBonusData") => {
                let mut v: Vec<Value> = [d.speed, d.max_drop, d.max_air_seconds, d.ragdoll_seconds, d.max_spin_degrees]
                    .map(number)
                    .into();
                v.extend(d.metric_flags.map(Value::Bool));
                array(vm, v)
            }
            ("HUDComponents", "HOM_GetBonusCollisionData") => array(
                vm,
                [
                    Value::Number(d.car_hits as f64),
                    Value::Number(d.dmo_hits as f64),
                    Value::Bool(d.tweak_scored),
                    Value::Number(d.ped_hits as f64),
                    Value::Number(d.skater_hits as f64),
                ],
            ),
            ("HUDComponents", "HOM_GetBonusCollisionScore") => {
                let (car, dmo, ped, skater) = (d.car_scores, d.dmo_scores, d.ped_scores, d.skater_scores);
                let tweak = number(d.tweak_points);
                let car = array(vm, car.map(number))?;
                let dmo = array(vm, dmo.map(number))?;
                let ped = array(vm, ped.map(number))?;
                let skater = array(vm, skater.map(number))?;
                array(vm, [car, dmo, tweak, ped, skater])
            }
            _ => Err(format!("Unimplemented Hall of Meat HUD binding {native}.{method}")),
        }
    }
}

/// The running `homscoring` movie.
pub struct HomPlayer {
    core: MoviePlayer,
    controller: usize,
    /// `homscoring.mScreen` (Screen_EdgeOffset target).
    edge: EdgeScreen,
}

impl HomPlayer {
    pub fn new(assets: &Assets) -> Result<Self, String> {
        let core = MoviePlayer::new(assets, "Hall of Meat HUD")?;
        let Value::Object(controller) = core.vm.get(core.bindings.movie.root, "screen") else {
            return Err("Hall of Meat HUD controller was not constructed".into());
        };
        let screen = match core.vm.get(core.vm.global, "homscoring") {
            Value::Object(class) => match core.vm.get(class, "mScreen") {
                Value::Object(id) => Some(id),
                _ => None,
            },
            _ => None,
        };
        let edge = EdgeScreen::new(&core.vm, screen);
        Ok(Self { core, controller, edge })
    }

    pub fn set_edge_offset(&mut self, offset: f32) -> Result<(), String> {
        self.edge.set(&mut self.core.vm, offset)
    }

    pub fn edge_offset(&self) -> f32 {
        self.edge.offset()
    }

    /// Publishes the scorer's data for the movie's next native calls.
    pub fn set_data(&mut self, data: &HomData) {
        self.core.bindings.data = data.clone();
    }

    /// `homscoring.SetState(state)` (see [`state`]).
    pub fn set_state(&mut self, state: i32) -> Result<(), String> {
        let core = &mut self.core;
        core.vm.begin_update();
        core.vm.call_method(self.controller, "SetState", vec![Value::Number(state as f64)], &mut core.bindings)?;
        core.drain()
    }

    /// Whether the movie shows itself (`homscoring.mVisible`).
    pub fn visible(&self) -> bool {
        let vm = &self.core.vm;
        match vm.get(vm.global, "homscoring") {
            Value::Object(class) => vm.get(class, "mVisible").truth(),
            _ => false,
        }
    }

    /// One timeline frame.
    pub fn advance(&mut self) -> Result<(), String> {
        self.core.advance()
    }

    /// The movie's scene in `layout`'s pixels, moved to [`placement`].
    pub fn frame(&self, assets: &Assets, layout: &Layout) -> Result<Frame, String> {
        let mut frame = self.native_frame(assets, layout)?;
        let place = placement(layout);
        for v in &mut frame.vertices {
            [v.x, v.y] = place.apply([v.x, v.y]);
        }
        Ok(frame)
    }

    /// The movie's own layout (source comparisons).
    pub fn native_frame(&self, assets: &Assets, layout: &Layout) -> Result<Frame, String> {
        self.core.native_frame(assets, layout)
    }

    /// Text currently shown by the movie's text fields (diagnostics/tests).
    pub fn visible_text(&self) -> Vec<String> {
        self.core.visible_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn placement_keeps_the_minimap_and_feed_clear() {
        for (w, h, sz) in [(1920, 1080, 1.0), (2560, 1080, 1.0), (1920, 1200, 0.9), (3840, 2160, 1.0)] {
            let l = Layout::compute(w, h, sz, crate::layout::DEFAULT_MAX_ASPECT);
            let map = minimap(&l);
            let p = placement(&l);
            let [left, bottom] = p.apply(l.to_pixels([SCORE_BOX_LEFT, SCORE_BOX_BOTTOM]));
            assert!((left - map[0]).abs() < 0.01, "{w}x{h}");
            let room = map[1] - bottom;
            assert!((room - FEED_ROOM * (map[3] - map[1])).abs() < 0.01, "{w}x{h}");
            assert!(map[0] >= l.safe[0] && map[3] <= l.safe[3]);
            let [_, top] = p.apply(l.to_pixels([SCORE_BOX_LEFT, STACK_TOP]));
            assert!(top >= l.safe[1] - 0.01, "{w}x{h}: stack top {top}");
            assert!(p.scale > 0.0 && p.scale <= 1.0);
        }
    }

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(999.4), "999");
        assert_eq!(format_number(1234567.0), "1,234,567");
        assert_eq!(format_number(-4321.0), "-4,321");
        assert_eq!(format_number(f64::NAN), "0");
    }

    #[test]
    fn custom_strings_expand_units() {
        let mut l = BTreeMap::new();
        l.insert("ID_COMMON_KMPH".to_string(), "Km/h".to_string());
        l.insert("ID_COMMON_METRIC_METER_FLOAT".to_string(), "{0:f.1} m".to_string());
        assert_eq!(format_custom("{0:speed}", "10", &l), "36 Km/h");
        assert_eq!(format_custom("{0:distance}", "2.26", &l), "2.3 m");
        assert_eq!(format_custom("x{0}y", "7", &l), "x7y");
        assert_eq!(format_custom("{0:f.2}", "1.5", &l), "1.50");
        assert_eq!(format_custom("{0:speed}", "nope", &l), "0 Km/h");
    }
}

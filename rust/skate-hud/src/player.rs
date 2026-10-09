//! Project-owned glue around the donor HUD runtime: loads the converted
//! `trickdisplay` movie and its RGBA textures, maps Skate's scorer onto the
//! movie's native bindings, applies the movie's edge offset and flattens the
//! scene into one draw list in output pixels.
use crate::apt_vm::Value;
use crate::layout::Layout;
use crate::movie::EdgeScreen;
use crate::{apt_scene, hud_runtime};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Skate's scorer publication after one simulation tick (skate-host overlay
/// patch 0010 `Session::hud_input`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScoreInput {
    pub sequence_score: f32,
    pub line_score: f32,
    pub line_points: f32,
    pub line_drain: f32,
    pub line_capacity: f32,
    pub multiplier: f32,
    pub clean: bool,
    pub sketchy: bool,
    pub stance: [bool; 4],
    pub trick_name: String,
    pub new_trick: bool,
    pub modified_trick: bool,
    pub close_tricks: bool,
}

impl ScoreInput {
    /// The movie's native binding values. Same derivation as the donor's
    /// `ScoringRuntime::hud_input` (skate-game/src/scoring_runtime.rs
    /// @ cb79689, lines 115-136): the line timer is reported in seconds
    /// (points / drain), scores as integers, the metrics slot carries the
    /// trick name, and there are no context tricks.
    pub fn runtime_input(&self) -> hud_runtime::Input {
        let line_time = if self.line_drain != 0.0 {
            self.line_points / self.line_drain
        } else {
            0.0
        };
        hud_runtime::Input {
            sequence_score: finite_i32(self.sequence_score),
            line_score: finite_i32(self.line_score),
            sequence_timer: finite_i32(line_time),
            line_time: if line_time.is_finite() { line_time } else { 0.0 },
            line_capacity: self.line_capacity,
            multiplier: self.multiplier,
            clean: self.clean,
            sketchy: self.sketchy,
            stance: self.stance,
            trick_name: self.trick_name.clone(),
            trick_metrics: [
                Value::Text(self.trick_name.clone()),
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(false),
            ],
            context_tricks: Vec::new(),
        }
    }
}

fn finite_i32(v: f32) -> i32 {
    if v.is_finite() { v as i32 } else { 0 }
}

/// One RGBA8 texture (straight alpha, sRGB colour) used by the movie.
#[derive(Clone)]
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The converted movie: `assets/private/hud/runtime/trickdisplay.json` plus
/// the raw RGBA files it lists (donor `tools/prepare_runtime_huds.py`).
pub struct Assets {
    pub root: PathBuf,
    source: serde_json::Value,
    shapes: apt_scene::Shapes,
    pub textures: Vec<Texture>,
    index: HashMap<String, u32>,
}

impl Assets {
    /// `<data_root>/private/hud` for a SkateV data root (`<dir>/assets`).
    pub fn dir_for(data_root: &Path) -> PathBuf {
        data_root.join("private").join("hud")
    }

    /// `<data_root>/private/hud-hom` (tools/prepare-hom-hud.py).
    pub fn hom_dir_for(data_root: &Path) -> PathBuf {
        data_root.join("private").join("hud-hom")
    }

    pub fn load(root: &Path) -> Result<Self, String> {
        Self::load_movie(root, "trickdisplay.json")
    }

    /// `<data_root>/private/hud-chyron` (tools/prepare-hom-hud.py --movie chyron).
    pub fn chyron_dir_for(data_root: &Path) -> PathBuf {
        data_root.join("private").join("hud-chyron")
    }

    /// The TRAX now-playing movie (`runtime/chyron.json`).
    pub fn load_chyron(root: &Path) -> Result<Self, String> {
        Self::load_movie(root, "chyron.json")
    }

    /// The Hall of Meat movie (`runtime/homscoring.json`).
    pub fn load_hom(root: &Path) -> Result<Self, String> {
        Self::load_movie(root, "homscoring.json")
    }

    pub(crate) fn source(&self) -> &serde_json::Value {
        &self.source
    }

    pub(crate) fn texture_index(&self, name: &str) -> Option<u32> {
        self.index.get(name).copied()
    }

    pub(crate) fn shapes(&self) -> &apt_scene::Shapes {
        &self.shapes
    }

    fn load_movie(root: &Path, manifest: &str) -> Result<Self, String> {
        let path = root.join("runtime").join(manifest);
        let source: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;
        if source["format"] != "skate3-scoring-hud" || source["version"] != 1 {
            return Err(format!("{} is not a v1 skate3-scoring-hud", path.display()));
        }
        let shapes: apt_scene::Shapes =
            serde_json::from_value(source["shapes"].clone()).map_err(|e| e.to_string())?;
        // Same file set the donor's scoring_hud::setup loads: shape textures
        // and every font atlas the movie's text assets reference.
        let text = crate::apt_text::TextAssets::load(&source)?;
        let mut files: Vec<(String, [u32; 2])> = Vec::new();
        for shape in shapes.values().flatten() {
            files.push((shape.texture.rgba.clone(), [shape.texture.width, shape.texture.height]));
        }
        for font in text.fonts.values() {
            files.push((font.texture.clone(), font.size));
        }
        files.sort();
        files.dedup_by(|a, b| a.0 == b.0);
        let mut textures = Vec::with_capacity(files.len());
        let mut index = HashMap::new();
        for (name, [width, height]) in files {
            let file = contained(root, &name)?;
            let rgba = std::fs::read(&file).map_err(|e| format!("HUD {name}: {e}"))?;
            if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
                return Err(format!("invalid HUD texture size {name}"));
            }
            index.insert(name.clone(), textures.len() as u32);
            textures.push(Texture {
                name,
                width,
                height,
                rgba,
            });
        }
        Ok(Self {
            root: root.to_path_buf(),
            source,
            shapes,
            textures,
            index,
        })
    }
}

fn contained(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let rel = Path::new(relative);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::Prefix(_)))
    {
        return Err(format!("HUD path escapes its root: {relative}"));
    }
    Ok(root.join(rel))
}

/// One vertex in output pixels with its texture coordinate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
}

/// A run of triangles (`count` vertices from `first`) sampling one texture
/// through the APT colour transform `clamp(texel * multiply + add, 0, 1)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DrawCmd {
    pub texture: u32,
    pub first: u32,
    pub count: u32,
    pub multiply: [f32; 4],
    pub add: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub layout: Option<Layout>,
    pub draws: Vec<DrawCmd>,
    pub vertices: Vec<Vertex>,
}

/// The running movie.
pub struct Player {
    pub runtime: hud_runtime::Runtime,
    /// The movie's `mScreen` container (Screen_EdgeOffset target).
    edge: EdgeScreen,
    pub ticks: u64,
}

impl Player {
    pub fn new(assets: &Assets, input: &ScoreInput) -> Result<Self, String> {
        let runtime = hud_runtime::Runtime::load(&assets.source, input.runtime_input())?;
        // trickdisplay2's constructor did `trickdisplay2.mScreen._x +=
        // _global.Screen_EdgeOffset` with the runtime's 0 (mScreen is the
        // movie root it was constructed with); later offsets move that
        // container from its constructed position.
        let vm = &runtime.vm;
        let screen = match vm.get(vm.global, "trickdisplay2") {
            Value::Object(class) => match vm.get(class, "mScreen") {
                Value::Object(id) => Some(id),
                _ => None,
            },
            _ => None,
        }
        .or(Some(runtime.bindings.movie.root));
        let edge = EdgeScreen::new(&runtime.vm, screen);
        Ok(Self { runtime, edge, ticks: 0 })
    }

    /// The movie's `Screen_EdgeOffset`, applied the way its constructor does.
    pub fn set_edge_offset(&mut self, offset: f32) -> Result<(), String> {
        self.edge.set(&mut self.runtime.vm, offset)
    }

    pub fn edge_offset(&self) -> f32 {
        self.edge.offset()
    }

    /// One Skate tick: the donor's `scoring_hud::advance`.
    pub fn update(&mut self, input: &ScoreInput) -> Result<(), String> {
        self.runtime.update(
            input.runtime_input(),
            input.new_trick,
            input.modified_trick,
            input.close_tricks,
        )?;
        self.ticks += 1;
        Ok(())
    }

    /// The movie's current scene as one draw list in `layout`'s pixels.
    pub fn frame(&mut self, assets: &Assets, layout: &Layout) -> Result<Frame, String> {
        let runtime = &mut self.runtime;
        let draws = crate::presentation::compact(&runtime.bindings.movie, &mut runtime.vm,
            &assets.shapes, layout.edge_offset)?;
        Ok(flatten(draws, assets, layout))
    }

    /// Unmodified movie layout for source comparisons and review mockups.
    pub fn native_frame(&self, assets: &Assets, layout: &Layout) -> Result<Frame, String> {
        let draws = apt_scene::draw(&self.runtime.bindings.movie, &self.runtime.vm, &assets.shapes)?;
        Ok(flatten(draws, assets, layout))
    }

    /// Text currently shown by the movie's text fields (diagnostics/tests).
    pub fn visible_text(&self) -> Vec<String> {
        crate::movie::visible_text(&self.runtime.bindings.movie, &self.runtime.vm)
    }
}

pub(crate) fn flatten(draws: Vec<apt_scene::Draw>, assets: &Assets, layout: &Layout) -> Frame {
        let mut frame = Frame {
            layout: Some(*layout),
            draws: Vec::with_capacity(draws.len()),
            vertices: Vec::new(),
        };
        for d in draws {
            let Some(&texture) = assets.index.get(&d.texture) else {
                continue;
            };
            if d.vertices.is_empty() {
                continue;
            }
            let first = frame.vertices.len() as u32;
            for v in &d.vertices {
                let [x, y] = layout.to_pixels(v.position);
                frame.vertices.push(Vertex {
                    x,
                    y,
                    u: v.uv[0],
                    v: v.uv[1],
                });
            }
            frame.draws.push(DrawCmd {
                texture,
                first,
                count: d.vertices.len() as u32,
                multiply: d.multiply,
                add: d.add,
            });
        }
        frame
}

pub(crate) fn collect_text(movie: &crate::apt_movie::Movie, vm: &crate::apt_vm::Vm, id: usize, out: &mut Vec<String>) {
    if !vm.get(id, "_visible").truth() || vm.get(id, "_alpha").number() <= 0.0 {
        return;
    }
    let Some(instance) = movie.instances.get(&id) else {
        return;
    };
    if movie.characters[&instance.character].text.is_some() {
        let t = vm.get(id, "_displayText").text();
        if !t.is_empty() && t != "undefined" {
            out.push(t);
        }
    }
    for child in instance.children.values() {
        collect_text(movie, vm, *child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scorer_maps_onto_the_movie_bindings() {
        let s = ScoreInput {
            sequence_score: 1020.7,
            line_score: 1530.2,
            line_points: 175.0,
            line_drain: 50.0,
            line_capacity: 400.0,
            multiplier: 1.5,
            clean: true,
            sketchy: false,
            stance: [true, false, true, false],
            trick_name: "GRIND_BS_LIP_SLIDE".into(),
            ..Default::default()
        };
        let i = s.runtime_input();
        assert_eq!(i.sequence_score, 1020);
        assert_eq!(i.line_score, 1530);
        assert_eq!(i.sequence_timer, 3);
        assert_eq!(i.line_time, 3.5);
        assert_eq!(i.line_capacity, 400.0);
        assert_eq!(i.multiplier, 1.5);
        assert!(i.clean && !i.sketchy);
        assert_eq!(i.stance, [true, false, true, false]);
        assert_eq!(i.trick_metrics[0], Value::Text("GRIND_BS_LIP_SLIDE".into()));
        assert!(i.context_tricks.is_empty());
    }

    #[test]
    fn nonfinite_scorer_values_never_reach_the_movie() {
        let s = ScoreInput {
            sequence_score: f32::NAN,
            line_points: 10.0,
            line_drain: 0.0,
            ..Default::default()
        };
        let i = s.runtime_input();
        assert_eq!(i.sequence_score, 0);
        assert_eq!(i.line_time, 0.0);
        assert_eq!(i.sequence_timer, 0);
    }

    #[test]
    fn hud_paths_cannot_escape_the_root() {
        let root = Path::new("C:/hud");
        assert!(contained(root, "assets/x.rgba").is_ok());
        assert!(contained(root, "../x.rgba").is_err());
        assert!(contained(root, "C:/windows/x").is_err());
    }
}

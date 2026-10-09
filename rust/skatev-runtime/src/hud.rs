//! Skate 3's original HUD (ABI 8, workstream "hud").
//!
//! The APT `trickdisplay` movie runs on the Skate worker through the donor's
//! HUD runtime (`skate-hud`, compiled in place from the pinned donor), one
//! movie update per Skate tick, fed by Skate's own scorer (overlay patch 0010
//! `Session::hud_input`). Each publication flattens the movie into one draw
//! list in the host's backbuffer pixels (layout: `skate_hud::layout`); the
//! host only rasterises it (host/src/hud_overlay.cpp). Nothing here scores.
//!
//! With Hall of Meat on, the APT `homscoring` movie runs beside it, fed by
//! the WipeoutScorer's HUD data (hom.rs); its textures follow the trick
//! movie's in the shared set and its draws follow the trick movie's.
//!
//! The APT `chyron` movie (Skate's TRAX now-playing banner) shows GTA's radio
//! station when the host changes it on the board (`sv_show_radio`), top-left,
//! with the station's own logo in place of the EA logo when
//! tools/prepare-hom-hud.py --movie chyron --gta exported the logos.
use crate::hom::TickOutput;
use crate::worker::{Log, Shared};
use skate_hud::hom::state;
use skate_hud::layout::{DEFAULT_MAX_ASPECT, Layout};
use skate_hud::player::Texture;
use skate_hud::{Assets, ChyronPlayer, Frame, HomPlayer, Player, ScoreInput};
use std::ffi::c_void;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Host backbuffer + presentation settings (`sv_set_hud_viewport`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvHudViewport {
    pub size: u32,
    pub width: u32,
    pub height: u32,
    /// GTA `GET_SAFE_ZONE_SIZE` (1.0 = no inset).
    pub safe_zone: f32,
    /// Widest HUD region aspect; <= 0 uncapped.
    pub max_aspect: f32,
    /// Bit 0: HUD enabled (the runtime runs the movie only when set).
    pub flags: u32,
}

/// One HUD texture: RGBA8, straight alpha, sRGB colour.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvHudTexture {
    pub size: u32,
    pub width: u32,
    pub height: u32,
    /// Texture set generation (changes if the set is ever reloaded).
    pub generation: u32,
}

/// Backbuffer pixels (origin top-left) and texture coordinates.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvHudVertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
}

/// Triangles `first_vertex .. first_vertex + vertex_count` sampling
/// `texture` through `clamp(texel * multiply + add, 0, 1)`, straight-alpha
/// blended into a transparent sRGB target that is then composited
/// premultiplied over the frame (the donor's hud_render/hud_composite).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvHudDraw {
    pub texture: u32,
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub flags: u32,
    pub multiply: [f32; 4],
    pub add: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvHudFrame {
    pub size: u32,
    /// 0: draw nothing (not skating, HUD disabled/unavailable).
    pub visible: u32,
    /// Increments with every publication.
    pub serial: u64,
    /// Backbuffer size the vertices were laid out for.
    pub width: u32,
    pub height: u32,
    pub draw_count: u32,
    pub vertex_count: u32,
    pub texture_count: u32,
    pub texture_generation: u32,
    /// Pixels per movie unit and `Screen_EdgeOffset` (movie units).
    pub scale: f32,
    pub edge_offset: f32,
    /// HUD region in pixels: x0, y0, x1, y1.
    pub region: [f32; 4],
}

/// HUD publication inside the worker's `Shared`.
#[derive(Clone, Default)]
pub struct HudShared {
    pub viewport: SvHudViewport,
    pub textures: Arc<Vec<Texture>>,
    pub generation: u32,
    pub visible: bool,
    pub serial: u64,
    pub frame: Arc<Frame>,
    /// Pending radio banner (`sv_show_radio`): the three lines, then the
    /// station id for its logo (GET_RADIO_STATION_NAME).
    pub radio: Option<[String; 4]>,
}

/// Skate's scorer -> the movie input (field-for-field; see patch 0010).
pub fn score_input(h: &skate_host::bridge::hud::HudInput) -> ScoreInput {
    ScoreInput {
        sequence_score: h.sequence_score,
        line_score: h.line_score,
        line_points: h.line_points,
        line_drain: h.line_drain,
        line_capacity: h.line_capacity,
        multiplier: h.multiplier,
        clean: h.clean,
        sketchy: h.sketchy,
        stance: h.stance,
        trick_name: h.trick_name.clone(),
        new_trick: h.new_trick,
        modified_trick: h.modified_trick,
        close_tricks: h.close_tricks,
    }
}

fn layout_for(v: &SvHudViewport) -> Option<Layout> {
    (v.flags & 1 != 0 && v.width > 0 && v.height > 0).then(|| {
        let max_aspect = if v.max_aspect.is_finite() && v.max_aspect != 0.0 {
            v.max_aspect
        } else {
            DEFAULT_MAX_ASPECT
        };
        Layout::compute(v.width, v.height, v.safe_zone, max_aspect)
    })
}

/// When the Hall of Meat movie shows (presentation choice, docs/DECISIONS.md
/// "Hall of Meat HUD"): Show when a wipeout starts, UpdateScoring on every
/// publication while shown, Hide when the scorer resets or, once the
/// wipeout is over, when the rider is back on the board.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HomShow {
    pub shown: bool,
    pub over: bool,
}

impl HomShow {
    /// `homscoring.SetState` calls for this tick, in order.
    pub fn step(&mut self, out: &TickOutput, category: u32) -> Vec<i32> {
        let mut calls = Vec::new();
        // FilteredCategory 1 Ground, 2 Air, 3 Grind.
        let on_board = matches!(category, 1..=3);
        if self.shown && (out.reset || (self.over && on_board)) {
            calls.push(state::HIDE);
            *self = Self::default();
        }
        if out.started {
            calls.push(state::SHOW);
            *self = Self { shown: true, over: false };
        }
        if out.published && self.shown {
            calls.push(state::UPDATE_SCORING);
        }
        if out.ended && self.shown {
            self.over = true;
        }
        calls
    }
}

struct HomMovie {
    assets: Arc<Assets>,
    /// Index of its first texture in the shared set.
    texture_base: u32,
    player: Option<HomPlayer>,
    show: HomShow,
    failed: bool,
}

/// How long the radio banner stays before its outro.
const RADIO_SECONDS: f32 = 4.0;

struct ChyronMovie {
    assets: Arc<Assets>,
    texture_base: u32,
    /// The EA logo's texture in `assets`.
    logo: Option<u32>,
    /// Station id (lowercase) -> texture index relative to `texture_base`.
    logos: std::collections::HashMap<String, u32>,
    player: Option<ChyronPlayer>,
    station: Option<u32>,
    shown_at: Option<std::time::Instant>,
}

/// `<dir>/logos/logos.json`: [{"id", "file", "width", "height"}] (RGBA8).
fn station_logos(dir: &Path) -> Vec<(String, Texture)> {
    let Ok(text) = std::fs::read(dir.join("logos").join("logos.json")) else {
        return Vec::new();
    };
    let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(&text) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|r| {
            let (id, file) = (r["id"].as_str()?, r["file"].as_str()?);
            let (width, height) = (r["width"].as_u64()? as u32, r["height"].as_u64()? as u32);
            if file.contains("..") || file.contains(['/', '\\']) {
                return None;
            }
            let rgba = std::fs::read(dir.join("logos").join(file)).ok()?;
            (rgba.len() == width as usize * height as usize * 4).then(|| {
                (id.to_ascii_lowercase(), Texture { name: format!("logos/{file}"), width, height, rgba })
            })
        })
        .collect()
}

/// Worker-side owner of the running movies.
pub struct Driver {
    assets: Option<Arc<Assets>>,
    player: Option<Player>,
    layout: Option<Layout>,
    failed: bool,
    hom: Option<HomMovie>,
    chyron: Option<ChyronMovie>,
}

impl Driver {
    /// Loads `<data_root>/private/hud` (convert-skate-data.py `hud` export)
    /// and, with Hall of Meat on, `<data_root>/private/hud-hom`
    /// (tools/prepare-hom-hud.py).
    pub fn load(data_root: &Path, hall_of_meat: bool, shared: &Mutex<Shared>, log: &Log) -> Self {
        let dir = Assets::dir_for(data_root);
        let t = std::time::Instant::now();
        let mut textures = Vec::new();
        let assets = match Assets::load(&dir) {
            Ok(a) => {
                log(&format!(
                    "original HUD loaded from {} ({} textures) in {}ms",
                    dir.display(),
                    a.textures.len(),
                    t.elapsed().as_millis()
                ));
                textures.extend(a.textures.iter().cloned());
                Some(Arc::new(a))
            }
            Err(e) => {
                log(&format!(
                    "original HUD unavailable ({e}); run tools/convert-skate-data.py --exports hud --incremental"
                ));
                None
            }
        };
        let hom = if hall_of_meat {
            let dir = Assets::hom_dir_for(data_root);
            match Assets::load_hom(&dir) {
                Ok(a) => {
                    log(&format!(
                        "Hall of Meat HUD loaded from {} ({} textures)",
                        dir.display(),
                        a.textures.len()
                    ));
                    let texture_base = textures.len() as u32;
                    textures.extend(a.textures.iter().cloned());
                    Some(HomMovie {
                        assets: Arc::new(a),
                        texture_base,
                        player: None,
                        show: HomShow::default(),
                        failed: false,
                    })
                }
                Err(e) => {
                    log(&format!(
                        "Hall of Meat HUD unavailable ({e}); run tools/prepare-hom-hud.py (scoring still runs)"
                    ));
                    None
                }
            }
        } else {
            None
        };
        let dir = Assets::chyron_dir_for(data_root);
        let chyron = match Assets::load_chyron(&dir) {
            Ok(a) => {
                let texture_base = textures.len() as u32;
                textures.extend(a.textures.iter().cloned());
                let mut logos = std::collections::HashMap::new();
                for (id, t) in station_logos(&dir) {
                    logos.insert(id, textures.len() as u32 - texture_base);
                    textures.push(t);
                }
                log(&format!(
                    "radio banner loaded from {} ({} textures, {} station logos)",
                    dir.display(),
                    a.textures.len(),
                    logos.len()
                ));
                Some(ChyronMovie {
                    logo: skate_hud::chyron::logo_texture(&a),
                    assets: Arc::new(a),
                    texture_base,
                    logos,
                    player: None,
                    station: None,
                    shown_at: None,
                })
            }
            Err(e) => {
                log(&format!("radio banner unavailable ({e}); run tools/prepare-hom-hud.py --movie chyron"));
                None
            }
        };
        if !textures.is_empty() {
            let mut s = shared.lock().unwrap();
            s.hud.textures = Arc::new(textures);
            s.hud.generation = s.hud.generation.wrapping_add(1).max(1);
        }
        Self {
            assets,
            player: None,
            layout: None,
            failed: false,
            hom,
            chyron,
        }
    }

    fn enabled(&self, shared: &Mutex<Shared>) -> bool {
        self.assets.is_some() && !self.failed && shared.lock().unwrap().hud.viewport.flags & 1 != 0
    }

    /// Skate (re)activated: a fresh movie, as the donor reloads it per map.
    pub fn activate(&mut self, s: &skate_host::bridge::Session, shared: &Mutex<Shared>, log: &Log) {
        self.player = None;
        self.layout = None;
        let hud_on = shared.lock().unwrap().hud.viewport.flags & 1 != 0;
        if let Some(m) = self.hom.as_mut().filter(|m| !m.failed) {
            m.show = HomShow::default();
            m.player = None;
            if hud_on {
                match HomPlayer::new(&m.assets) {
                    Ok(p) => m.player = Some(p),
                    Err(e) => {
                        log(&format!("Hall of Meat HUD stopped: {e}"));
                        m.failed = true;
                    }
                }
            }
        }
        if let Some(c) = self.chyron.as_mut() {
            c.player = None;
            c.shown_at = None;
            if hud_on {
                match ChyronPlayer::new(&c.assets) {
                    Ok(p) => c.player = Some(p),
                    Err(e) => log(&format!("radio banner stopped: {e}")),
                }
            }
        }
        if !self.enabled(shared) {
            self.publish(shared, log);
            return;
        }
        let assets = self.assets.clone().unwrap();
        let t = std::time::Instant::now();
        match Player::new(&assets, &score_input(&s.hud_input())) {
            Ok(p) => {
                log(&format!("original HUD started in {}ms", t.elapsed().as_millis()));
                self.player = Some(p);
            }
            Err(e) => self.fail(&e, log),
        }
        self.publish(shared, log);
    }

    /// After every Skate tick (the donor's `scoring_hud::advance`) and the
    /// WipeoutScorer's tick.
    pub fn tick(
        &mut self,
        s: &skate_host::bridge::Session,
        hom: &crate::hom::Driver,
        shared: &Mutex<Shared>,
        log: &Log,
    ) {
        self.tick_radio(shared, log);
        if let Some(p) = self.player.as_mut() {
            if let Err(e) = p.update(&score_input(&s.hud_input())) {
                self.fail(&e, log);
            }
        }
        let (Some(m), Some(scorer)) = (self.hom.as_mut(), hom.scorer()) else {
            return;
        };
        let Some(p) = m.player.as_mut() else {
            return;
        };
        let calls = if hom.enabled() {
            m.show.step(&hom.last, hom.category)
        } else if m.show.shown {
            // Switched off in game during a bail.
            m.show = HomShow::default();
            vec![state::HIDE]
        } else {
            Vec::new()
        };
        let run = || -> Result<(), String> {
            for call in calls {
                if call != state::HIDE {
                    p.set_data(scorer.hud());
                }
                p.set_state(call)?;
            }
            p.advance()
        };
        if let Err(e) = run() {
            log(&format!("Hall of Meat HUD stopped: {e}"));
            m.failed = true;
            m.player = None;
        }
    }

    fn tick_radio(&mut self, shared: &Mutex<Shared>, log: &Log) {
        let request = shared.lock().unwrap().hud.radio.take();
        let Some(c) = self.chyron.as_mut() else { return };
        // Created on demand: `activate` makes it only when GTA's HUD is on at
        // that moment, so an activation behind a fade or loading screen left
        // the banner gone for the whole session.
        if c.player.is_none() && request.is_some() {
            match ChyronPlayer::new(&c.assets) {
                Ok(p) => c.player = Some(p),
                Err(e) => log(&format!("radio banner stopped: {e}")),
            }
        }
        let Some(p) = c.player.as_mut() else { return };
        let mut run = || -> Result<(), String> {
            if let Some([top, middle, bottom, id]) = &request {
                c.station = c.logos.get(&id.to_ascii_lowercase()).copied();
                p.show(top, middle, bottom)?;
                c.shown_at = Some(std::time::Instant::now());
            } else if c.shown_at.is_some_and(|t| t.elapsed().as_secs_f32() > RADIO_SECONDS) {
                c.shown_at = None;
                p.hide()?;
            }
            p.advance()
        };
        if let Err(e) = run() {
            log(&format!("radio banner stopped: {e}"));
            c.player = None;
        }
    }

    /// Publishes the current scene for the host.
    pub fn publish(&mut self, shared: &Mutex<Shared>, log: &Log) {
        let viewport = shared.lock().unwrap().hud.viewport;
        let layout = layout_for(&viewport);
        let mut hom_frame = None;
        if let (Some(m), Some(l)) = (self.hom.as_mut(), layout) {
            // Placed against GTA's minimap (skate_hud::hom::placement), not
            // by the movie's edge offset.
            if let Some(p) = m.player.as_mut() {
                if p.visible() {
                    match p.frame(&m.assets, &l) {
                        Ok(f) => hom_frame = Some((f, m.texture_base)),
                        Err(e) => {
                            log(&format!("Hall of Meat HUD stopped: {e}"));
                            m.failed = true;
                            m.player = None;
                        }
                    }
                }
            }
        }
        let mut chyron_frame = None;
        if let (Some(c), Some(l)) = (self.chyron.as_mut(), layout) {
            if let Some(p) = c.player.as_ref() {
                match p.frame(&c.assets, &l) {
                    Ok(mut f) => {
                        if let (Some(logo), Some(station)) = (c.logo, c.station) {
                            skate_hud::chyron::replace_logo(&mut f, logo, station);
                        }
                        skate_hud::chyron::close_seams(&mut f);
                        chyron_frame = Some((f, c.texture_base));
                    }
                    Err(e) => {
                        log(&format!("radio banner stopped: {e}"));
                        c.player = None;
                    }
                }
            }
        }
        let mut frame = None;
        if let (Some(p), Some(l), Some(a)) = (self.player.as_mut(), layout, self.assets.as_ref()) {
            if self.layout != Some(l) {
                if let Err(e) = p.set_edge_offset(l.edge_offset) {
                    log(&format!("original HUD edge offset: {e}"));
                }
                log(&format!(
                    "original HUD layout {}x{} safe zone {:.2}: scale {:.3}, edge offset {:.1}, region {:?}",
                    l.width, l.height, viewport.safe_zone, l.scale, l.edge_offset, l.region
                ));
                self.layout = Some(l);
            }
            match p.frame(a, &l) {
                Ok(f) => frame = Some(f),
                Err(e) => self.fail(&e, log),
            }
        }
        for (h, base) in hom_frame.into_iter().chain(chyron_frame) {
            let mut f = frame.unwrap_or_else(|| Frame { layout: h.layout, ..Default::default() });
            append_frame(&mut f, &h, base);
            frame = Some(f);
        }
        if layout.is_some() {
            self.layout = layout;
        }
        let mut s = shared.lock().unwrap();
        s.hud.serial += 1;
        s.hud.visible = frame.is_some();
        s.hud.frame = Arc::new(frame.unwrap_or_default());
    }

    /// Skate suspended: hide; the movie restarts on the next activation.
    pub fn suspend(&mut self, shared: &Mutex<Shared>) {
        self.player = None;
        if let Some(m) = self.hom.as_mut() {
            m.player = None;
            m.show = HomShow::default();
        }
        if let Some(c) = self.chyron.as_mut() {
            c.player = None;
            c.shown_at = None;
        }
        let mut s = shared.lock().unwrap();
        s.hud.serial += 1;
        s.hud.visible = false;
        s.hud.frame = Arc::default();
    }

    fn fail(&mut self, e: &str, log: &Log) {
        log(&format!("original HUD stopped: {e}"));
        self.failed = true;
        self.player = None;
    }
}

/// Appends `extra`'s draws (texture indices offset by `texture_base`).
fn append_frame(frame: &mut Frame, extra: &Frame, texture_base: u32) {
    let first = frame.vertices.len() as u32;
    frame.vertices.extend_from_slice(&extra.vertices);
    frame.draws.extend(extra.draws.iter().map(|d| skate_hud::DrawCmd {
        texture: d.texture + texture_base,
        first: d.first + first,
        ..*d
    }));
}

/// Radio on the board: Skate's TRAX banner shows `top` (big), `middle` and
/// `bottom` (song, artist, station), with the logo exported for station `id`
/// (GTA's GET_RADIO_STATION_NAME; may be null). UTF-8, NUL-terminated.
/// Returns 1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_show_radio(
    rt: *mut c_void,
    top: *const std::ffi::c_char,
    middle: *const std::ffi::c_char,
    bottom: *const std::ffi::c_char,
    id: *const std::ffi::c_char,
) -> u32 {
    crate::guard(0, || {
        let Some(shared) = runtime_shared(rt) else { return 0 };
        let text = |p: *const std::ffi::c_char| -> String {
            if p.is_null() {
                String::new()
            } else {
                unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().chars().take(64).collect()
            }
        };
        shared.lock().unwrap().hud.radio = Some([text(top), text(middle), text(bottom), text(id)]);
        1
    })
}

fn runtime_shared(rt: *const c_void) -> Option<Arc<Mutex<Shared>>> {
    unsafe { crate::runtime(rt) }.map(|r| Arc::clone(&r.shared))
}

/// Host backbuffer size, GTA safe zone and enable flag. Returns 1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_set_hud_viewport(rt: *mut c_void, viewport: *const SvHudViewport) -> u32 {
    crate::guard(0, || {
        let (Some(shared), Some(v)) = (runtime_shared(rt), unsafe { viewport.as_ref() }) else {
            return 0;
        };
        if (v.size as usize) < size_of::<SvHudViewport>() {
            return 0;
        }
        shared.lock().unwrap().hud.viewport = *v;
        1
    })
}

/// HUD texture `index`: fills `info`; copies the RGBA8 pixels when `rgba`
/// holds at least width*height*4 bytes. Returns 1 when the texture exists.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_hud_texture(
    rt: *const c_void,
    index: u32,
    info: *mut SvHudTexture,
    rgba: *mut c_void,
    capacity: u32,
) -> u32 {
    crate::guard(0, || {
        let Some(shared) = runtime_shared(rt) else {
            return 0;
        };
        let (textures, generation) = {
            let s = shared.lock().unwrap();
            (Arc::clone(&s.hud.textures), s.hud.generation)
        };
        let Some(t) = textures.get(index as usize) else {
            return 0;
        };
        if let Some(info) = unsafe { info.as_mut() } {
            *info = SvHudTexture {
                size: size_of::<SvHudTexture>() as u32,
                width: t.width,
                height: t.height,
                generation,
            };
        }
        if !rgba.is_null() && capacity as usize >= t.rgba.len() {
            unsafe { std::ptr::copy_nonoverlapping(t.rgba.as_ptr(), rgba as *mut u8, t.rgba.len()) };
        }
        1
    })
}

/// The latest HUD publication. Always fills `frame` (counts are totals);
/// draws/vertices are copied only when both fit their capacities. Returns 1
/// when the arrays were filled (or the frame is empty), 0 otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_get_hud_frame(
    rt: *const c_void,
    frame: *mut SvHudFrame,
    draws: *mut SvHudDraw,
    draw_capacity: u32,
    vertices: *mut SvHudVertex,
    vertex_capacity: u32,
) -> u32 {
    crate::guard(0, || {
        let (Some(shared), Some(out)) = (runtime_shared(rt), unsafe { frame.as_mut() }) else {
            return 0;
        };
        let (h, f) = {
            let s = shared.lock().unwrap();
            (
                (s.hud.visible, s.hud.serial, s.hud.textures.len(), s.hud.generation),
                Arc::clone(&s.hud.frame),
            )
        };
        let l = f.layout.unwrap_or(Layout::compute(1, 1, 1.0, 0.0));
        *out = SvHudFrame {
            size: size_of::<SvHudFrame>() as u32,
            visible: u32::from(h.0 && f.layout.is_some()),
            serial: h.1,
            width: if f.layout.is_some() { l.width } else { 0 },
            height: if f.layout.is_some() { l.height } else { 0 },
            draw_count: f.draws.len() as u32,
            vertex_count: f.vertices.len() as u32,
            texture_count: h.2 as u32,
            texture_generation: h.3,
            scale: l.scale,
            edge_offset: l.edge_offset,
            region: l.region,
        };
        if f.draws.len() > draw_capacity as usize || f.vertices.len() > vertex_capacity as usize {
            return 0;
        }
        if f.draws.is_empty() {
            return 1;
        }
        if draws.is_null() || vertices.is_null() {
            return 0;
        }
        for (i, d) in f.draws.iter().enumerate() {
            unsafe {
                *draws.add(i) = SvHudDraw {
                    texture: d.texture,
                    first_vertex: d.first,
                    vertex_count: d.count,
                    flags: 0,
                    multiply: d.multiply,
                    add: d.add,
                };
            }
        }
        for (i, v) in f.vertices.iter().enumerate() {
            unsafe {
                *vertices.add(i) = SvHudVertex {
                    x: v.x,
                    y: v.y,
                    u: v.u,
                    v: v.v,
                };
            }
        }
        1
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    /// Must match the static_asserts in the hud block of skatev_runtime.h.
    #[test]
    fn hud_abi_layout_matches_header() {
        assert_eq!(size_of::<SvHudViewport>(), 24);
        assert_eq!(offset_of!(SvHudViewport, safe_zone), 12);
        assert_eq!(offset_of!(SvHudViewport, flags), 20);
        assert_eq!(size_of::<SvHudTexture>(), 16);
        assert_eq!(size_of::<SvHudVertex>(), 16);
        assert_eq!(size_of::<SvHudDraw>(), 48);
        assert_eq!(offset_of!(SvHudDraw, multiply), 16);
        assert_eq!(offset_of!(SvHudDraw, add), 32);
        assert_eq!(size_of::<SvHudFrame>(), 64);
        assert_eq!(offset_of!(SvHudFrame, serial), 8);
        assert_eq!(offset_of!(SvHudFrame, width), 16);
        assert_eq!(offset_of!(SvHudFrame, draw_count), 24);
        assert_eq!(offset_of!(SvHudFrame, texture_generation), 36);
        assert_eq!(offset_of!(SvHudFrame, scale), 40);
        assert_eq!(offset_of!(SvHudFrame, region), 48);
    }

    #[test]
    fn viewport_maps_to_layout_only_when_enabled() {
        let mut v = SvHudViewport {
            size: size_of::<SvHudViewport>() as u32,
            width: 2560,
            height: 1080,
            safe_zone: 1.0,
            max_aspect: 0.0,
            flags: 0,
        };
        assert!(layout_for(&v).is_none());
        v.flags = 1;
        let l = layout_for(&v).unwrap();
        assert_eq!(l.region, [0.0, 0.0, 2560.0, 1080.0]);
        v.width = 0;
        assert!(layout_for(&v).is_none());
    }

    #[test]
    fn hall_of_meat_shows_for_the_bail_and_hides_back_on_the_board() {
        let mut show = HomShow::default();
        let tick = |started, ended, reset, published| TickOutput {
            started,
            ended,
            reset,
            published,
            ..Default::default()
        };
        // Riding: nothing.
        assert!(show.step(&tick(false, false, false, false), 1).is_empty());
        // Fresh wipeout: reset + start + first publication.
        assert_eq!(show.step(&tick(true, false, true, true), 4), [state::SHOW, state::UPDATE_SCORING]);
        assert_eq!(show.step(&tick(false, false, false, true), 4), [state::UPDATE_SCORING]);
        // Over: final publication; stays up while off the board.
        assert_eq!(show.step(&tick(false, true, false, true), 4), [state::UPDATE_SCORING]);
        assert!(show.step(&tick(false, false, false, false), 5).is_empty());
        assert!(show.shown && show.over);
        // Back on the board.
        assert_eq!(show.step(&tick(false, false, false, false), 1), [state::HIDE]);
        assert!(!show.shown);
        // A new bail while the last one is still shown restarts the movie.
        show.step(&tick(true, false, true, true), 4);
        assert_eq!(
            show.step(&tick(true, false, true, true), 4),
            [state::HIDE, state::SHOW, state::UPDATE_SCORING]
        );
    }

    #[test]
    fn hall_of_meat_draws_follow_the_trick_draws() {
        let v = skate_hud::Vertex::default();
        let d = |texture, first, count| skate_hud::DrawCmd { texture, first, count, ..Default::default() };
        let mut f = Frame { layout: None, draws: vec![d(0, 0, 3)], vertices: vec![v; 3] };
        let h = Frame { layout: None, draws: vec![d(1, 0, 6), d(0, 6, 3)], vertices: vec![v; 9] };
        append_frame(&mut f, &h, 10);
        assert_eq!(f.vertices.len(), 12);
        assert_eq!(f.draws[1], d(11, 3, 6));
        assert_eq!(f.draws[2], d(10, 9, 3));
    }

    #[test]
    fn null_handles_are_rejected() {
        let mut f = SvHudFrame::default();
        assert_eq!(
            unsafe { sv_get_hud_frame(std::ptr::null(), &mut f, std::ptr::null_mut(), 0, std::ptr::null_mut(), 0) },
            0
        );
        assert_eq!(unsafe { sv_set_hud_viewport(std::ptr::null_mut(), std::ptr::null()) }, 0);
        assert_eq!(
            unsafe { sv_get_hud_texture(std::ptr::null(), 0, std::ptr::null_mut(), std::ptr::null_mut(), 0) },
            0
        );
    }
}

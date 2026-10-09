//! Skate 3's TRAX now-playing banner (the APT `chyron` movie), shown for
//! GTA's radio while skating. Same VM and natives as the Hall of Meat movie.
use crate::apt_vm::Value;
use crate::hom::Placement;
use crate::layout::Layout;
use crate::movie::MoviePlayer;
use crate::player::{Assets, Frame, Vertex};

pub struct ChyronPlayer {
    core: MoviePlayer,
}

impl ChyronPlayer {
    pub fn new(assets: &Assets) -> Result<Self, String> {
        Ok(Self { core: MoviePlayer::new(assets, "chyron")? })
    }

    /// One timeline frame.
    pub fn advance(&mut self) -> Result<(), String> {
        self.core.advance()
    }

    /// The movie's scene in `layout`'s pixels (its own placement).
    pub fn native_frame(&self, assets: &Assets, layout: &Layout) -> Result<Frame, String> {
        self.core.native_frame(assets, layout)
    }

    /// The scene moved to [`placement`].
    pub fn frame(&self, assets: &Assets, layout: &Layout) -> Result<Frame, String> {
        let mut frame = self.native_frame(assets, layout)?;
        let place = placement(layout);
        for v in &mut frame.vertices {
            [v.x, v.y] = place.apply([v.x, v.y]);
        }
        // The movie measures text in `testlength` fields parked below the
        // screen; moved up they would show, so nothing below the box draws.
        let bottom = place.apply(layout.to_pixels([BOX_LEFT, BOX_BOTTOM]))[1] + 4.0;
        let vertices = &frame.vertices;
        frame.draws.retain(|d| {
            vertices[d.first as usize..(d.first + d.count) as usize].iter().any(|v| v.y <= bottom)
        });
        Ok(frame)
    }

    fn call_global(&mut self, name: &str, args: Vec<Value>) -> Result<(), String> {
        let core = &mut self.core;
        core.vm.begin_update();
        let g = core.vm.global;
        core.vm.call_method(g, name, args, &mut core.bindings)?;
        core.drain()
    }

    /// `Chyron_SetVisible("true")` then `Chyron_Display(top, middle,
    /// bottom)`: sets the three lines (big first) and plays the intro.
    pub fn show(&mut self, top: &str, middle: &str, bottom: &str) -> Result<(), String> {
        // The movie compares its argument with the string "true".
        self.call_global("Chyron_SetVisible", vec![Value::Text("true".into())])?;
        let text = |t: &str| Value::Text(t.into());
        self.call_global("Chyron_Display", vec![text(top), text(middle), text(bottom)])
    }

    /// `Chyron_PlayOutro`: slides the banner away.
    pub fn hide(&mut self) -> Result<(), String> {
        self.call_global("Chyron_PlayOutro", vec![])
    }

    pub fn visible_text(&self) -> Vec<String> {
        self.core.visible_text()
    }
}

/// The banner box at rest, movie units (measured from the rendered movie):
/// left, top, bottom. The movie sits it bottom-left, `720 - BOX_BOTTOM` above
/// the bottom edge.
pub const BOX_LEFT: f32 = 82.7;
pub const BOX_TOP: f32 = 540.7;
pub const BOX_BOTTOM: f32 = 632.7;

/// The box's top-left in GTA's own top-left HUD corner (the safe area, where
/// GTA's help text sits), a hair inside it, instead of the movie's margins
/// inside the 16:9 region.
pub fn placement(layout: &Layout) -> Placement {
    let from = layout.to_pixels([BOX_LEFT, BOX_TOP]);
    let margin = 0.01 * layout.height as f32;
    let to = [layout.safe[0] + margin, layout.safe[1] + margin];
    Placement { from, to, scale: 1.0 }
}

/// Character 12 is the EA TRAX logo (a 64x32 unit bitmap shape).
const LOGO_SHAPE: i32 = 12;

/// The logo's texture in `assets`' set.
pub fn logo_texture(assets: &Assets) -> Option<u32> {
    let name = &assets.shapes().get(&LOGO_SHAPE)?.first()?.texture.rgba;
    assets.texture_index(name)
}

/// Swaps the logo's draws for `texture` (a square image), centred where the
/// logo was and sized to the banner's dark logo box; keeps its fade.
pub fn replace_logo(frame: &mut Frame, logo: u32, texture: u32) {
    for d in frame.draws.iter_mut().filter(|d| d.texture == logo && d.count == 6) {
        let range = d.first as usize..(d.first + d.count) as usize;
        let vs = &frame.vertices[range.clone()];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for v in vs {
            x0 = x0.min(v.x);
            y0 = y0.min(v.y);
            x1 = x1.max(v.x);
            y1 = y1.max(v.y);
        }
        let (cx, cy, h) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (y1 - y0) * 1.1);
        let corner = |u: f32, v: f32| Vertex { x: cx + (u - 0.5) * 2.0 * h, y: cy + (v - 0.5) * 2.0 * h, u, v };
        let quad = [corner(0., 0.), corner(1., 0.), corner(1., 1.), corner(0., 0.), corner(1., 1.), corner(0., 1.)];
        frame.vertices[range].copy_from_slice(&quad);
        d.texture = texture;
        d.multiply = [1.0, 1.0, 1.0, d.multiply[3]];
        d.add = [0.0, 0.0, 0.0, d.add[3]];
    }
}

/// Closes sub-pixel gaps between the banner's single-quad pieces: the main
/// panel is two quads 0.075 units apart, which the GPU leaves as a 1 px
/// unpainted column wherever it straddles a pixel centre, seen through the
/// translucent logo box. A piece's right or
/// bottom edge moves onto the next piece's left or top edge when it is less
/// than half a pixel short of it.
pub fn close_seams(frame: &mut Frame) {
    let quads: Vec<std::ops::Range<usize>> = frame
        .draws
        .iter()
        .filter(|d| d.count == 6)
        .map(|d| d.first as usize..(d.first + d.count) as usize)
        .collect();
    let bounds = |vs: &[Vertex]| {
        vs.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, v| [b[0].min(v.x), b[1].min(v.y), b[2].max(v.x), b[3].max(v.y)])
    };
    let all: Vec<[f32; 4]> = quads.iter().map(|r| bounds(&frame.vertices[r.clone()])).collect();
    for (i, r) in quads.iter().enumerate() {
        let b = all[i];
        let next = |axis: usize| {
            all.iter().map(|o| o[axis]).filter(|&e| e > b[axis + 2] && e - b[axis + 2] < 0.5).fold(None, |m: Option<f32>, e| Some(m.map_or(e, |m| m.min(e))))
        };
        let (nx, ny) = (next(0), next(1));
        for v in &mut frame.vertices[r.clone()] {
            if let Some(x) = nx.filter(|_| v.x == b[2]) { v.x = x }
            if let Some(y) = ny.filter(|_| v.y == b[3]) { v.y = y }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seams_close() {
        let q = |x0: f32, x1: f32| [(x0, 0.0), (x1, 0.0), (x1, 10.0), (x0, 0.0), (x1, 10.0), (x0, 10.0)].map(|(x, y)| Vertex { x, y, u: 0.0, v: 0.0 });
        let mut f = Frame::default();
        f.vertices.extend(q(9.75, 105.663));
        f.vertices.extend(q(105.738, 486.7));
        f.draws.push(crate::player::DrawCmd { first: 0, count: 6, ..Default::default() });
        f.draws.push(crate::player::DrawCmd { first: 6, count: 6, ..Default::default() });
        close_seams(&mut f);
        assert_eq!(f.vertices[1].x, 105.738);
        assert_eq!(f.vertices[0].x, 9.75);
        assert_eq!(f.vertices[7].x, 486.7);
    }

    #[test]
    fn placement_is_the_safe_area_corner() {
        // 0.9 safe zone: the box sits in GTA's corner, 1% of the height in.
        let l = Layout::compute(2560, 1080, 0.9, crate::layout::DEFAULT_MAX_ASPECT);
        let p = placement(&l);
        let [x, y] = p.apply(l.to_pixels([BOX_LEFT, BOX_TOP]));
        assert!((x - (l.safe[0] + 10.8)).abs() < 0.01 && (y - (l.safe[1] + 10.8)).abs() < 0.01, "{x} {y}");
    }

    #[test]
    fn logo_becomes_a_centred_square() {
        let mut f = Frame::default();
        for (x, y) in [(10., 20.), (74., 20.), (74., 52.), (10., 20.), (74., 52.), (10., 52.)] {
            f.vertices.push(Vertex { x, y, u: 0.0, v: 0.0 });
        }
        f.draws.push(crate::player::DrawCmd { texture: 3, first: 0, count: 6, multiply: [0.5; 4], add: [0.1; 4] });
        replace_logo(&mut f, 3, 9);
        assert_eq!(f.draws[0].texture, 9);
        assert_eq!(f.draws[0].multiply, [1.0, 1.0, 1.0, 0.5]);
        let xs: Vec<f32> = f.vertices.iter().map(|v| v.x).collect();
        let ys: Vec<f32> = f.vertices.iter().map(|v| v.y).collect();
        let w = xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min);
        let h = ys.iter().cloned().fold(f32::MIN, f32::max) - ys.iter().cloned().fold(f32::MAX, f32::min);
        assert!((w - h).abs() < 1e-4 && (w - 70.4).abs() < 1e-3);
        assert!((xs.iter().sum::<f32>() / 6.0 - 42.0).abs() < 1e-3);
    }

    /// Retail movie (local data only): the lines show after `show`.
    #[test]
    fn shows_the_station() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/skate-data/assets");
        let Ok(a) = Assets::load_chyron(&Assets::chyron_dir_for(&root)) else { return };
        assert!(logo_texture(&a).is_some());
        let mut p = ChyronPlayer::new(&a).unwrap();
        p.show("Radio Los Santos", "Station 3 of 19", "").unwrap();
        for _ in 0..30 {
            p.advance().unwrap();
        }
        assert!(p.visible_text().iter().any(|t| t == "Radio Los Santos"));
        // SKATEV_HUD_PNG_DIR: chyron.png with the Radio Los Santos logo.
        let logo = std::fs::read(Assets::chyron_dir_for(&root).join("logos/radio_03_hiphop_new.rgba"));
        if let (Some(dir), Ok(rgba)) = (std::env::var_os("SKATEV_HUD_PNG_DIR"), logo) {
            let l = Layout::compute(1920, 1080, 1.0, crate::layout::DEFAULT_MAX_ASPECT);
            let mut textures: Vec<_> = a.textures.iter().map(crate::raster::LinearTexture::from).collect();
            let t = crate::player::Texture { name: "logo".into(), width: 128, height: 128, rgba };
            textures.push(crate::raster::LinearTexture::from(&t));
            let mut f = p.frame(&a, &l).unwrap();
            replace_logo(&mut f, logo_texture(&a).unwrap(), textures.len() as u32 - 1);
            let mut img = crate::raster::Image::new(1920, 1080, [0.3, 0.35, 0.3, 1.0]);
            crate::raster::composite(&mut img, &crate::raster::render_target(&f, &textures));
            let file = std::fs::File::create(std::path::Path::new(&dir).join("chyron.png")).unwrap();
            let mut e = png::Encoder::new(std::io::BufWriter::new(file), 1920, 1080);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&img.to_rgba8()).unwrap();
        }
        p.hide().unwrap();
    }
}


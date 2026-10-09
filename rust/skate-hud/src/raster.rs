//! CPU reference of the HUD's GPU path, for headless tests and previews.
//! Mirrors the donor's `scoring_hud` pipeline (and SkateV's D3D11 overlay):
//!
//! 1. an offscreen sRGB target cleared to transparent black;
//! 2. per draw, bilinear clamp sampling of an sRGB texture (filtered in
//!    linear), `clamp(texel * multiply + add, 0, 1)` (`hud_render.wgsl`),
//!    straight-alpha blending (colour: SrcAlpha / InvSrcAlpha, alpha: One /
//!    InvSrcAlpha), done in linear and stored sRGB-encoded;
//! 3. a premultiplied-alpha composite of that target over the frame
//!    (`hud_composite.wgsl` + PREMULTIPLIED_ALPHA_BLENDING), in linear.
use crate::player::{Frame, Texture};

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Linear RGBA image (premultiplied when used as the HUD target).
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

impl Image {
    pub fn new(width: u32, height: u32, fill: [f32; 4]) -> Self {
        Self {
            width,
            height,
            pixels: vec![fill; width as usize * height as usize],
        }
    }

    /// 8-bit sRGB RGBA bytes (alpha linear).
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for p in &self.pixels {
            for c in &p[..3] {
                out.push((linear_to_srgb(*c) * 255.0 + 0.5) as u8);
            }
            out.push((p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
        out
    }
}

/// Linear texels of an sRGB RGBA8 texture.
pub struct LinearTexture {
    pub width: u32,
    pub height: u32,
    texels: Vec<[f32; 4]>,
}

impl LinearTexture {
    pub fn from(t: &Texture) -> Self {
        let texels = t
            .rgba
            .as_chunks::<4>().0.iter()
            .map(|p| {
                [
                    srgb_to_linear(p[0] as f32 / 255.0),
                    srgb_to_linear(p[1] as f32 / 255.0),
                    srgb_to_linear(p[2] as f32 / 255.0),
                    p[3] as f32 / 255.0,
                ]
            })
            .collect();
        Self {
            width: t.width,
            height: t.height,
            texels,
        }
    }

    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let x = x.clamp(0, self.width as i64 - 1) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;
        self.texels[y * self.width as usize + x]
    }

    /// Bilinear, clamp to edge (Bevy's default linear sampler).
    pub fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = u * self.width as f32 - 0.5;
        let y = v * self.height as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.texel(x0, y0);
        let b = self.texel(x0 + 1, y0);
        let c = self.texel(x0, y0 + 1);
        let d = self.texel(x0 + 1, y0 + 1);
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            top + (bottom - top) * fy
        })
    }
}

fn quantize_srgb(p: [f32; 4]) -> [f32; 4] {
    let q = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() / 255.0;
    [
        srgb_to_linear(q(linear_to_srgb(p[0]))),
        srgb_to_linear(q(linear_to_srgb(p[1]))),
        srgb_to_linear(q(linear_to_srgb(p[2]))),
        q(p[3]),
    ]
}

/// Renders `frame` into a fresh transparent target the size of its layout.
pub fn render_target(frame: &Frame, textures: &[LinearTexture]) -> Image {
    let layout = frame.layout.expect("frame without layout");
    let mut target = Image::new(layout.width, layout.height, [0.0; 4]);
    for d in &frame.draws {
        let Some(tex) = textures.get(d.texture as usize) else {
            continue;
        };
        let verts = &frame.vertices[d.first as usize..(d.first + d.count) as usize];
        for tri in verts.as_chunks::<3>().0 {
            raster_triangle(&mut target, tri, |u, v| {
                let t = tex.sample(u, v);
                std::array::from_fn(|i| (t[i] * d.multiply[i] + d.add[i]).clamp(0.0, 1.0))
            });
        }
    }
    target
}

fn raster_triangle(target: &mut Image, tri: &[crate::player::Vertex], shade: impl Fn(f32, f32) -> [f32; 4]) {
    let (a, b, c) = (tri[0], tri[1], tri[2]);
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if area.abs() < 1e-12 || !area.is_finite() {
        return;
    }
    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i64;
    let max_x = a.x.max(b.x).max(c.x).ceil().min(target.width as f32) as i64;
    let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as i64;
    let max_y = a.y.max(b.y).max(c.y).ceil().min(target.height as f32) as i64;
    for py in min_y..max_y {
        for px in min_x..max_x {
            // Pixel centre, nudged off exact ties so a pixel on an edge shared
            // by two triangles is covered once (a stand-in for the GPU's
            // top-left rule).
            let (x, y) = (px as f32 + 0.5 + 1.0 / 1531.0, py as f32 + 0.5 + 1.0 / 2287.0);
            let w0 = ((b.x - x) * (c.y - y) - (b.y - y) * (c.x - x)) / area;
            let w1 = ((c.x - x) * (a.y - y) - (c.y - y) * (a.x - x)) / area;
            let w2 = 1.0 - w0 - w1;
            // Both windings (no culling).
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let u = w0 * a.u + w1 * b.u + w2 * c.u;
            let v = w0 * a.v + w1 * b.v + w2 * c.v;
            let s = shade(u, v);
            let dst = &mut target.pixels[py as usize * target.width as usize + px as usize];
            let out = [
                s[0] * s[3] + dst[0] * (1.0 - s[3]),
                s[1] * s[3] + dst[1] * (1.0 - s[3]),
                s[2] * s[3] + dst[2] * (1.0 - s[3]),
                s[3] + dst[3] * (1.0 - s[3]),
            ];
            *dst = quantize_srgb(out);
        }
    }
}

/// Premultiplied composite of the HUD target over `background` (linear).
pub fn composite(background: &mut Image, hud: &Image) {
    for (dst, src) in background.pixels.iter_mut().zip(&hud.pixels) {
        for i in 0..3 {
            dst[i] = src[i] + dst[i] * (1.0 - src[3]);
        }
        dst[3] = src[3] + dst[3] * (1.0 - src[3]);
    }
}

/// Bounding box (pixels) of target pixels with coverage above `alpha`.
pub fn coverage_bounds(image: &Image, alpha: f32) -> Option<[u32; 4]> {
    let mut b: Option<[u32; 4]> = None;
    for y in 0..image.height {
        for x in 0..image.width {
            if image.pixels[(y * image.width + x) as usize][3] > alpha {
                let r = b.get_or_insert([x, y, x, y]);
                r[0] = r[0].min(x);
                r[1] = r[1].min(y);
                r[2] = r[2].max(x);
                r[3] = r[3].max(y);
            }
        }
    }
    b
}

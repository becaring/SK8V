//! A board on its own in 3D, as Skate 3's board customiser shows it: the
//! deck, trucks and wheels with their finishes, floating and slowly turning
//! over so the graphic underneath and the grip on top both come round. It
//! is drawn in software into an image, so any menu that shows an image can
//! show it.

use crate::{Board, Texture};

type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 1e-12 { scale(a, 1. / l) } else { [0., 0., 1.] }
}

/// A column-major 4x4 matrix (glTF's order).
type M4 = [f32; 16];

fn transform(m: &M4, p: V3, w: f32) -> V3 {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12] * w,
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13] * w,
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14] * w,
    ]
}

fn multiply(a: &M4, b: &M4) -> M4 {
    let mut out = [0.; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

/// The inverse of an affine matrix.
fn invert(m: &M4) -> Option<M4> {
    let (a, b, c) = ([m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]);
    let det = dot(a, cross(b, c));
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    // Rows of the inverse are the cross products over the determinant.
    let r0 = scale(cross(b, c), 1. / det);
    let r1 = scale(cross(c, a), 1. / det);
    let r2 = scale(cross(a, b), 1. / det);
    let t = [m[12], m[13], m[14]];
    let it = [-dot(r0, t), -dot(r1, t), -dot(r2, t)];
    Some([
        r0[0], r1[0], r2[0], 0., //
        r0[1], r1[1], r2[1], 0., //
        r0[2], r1[2], r2[2], 0., //
        it[0], it[1], it[2], 1.,
    ])
}

#[derive(Clone, Debug)]
struct Tri {
    p: [V3; 3],
    uv: [[f32; 2]; 3],
    n: [V3; 3],
    texture: usize,
}

/// A board (or a skater holding one) ready to draw.
#[derive(Clone, Debug)]
pub struct Preview {
    tris: Vec<Tri>,
    textures: Vec<Texture>,
    /// Standing up and turning about the vertical (a skater), rather than
    /// lying along x and turning over (a board).
    upright: bool,
}

/// Skate's space (y up) as the preview's (z up).
fn upright_axes(p: V3) -> V3 {
    [p[0], -p[2], p[1]]
}

impl Preview {
    /// The board, posed: each part placed by its bones as the `reference`
    /// board's (the default skater's) bind pose has them, so parts from
    /// different models line up; then centred, laid along x with the deck's
    /// face up z, and scaled to two units long.
    pub fn new(board: &Board, reference: Option<&Board>) -> Option<Self> {
        let pose: Vec<M4> = board
            .joints
            .iter()
            .map(|joint| {
                let world = reference
                    .and_then(|r| r.joints.iter().find(|j| j.name == joint.name))
                    .and_then(|j| invert(&j.inverse_bind));
                match world {
                    Some(world) => multiply(&world, &joint.inverse_bind),
                    None => IDENTITY,
                }
            })
            .collect();
        let skin = |v: &crate::Vertex, w: f32, value: V3| -> V3 {
            let mut out = [0.; 3];
            for k in 0..4 {
                if v.weights[k] > 0. {
                    let m = pose.get(v.joints[k] as usize).unwrap_or(&IDENTITY);
                    out = add(out, scale(transform(m, value, w), v.weights[k]));
                }
            }
            out
        };
        let mut tris = Vec::new();
        for surface in &board.surfaces {
            for t in surface.indices.chunks_exact(3) {
                let Some(v) = [t[0], t[1], t[2]].map(|i| surface.vertices.get(i as usize)).into_iter().collect::<Option<Vec<_>>>() else {
                    continue;
                };
                let v = [v[0], v[1], v[2]];
                tris.push(Tri {
                    p: v.map(|v| skin(v, 1., v.position)),
                    uv: v.map(|v| v.uv),
                    n: v.map(|v| normalize(skin(v, 0., v.normal))),
                    texture: surface.texture,
                });
            }
        }
        if tris.is_empty() {
            return None;
        }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in tris.iter().flat_map(|t| t.p) {
            if p.iter().any(|v| !v.is_finite()) {
                return None;
            }
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let size = sub(hi, lo);
        let centre = scale(add(hi, lo), 0.5);
        // Long axis along x, the thinnest (the deck's face) up z.
        let mut axes = [0, 1, 2];
        axes.sort_by(|a, b| size[*b].total_cmp(&size[*a]));
        let [long, wide, thin] = axes;
        let unit = 2. / size[long].max(1e-6);
        let place = |p: V3, w: f32| -> V3 {
            let p = if w > 0. { sub(p, centre) } else { p };
            let s = if w > 0. { unit } else { 1. };
            [p[long] * s, p[wide] * s, p[thin] * s]
        };
        // A swap of two axes mirrors the board: undo it with y.
        let mirrored = {
            let e = |i: usize| {
                let mut v = [0.; 3];
                v[i] = 1.;
                v
            };
            dot(cross(e(long), e(wide)), e(thin)) < 0.
        };
        for t in &mut tris {
            t.p = t.p.map(|p| {
                let mut q = place(p, 1.);
                if mirrored {
                    q[1] = -q[1];
                }
                q
            });
            t.n = t.n.map(|n| {
                let mut q = place(n, 0.);
                if mirrored {
                    q[1] = -q[1];
                }
                q
            });
        }
        Some(Self { tris, textures: board.textures.clone(), upright: false })
    }

    /// Whether this is a skater standing (turning about the vertical)
    /// rather than a board on its own.
    pub fn upright(&self) -> bool {
        self.upright
    }

    /// Meshes posed by `world` (a joint's world transform in Skate's space,
    /// by name; skinned as `world * inverse_bind`), standing up: a skater
    /// holding their board, as Skate's customiser shows them. Centred,
    /// scaled to two units tall.
    pub fn posed(meshes: &[&Board], world: &dyn Fn(&str) -> Option<[f32; 16]>) -> Option<Self> {
        let mut tris = Vec::new();
        let mut textures = Vec::new();
        for board in meshes {
            let pose: Vec<M4> = board
                .joints
                .iter()
                .map(|j| world(&j.name).map_or(IDENTITY, |w| multiply(&w, &j.inverse_bind)))
                .collect();
            let skin = |v: &crate::Vertex, w: f32, value: V3| -> V3 {
                let mut out = [0.; 3];
                for k in 0..4 {
                    if v.weights[k] > 0. {
                        let m = pose.get(v.joints[k] as usize).unwrap_or(&IDENTITY);
                        out = add(out, scale(transform(m, value, w), v.weights[k]));
                    }
                }
                out
            };
            let base = textures.len();
            textures.extend(board.textures.iter().cloned());
            for surface in &board.surfaces {
                for t in surface.indices.chunks_exact(3) {
                    let Some(v) = [t[0], t[1], t[2]].map(|i| surface.vertices.get(i as usize)).into_iter().collect::<Option<Vec<_>>>() else {
                        continue;
                    };
                    tris.push(Tri {
                        p: [v[0], v[1], v[2]].map(|v| upright_axes(skin(v, 1., v.position))),
                        uv: [v[0].uv, v[1].uv, v[2].uv],
                        n: [v[0], v[1], v[2]].map(|v| normalize(upright_axes(skin(v, 0., v.normal)))),
                        texture: base + surface.texture,
                    });
                }
            }
        }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in tris.iter().flat_map(|t| t.p) {
            if p.iter().any(|v| !v.is_finite()) {
                return None;
            }
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if tris.is_empty() {
            return None;
        }
        let centre = scale(add(hi, lo), 0.5);
        let unit = 2. / (hi[2] - lo[2]).max(1e-6);
        for t in &mut tris {
            t.p = t.p.map(|p| scale(sub(p, centre), unit));
        }
        Some(Self { tris, textures, upright: true })
    }

    /// The board drawn `width` by `height`, turned `angle` radians over
    /// about its length (0 shows its top face, pi the underside), on a
    /// clear background.
    pub fn render(&self, angle: f32, width: u32, height: u32) -> Texture {
        // Drawn at twice the size and averaged down, for smooth edges.
        let (w, h) = (width.max(1) as usize * 2, height.max(1) as usize * 2);
        let mut color = vec![[0f32; 4]; w * h];
        let mut depth = vec![f32::INFINITY; w * h];
        // Rolls over about its length, seen a little from the side and
        // from above.
        // A board: at 0 the deck's face turns to the camera, and it rolls.
        // A skater: they turn about the vertical, facing the camera at 0.
        let upright = self.upright;
        let (sr, cr) = if upright { (0., 1.) } else { (angle + std::f32::consts::FRAC_PI_2).sin_cos() };
        let (sy, cy) = if upright { (angle + std::f32::consts::PI).sin_cos() } else { (0.35f32).sin_cos() };
        let (sp, cp) = (if upright { 0.12f32 } else { 0.3 }).sin_cos();
        let view = |p: V3| -> V3 {
            // Roll about x.
            let p = [p[0], p[1] * cr - p[2] * sr, p[1] * sr + p[2] * cr];
            // Yaw about the up axis (z), then pitch towards the viewer.
            let p = [p[0] * cy - p[1] * sy, p[0] * sy + p[1] * cy, p[2]];
            [p[0], p[1] * cp - p[2] * sp, p[1] * sp + p[2] * cp]
        };
        // Camera on -y looking along +y, z up.
        const DISTANCE: f32 = 4.;
        let focal = if upright {
            // Two units tall, fitted to the height.
            0.85 * h as f32 * DISTANCE / 2.2
        } else {
            (0.9 * w as f32 * DISTANCE / 2.2).min(1.6 * h as f32 * DISTANCE / 0.9)
        };
        // A studio: a key light up and to the left of the camera, a softer
        // fill from the right, a rim from behind and above, and an even
        // glow all round, so dark finishes still read.
        let lights = [
            (normalize([-0.45, -0.75, 0.55]), 0.7f32),
            (normalize([0.7, -0.55, 0.05]), 0.35),
            (normalize([0.1, 0.75, 0.65]), 0.3),
        ];
        let key = lights[0].0;
        const AMBIENT: f32 = 0.42;
        let eye = [0., -DISTANCE, 0.];
        let project = |p: V3| -> (f32, f32, f32) {
            let z = p[1] + DISTANCE;
            (w as f32 * 0.5 + focal * p[0] / z, h as f32 * 0.5 - focal * p[2] / z, z)
        };
        for tri in &self.tris {
            let v = tri.p.map(view);
            let n = tri.n.map(view);
            let s = v.map(project);
            if s.iter().any(|(_, _, z)| *z < 0.1) {
                continue;
            }
            let area = (s[1].0 - s[0].0) * (s[2].1 - s[0].1) - (s[2].0 - s[0].0) * (s[1].1 - s[0].1);
            if area.abs() < 1e-6 {
                continue;
            }
            let min_x = s.iter().map(|p| p.0).fold(f32::INFINITY, f32::min).floor().max(0.) as usize;
            let max_x = s.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max).ceil().min(w as f32 - 1.) as usize;
            let min_y = s.iter().map(|p| p.1).fold(f32::INFINITY, f32::min).floor().max(0.) as usize;
            let max_y = s.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max).ceil().min(h as f32 - 1.) as usize;
            if min_x > max_x || min_y > max_y {
                continue;
            }
            let texture = self.textures.get(tri.texture);
            let inv_z = s.map(|p| 1. / p.2);
            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let edge = |a: usize, b: usize| (s[b].0 - s[a].0) * (py - s[a].1) - (s[b].1 - s[a].1) * (px - s[a].0);
                    let mut b = [edge(1, 2) / area, edge(2, 0) / area, edge(0, 1) / area];
                    if b.iter().any(|v| *v < -1e-4) {
                        continue;
                    }
                    // Perspective-correct weights.
                    let wz = [b[0] * inv_z[0], b[1] * inv_z[1], b[2] * inv_z[2]];
                    let sum = wz[0] + wz[1] + wz[2];
                    let z = 1. / sum;
                    let at = y * w + x;
                    if z >= depth[at] {
                        continue;
                    }
                    for k in 0..3 {
                        b[k] = wz[k] / sum;
                    }
                    let uv = [
                        b[0] * tri.uv[0][0] + b[1] * tri.uv[1][0] + b[2] * tri.uv[2][0],
                        b[0] * tri.uv[0][1] + b[1] * tri.uv[1][1] + b[2] * tri.uv[2][1],
                    ];
                    let texel = texture.map_or([200, 200, 200, 255], |t| t.sample(uv));
                    if texel[3] < 24 {
                        continue;
                    }
                    let mut normal = normalize(add(add(scale(n[0], b[0]), scale(n[1], b[1])), scale(n[2], b[2])));
                    let point = add(add(scale(v[0], b[0]), scale(v[1], b[1])), scale(v[2], b[2]));
                    let to_eye = normalize(sub(eye, point));
                    // A deck is thin: the side seen is the side lit.
                    if dot(normal, to_eye) < 0. {
                        normal = scale(normal, -1.);
                    }
                    let diffuse = AMBIENT + lights.iter().map(|(dir, power)| power * dot(normal, *dir).max(0.)).sum::<f32>();
                    // A soft shine off the key light, as off lacquer.
                    let half = normalize(add(key, to_eye));
                    let shine = 60. * dot(normal, half).max(0.).powf(28.);
                    depth[at] = z;
                    let lit = |c: u8| {
                        let x = f32::from(c) * diffuse + shine;
                        // Rolled off, not clipped, past white.
                        if x <= 200. { x } else { 255. - 55. * (-(x - 200.) / 55.).exp() }
                    };
                    color[at] = [lit(texel[0]), lit(texel[1]), lit(texel[2]), 255.];
                }
            }
        }
        let (out_w, out_h) = (w / 2, h / 2);
        let mut rgba = Vec::with_capacity(out_w * out_h * 4);
        for y in 0..out_h {
            for x in 0..out_w {
                let mut sum = [0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let c = color[(y * 2 + dy) * w + x * 2 + dx];
                    // Premultiplied, so clear samples darken nothing.
                    for k in 0..4 {
                        sum[k] += c[k];
                    }
                }
                let alpha = sum[3] / 4.;
                let unmultiply = if alpha > 0. { 255. / sum[3] } else { 0. };
                rgba.extend([
                    (sum[0] * unmultiply).round().clamp(0., 255.) as u8,
                    (sum[1] * unmultiply).round().clamp(0., 255.) as u8,
                    (sum[2] * unmultiply).round().clamp(0., 255.) as u8,
                    alpha.round().clamp(0., 255.) as u8,
                ]);
            }
        }
        Texture { width: out_w as u32, height: out_h as u32, rgba }
    }
}

const IDENTITY: M4 = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

/// Seconds for one whole turn of the board.
pub const TURN_SECONDS: f32 = 9.;

/// The turn at `seconds` since the preview opened: the underside (the
/// graphic, trucks and wheels) faces the viewer and the board swings a
/// little either way, turning back before any of it goes out of view.
pub fn angle_at(seconds: f32) -> f32 {
    std::f32::consts::PI + (seconds * std::f32::consts::TAU / TURN_SECONDS).sin() * 0.6
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Joint, Part, Surface, Vertex};

    /// A flat board, 0.8 long (x) and 0.2 wide (z), facing y, coloured
    /// red on the left half of its texture and blue on the right.
    fn board(offset: f32) -> Board {
        let v = |x: f32, z: f32, u: f32| Vertex {
            position: [x + offset, 0., z],
            normal: [0., 1., 0.],
            uv: [u, 0.5],
            joints: [0; 4],
            weights: [1., 0., 0., 0.],
        };
        let mut inverse_bind = IDENTITY;
        inverse_bind[12] = -offset;
        Board {
            joints: vec![Joint { name: "DECK".into(), inverse_bind }],
            surfaces: vec![Surface {
                part: Part::Deck,
                material: "deck".into(),
                texture: 0,
                vertices: vec![v(-0.4, -0.1, 0.), v(0.4, -0.1, 1.), v(0.4, 0.1, 1.), v(-0.4, 0.1, 0.)],
                indices: vec![0, 1, 2, 0, 2, 3],
            }],
            textures: vec![Texture { width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 0, 255, 255] }],
        }
    }

    #[test]
    fn the_board_turns_and_shows_its_finish() {
        let preview = Preview::new(&board(0.), None).unwrap();
        let image = preview.render(0., 96, 48);
        assert_eq!((image.width, image.height), (96, 48));
        let px = |x: usize, y: usize| &image.rgba[(y * 96 + x) * 4..(y * 96 + x) * 4 + 4];
        assert_eq!(px(0, 0)[3], 0, "clear around the board");
        let covered = image.rgba.chunks(4).filter(|p| p[3] == 255).count();
        assert!(covered > 96 * 48 / 20, "the face is seen: {covered}");
        // Left of centre is red, right of centre blue (the finish's halves).
        let row: Vec<&[u8]> = (0..96).map(|x| px(x, 24)).filter(|p| p[3] == 255).collect();
        assert!(row.first().unwrap()[0] > row.first().unwrap()[2]);
        assert!(row.last().unwrap()[2] > row.last().unwrap()[0]);
        // Edge on, a flat board is a line.
        let edge = preview.render(std::f32::consts::FRAC_PI_2, 96, 48);
        let seen = edge.rgba.chunks(4).filter(|p| p[3] > 0).count();
        assert!(seen < covered / 2, "{seen} vs {covered}");
    }

    #[test]
    fn parts_line_up_by_the_reference_pose() {
        // The same deck modelled 5 units off, its bone saying so.
        let shifted = Preview::new(&board(5.), Some(&board(0.))).unwrap();
        let plain = Preview::new(&board(0.), None).unwrap();
        assert_eq!(shifted.render(1., 40, 20).rgba, plain.render(1., 40, 20).rgba);
        assert!(Preview::new(&Board::default(), None).is_none());
    }

    #[test]
    fn a_posed_skater_stands_up() {
        // A "skater": the flat test board stood on its end by its bone.
        let mut skater = board(0.);
        skater.joints[0].name = "HIPS".into();
        let stand: M4 = [1., 0., 0., 0., 0., 0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1.];
        let world = |name: &str| (name == "HIPS").then_some(stand);
        let preview = Preview::posed(&[&skater, &board(0.)], &world).unwrap();
        assert!(preview.upright);
        let image = preview.render(0., 64, 64);
        assert!(image.rgba.chunks(4).any(|p| p[3] == 255));
        assert!(Preview::posed(&[], &world).is_none());
    }

    #[test]
    fn affine_inverse() {
        let m: M4 = [2., 0., 0., 0., 0., 0., 1., 0., 0., -1., 0., 0., 3., 4., 5., 1.];
        let i = invert(&m).unwrap();
        let p = [0.3, -2., 7.];
        let back = transform(&i, transform(&m, p, 1.), 1.);
        assert!(back.iter().zip(p).all(|(a, b)| (a - b).abs() < 1e-5));
    }
}

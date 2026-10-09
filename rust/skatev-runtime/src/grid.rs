//! XY bucket grid of items by the cells their XY bounds touch, with the
//! barycentric height tests its users run on the triangles found there.
use bevy_math::Vec3;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct XyGrid {
    cell: f32,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl XyGrid {
    pub fn new(cell: f32) -> Self {
        Self { cell, cells: HashMap::new() }
    }

    /// Every triangle of `tris`, by index.
    pub fn triangles(cell: f32, tris: &[[Vec3; 3]]) -> Self {
        let mut g = Self::new(cell);
        for (i, t) in tris.iter().enumerate() {
            g.insert(i as u32, t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]));
        }
        g
    }

    pub fn cell_of(&self, v: f32) -> i32 {
        (v / self.cell).floor() as i32
    }

    /// Adds item `id` to every cell of the XY box `lo..hi` (z is ignored).
    pub fn insert(&mut self, id: u32, lo: Vec3, hi: Vec3) {
        let (x0, x1, y0, y1) = (self.cell_of(lo.x), self.cell_of(hi.x), self.cell_of(lo.y), self.cell_of(hi.y));
        for x in x0..=x1 {
            for y in y0..=y1 {
                self.cells.entry((x, y)).or_default().push(id);
            }
        }
    }

    /// The items of cell (`x`, `y`), in insertion order.
    pub fn cell(&self, x: i32, y: i32) -> &[u32] {
        self.cells.get(&(x, y)).map_or(&[], Vec::as_slice)
    }

    /// The items of the cell holding point (`x`, `y`).
    pub fn at(&self, x: f32, y: f32) -> &[u32] {
        self.cell(self.cell_of(x), self.cell_of(y))
    }
}

/// Barycentric weights of XY point (`x`, `y`) in triangle `t`, measured from
/// corner c; `None` for a footprint thinner than `eps` or a point more than
/// `tol` outside.
pub fn weights(t: &[[f32; 3]; 3], x: f32, y: f32, eps: f32, tol: f32) -> Option<[f32; 3]> {
    let [a, b, c] = t;
    let den = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if den.abs() < eps {
        return None;
    }
    let l0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / den;
    let l1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / den;
    let l2 = 1.0 - l0 - l1;
    (l0 >= -tol && l1 >= -tol && l2 >= -tol).then_some([l0, l1, l2])
}

/// Height of triangle `t` over XY point (`x`, `y`), measured from corner a
/// (1e-4 tolerance outside it); `None` outside or for a vertical face.
pub fn height(t: &[Vec3; 3], x: f32, y: f32) -> Option<f32> {
    let [a, b, c] = *t;
    let d = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
    if d.abs() < 1e-12 {
        return None;
    }
    let u = ((x - a.x) * (c.y - a.y) - (c.x - a.x) * (y - a.y)) / d;
    let v = ((b.x - a.x) * (y - a.y) - (x - a.x) * (b.y - a.y)) / d;
    if u < -1e-4 || v < -1e-4 || u + v > 1.0 + 1e-4 {
        return None;
    }
    Some(a.z + u * (b.z - a.z) + v * (c.z - a.z))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_land_in_every_cell_their_bounds_touch() {
        let tri = [Vec3::new(0.5, 0.5, 0.0), Vec3::new(2.5, 0.5, 0.0), Vec3::new(0.5, 1.5, 3.0)];
        let g = XyGrid::triangles(2.0, &[tri]);
        assert_eq!(g.at(0.1, 0.1), &[0]);
        assert_eq!(g.at(3.9, 1.0), &[0]);
        assert!(g.at(4.1, 1.0).is_empty() && g.at(-0.1, 1.0).is_empty());
        assert_eq!(height(&tri, 0.5, 1.0), Some(1.5));
        assert_eq!(height(&tri, 3.0, 1.0), None);
        let w = weights(&tri.map(|v| v.to_array()), 0.5, 1.0, 1e-12, 1e-5).unwrap();
        assert!((w[0] - 0.5).abs() < 1e-6 && w[1].abs() < 1e-6 && (w[2] - 0.5).abs() < 1e-6);
    }
}

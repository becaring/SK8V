//! GTA Legacy collision material -> authored Skate audio surface/pattern.
//! IDs were checked against the owner's common.rpf materials.dat and Skate's
//! AudioSurfaceMap (C489459A0C07D154/4CA607558B1CF440). These are integration
//! choices between games, not a claim that GTA and Skate share material IDs.
//! The physics surface remains unchanged; this mapping selects sound only.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surface {
    /// Native collision audio ID; audio consumers subtract one for table index.
    pub audio: u8,
    /// Native authored seam pattern. Spacing comes from owned Skate tuning.
    pub seam: u8,
    pub name: &'static str,
    pub fallback: bool,
}
impl Surface {
    pub fn packed(self) -> u32 { u32::from(self.audio) | (u32::from(self.seam) << 12) }
}

pub fn surface(material: u8) -> Surface {
    // GTA indexes are the ordered material definitions in materials.dat.
    // Skate values refer to complete authored rows, keeping rolling/grind/
    // landing/skid/foot/body selections together. Unknowns retain native0.
    let (row,seam,name) = match material {
        // 120..=124 CAR_GLASS_*: glass plays smooth mineral (DECISIONS); was
        // the unknown fallback row.
        1 | 14 | 25 | 81 | 112..=115 | 120..=124 => (2,0,"smooth mineral"),
        2 | 3 | 8 | 9 | 11 | 16 | 17 | 101 | 102 | 179 => (3,0,"rough mineral"),
        4 | 5 | 26 | 30 => (0,0,"smooth asphalt"),
        6 => (1,1,"rough asphalt"),
        7 => (3,10,"rumble strip"),
        12 => (3,7,"cobblestone"),
        13 | 175 => (3,12,"brick"),
        15 | 174 => (2,11,"pavement slabs"),
        10 | 46..=52 | 180 => (9,0,"vegetation"),
        18..=24 | 27..=29 | 31..=45 => (7,0,"loose ground"),
        53 | 54 | 69..=80 | 96 | 111 | 177 | 178 => (6,0,"wood"),
        55..=63 | 65..=67 | 116 | 133..=136 | 142 | 144..=146 | 172 | 181 => (8,0,"metal"),
        64 => (67,0,"metal grille"),
        68 => (66,0,"manhole"),
        82 => (2,12,"roof tile"),
        84 | 86..=92 | 117..=119 | 132 => (47,0,"plastic"),
        _ => return Surface { audio:0, seam:0, name:"unknown", fallback:true },
    };
    Surface { audio:row+1, seam, name, fallback:false }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrelated_materials_select_distinct_authored_rows() {
        let ids=[1,4,55,69,13,68].map(|id|surface(id).packed());
        let distinct:std::collections::BTreeSet<_>=ids.into_iter().collect();
        assert_eq!(distinct.len(),ids.len());
        assert_ne!(surface(1),surface(2));
        assert_ne!(surface(4),surface(6));
        assert_eq!(surface(174).seam,11);
        assert_eq!(surface(175).seam,12);
    }
    #[test]
    fn every_byte_is_safe_and_unknown_materials_are_explicit() {
        for id in 0..=255 {
            let s=surface(id);let p=s.packed();
            assert_eq!((p>>7)&31,0,"audio mapping changed physics on {id}");
            assert!(s.audio<=95 && s.seam<=15);
            assert_eq!(p & 0xffff_0000,0,"collided with entity/grind flags");
        }
        for id in [0,125,143,182,255] {assert!(surface(id).fallback);assert_eq!(surface(id).packed(),0);}
        for id in [118,119,120,124,142] {assert!(!surface(id).fallback, "car material {id}");}
    }
}

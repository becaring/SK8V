//! Game-side parameter inputs, from the TU3 instructions. These are sparse setters: untouched banks/slots retain state.
use super::{components::helpers, driver::Component, frame::SkaterImage, tuning::Tuning};

const NEGATIVE_SLIP: u64 = 0x57A7_8D3B_E8D4_7BB3;
const POSITIVE_SLIP: u64 = 0x8DD4_C3FC_8DAF_4059;
const DELTA_CLASS: u64 = 0xC183_1BDB_6CB1_B1EA;
const DELTA_COLLECTION: u64 = 0x1C20_B475_CE21_8459;
const DELTA_MAX: u64 = 0x780F_5C81_6E00_BDFC;
const DELTA_SLEW: u64 = 0x6875_6CFE_B1FF_3428;

// PPC fsel differs from min/max for NaN: an unordered selector takes
// the negative branch. The original's two selects therefore map NaN to one.
fn unit_fraction(x: f32) -> f32 {
    let lower = if -x >= 0. { 0. } else { x };
    if 1. - lower >= 0. { lower } else { 1. }
}
fn packed(x: f32) -> i32 {
    // fctiwz returns integer indefinite on NaN; the following signed clamp
    // maps it to zero. Rust's positive-overflow saturation also clamps to 32767.
    if x.is_nan() {
        0
    } else {
        ((x * 32767.) as i32).clamp(0, 32767)
    }
}
fn attrib(t: &Tuning, c: u32, key: u64) -> f32 {
    f32::from_bits(t.g32(t.attrib(c, key)))
}

/// TU3 824CA738; call first, before surface lifecycle 824C5CA8.
pub fn slip(
    c: &mut Component,
    s: &SkaterImage,
    t: &Tuning,
    inputs: &mut [i32; 16],
) -> Result<(), String> {
    if c.image.len() < 1904 {
        return Err("board input component image is truncated".into());
    }
    let selected = c.c32(1500);
    if selected > 1 {
        return Err(format!("invalid audio selected wheel {selected}"));
    }
    // 824CA738: normalization uses the wheel's source tuning collection.
    c.setf(1508, 0.);
    c.setf(1512, 0.);
    let mut negative = 0.;
    let mut positive = 0.;
    if c.c32(1320 + selected as usize * 4) == 1 {
        let slip = s.rf(712);
        let collection = c.c32(184 + selected as usize * 16);
        if slip < 0. {
            negative = unit_fraction(slip / attrib(t, collection, NEGATIVE_SLIP));
        } else if slip > 0. {
            positive = unit_fraction(slip / attrib(t, collection, POSITIVE_SLIP));
        }
    }
    c.setf(1508, negative);
    c.setf(1512, positive);
    inputs[2] = packed(negative);
    inputs[3] = packed(positive);

    Ok(())
}
/// 824C5CB8: reset before the surface lifecycle loop.
pub fn begin_surfaces(inputs: &mut [i32; 16]) {
    inputs[0] = 0;
}
/// 824C5D28: call when a wheel's new surface differs from its prior surface,
/// before lifecycle changes the selected wheel or stores the new surface.
pub fn surface_changed(prior: u32, inputs: &mut [i32; 16]) {
    if prior != 14 {
        inputs[0] = 32767;
    }
}
/// 824C6144: call after the entire surface lifecycle loop, including wheel switches.
pub fn finish_surfaces(c: &mut Component, s: &SkaterImage, t: &Tuning, inputs: &mut [i32; 16]) {
    inputs[6] = if helpers(0x824C_82A8, &[0, 0], c, s, t).0 == 9 {
        32767
    } else {
        0
    };
}
/// 824C6198, before its wrapper lifecycle.
pub fn airborne(s: &SkaterImage, inputs: &mut [i32; 16]) {
    inputs[4] = if s.r8(333) != 0 || s.r8(334) != 0 {
        32767
    } else {
        0
    };
}
/// 824C7438, before skid wrapper lifecycle.
pub fn skid(c: &mut Component, s: &SkaterImage, t: &Tuning, inputs: &mut [i32; 16]) {
    inputs[1] = if helpers(0x824C_72F0, &[], c, s, t).0 != 0 {
        32767
    } else {
        0
    };
}
/// 824CBAC0; last in board prep 824C6A78. Current/previous integer fields
/// +1892/+1896 are maintained by board update 824C6BD8.
pub fn delta(c: &mut Component, t: &Tuning, dt: f32, inputs: &mut [i32; 16]) -> Result<(), String> {
    if !dt.is_finite() || dt <= 0. {
        return Err("audio input dt must be finite and positive".into());
    }
    // 824CBAC0: integer delta, cap, then a tuning-defined linear slew.
    let collection = t.collection(DELTA_CLASS, DELTA_COLLECTION);
    let max = attrib(t, collection, DELTA_MAX);
    let rate = attrib(t, collection, DELTA_SLEW);
    let delta = (c.c32(1892) as i32)
        .wrapping_sub(c.c32(1896) as i32)
        .wrapping_abs();
    let mut target = delta as f32 / dt;
    if target > max {
        target = max;
    }
    let previous = f32::from_bits(c.c32(1900));
    let step = rate * dt;
    if target > previous {
        if target - previous > step {
            target = previous + step;
        }
    } else if target < previous && previous - target > step {
        target = previous - step;
    }
    c.setf(1900, target);
    inputs[5] = packed(target / max);
    Ok(())
}

/// Isolated setter exercise. Live callers must interleave the public stages
/// with their native component lifecycle; this convenience does no lifecycle.
#[cfg(test)]
fn update_inputs(
    bank: u8,
    c: &mut Component,
    s: &SkaterImage,
    t: &Tuning,
    dt: f32,
    inputs: &mut [i32; 16],
) -> Result<(), String> {
    if bank != 0 {
        return Ok(());
    }
    slip(c, s, t, inputs)?;
    begin_surfaces(inputs);
    finish_surfaces(c, s, t, inputs);
    airborne(s, inputs);
    skid(c, s, t, inputs);
    delta(c, t, dt, inputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_inputs_and_native_slip_slew() {
        let mut c = Component::new("board", 2048, vec![]);
        let mut s = SkaterImage::default();
        let mut t = Tuning::default();
        let col = t.set_collection(DELTA_CLASS, DELTA_COLLECTION);
        t.set_attrib(col, DELTA_MAX, 100f32.to_bits());
        t.set_attrib(col, DELTA_SLEW, 10f32.to_bits());
        t.set_attrib(17, NEGATIVE_SLIP, (-2f32).to_bits());
        t.set_attrib(17, POSITIVE_SLIP, 4f32.to_bits());
        c.set32(184, 17);
        c.set32(1320, 1);
        c.set32(1892, 20);
        s.wf(712, -1.);
        s.w8(333, 1);
        let mut inputs = [91; 16];
        update_inputs(0, &mut c, &s, &t, 0.5, &mut inputs).unwrap();
        assert_eq!(&inputs[2..6], &[16383, 0, 32767, 1638]);
        assert_eq!(&inputs[7..], &[91; 9]);
        assert_eq!(f32::from_bits(c.c32(1900)), 5.);
        s.wf(712, 2.);
        c.set32(1892, 0);
        update_inputs(0, &mut c, &s, &t, 0.5, &mut inputs).unwrap();
        assert_eq!(&inputs[2..4], &[0, 16383]);
        assert_eq!(inputs[5], 0);
        let old = inputs;
        update_inputs(8, &mut c, &s, &t, 0., &mut inputs).unwrap();
        assert_eq!(old, inputs);
    }
    #[test]
    fn ordered_surface_stage_uses_final_selected_wheel() {
        let mut c = Component::new("board", 2048, vec![]);
        let mut s = SkaterImage::default();
        let mut t = Tuning::default();
        let a = t.set_tuned_record(64, 0x4CA6_0755_8B1C_F440, 7, 48);
        let b = t.set_tuned_record(64, 0x4CA6_0755_8B1C_F440, 8, 48);
        t.w32(a + 4, 3);
        t.w32(b + 4, 9);
        s.w32(620, 7);
        s.w32(632, 8);
        let mut inputs = [91; 16];
        begin_surfaces(&mut inputs);
        surface_changed(14, &mut inputs);
        assert_eq!(inputs[0], 0);
        surface_changed(3, &mut inputs);
        assert_eq!(inputs[0], 32767);
        finish_surfaces(&mut c, &s, &t, &mut inputs);
        assert_eq!(inputs[6], 0);
        c.set32(1500, 1); // lifecycle switched wheels before its final setter
        finish_surfaces(&mut c, &s, &t, &mut inputs);
        assert_eq!(inputs[6], 32767);
        assert_eq!(&inputs[7..], &[91; 9]);
    }
    #[test]
    fn ppc_clamp_unordered_and_saturation() {
        assert_eq!(unit_fraction(f32::NAN), 1.);
        assert_eq!(unit_fraction(f32::NEG_INFINITY), 0.);
        assert_eq!(unit_fraction(f32::INFINITY), 1.);
        assert_eq!(packed(f32::NAN), 0);
        assert_eq!(packed(f32::INFINITY), 32767);
    }
}

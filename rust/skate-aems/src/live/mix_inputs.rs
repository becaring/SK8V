//! Inputs shared by player audio banks. The settings are host-owned; speed
//! normalization and wheel-contact selectors are recovered TU3 behavior.
use super::{evaluator::Evaluator,frame::SkaterImage};
use crate::{glue::fsel,ops::fctiwz};

fn packed(value:f32)->i32 {
    let lower=fsel(-value,0.,value);
    fctiwz(fsel(32767.-lower,lower,32767.)).clamp(0,32767)
}

/// 824D5160: settings gains and the normal simulation rate. SkateV renders
/// dry gameplay only: GTA owns listener attenuation and the user master gain.
/// Skate's shell/menu/replay/environment switches therefore stay inactive.
pub fn settings(e:&mut Evaluator) {
    let mut values=[0;16];
    for value in &mut values[1..=4] {*value=packed(1.*32767.);}
    e.set_inputs(0x4000_0020,values);
}

/// 824B19C8: MAINPLAYER is a separate input handle from the board component.
/// Its kind-3 ID is not a positional-only handle. Constants below were read
/// from TU3 822F9520..822F952C, not fitted to captured mix outputs.
pub fn player(e:&mut Evaluator,s:&SkaterImage) {
    let mut values=e.inputs(0x6001_0000);
    for (slot,scale) in [(0,23592.23828125_f32),(1,3932.039794921875_f32),
        (7,2359.223876953125_f32),(8,1685.159912109375_f32)] {
        values[slot]=packed(s.rf(208)*scale);
    }
    values[14]=packed(s.rf(212)*1685.159912109375_f32);
    values[2]=if s.r32(200)==0 {32767}else{0};
    values[10]=match s.r32(200) {1=>8191,2=>16383,3=>24575,4=>32767,_=>0};
    for (slot,field) in [(4,336),(5,339),(6,343)] {values[slot]=if s.r8(field)!=0 {32767}else{0};}
    // Fill824B1550..158C: input 12 is 0x824B23C8, the local skater's +684
    // (soft wheels), compared with 1.
    values[12]=if s.r32(684)==1 {32767}else{0};
    // One local player, no Skate challenge/menu camera. Listener orientation
    // and distance (3/13) remain neutral on the dry path, spatialized by GTA.
    values[9]=0;values[11]=0;values[3]=0;values[13]=0;
    e.set_inputs(0x6001_0000,values);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speed_normalization_preserves_small_motion_and_saturates() {
        assert_eq!(packed(0.),0);
        assert_eq!(packed(0.5*23592.23828125_f32),11796);
        assert_eq!(packed(20.*1685.159912109375_f32),32767);
        assert_eq!(packed(-100.),0);
    }
}

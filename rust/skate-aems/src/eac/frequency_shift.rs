//! FrequencyShiftSsb's board configuration (824C8878 passes option 0).
//! TU3 82B22898: two cascaded all-pass pairs, quadrature modulation, and a
//! persistent oscillator. Option 1's unrelated antialias prefilter is absent
//! from this configuration. Constants are the game's instruction and data values.
use super::{iir2::{self,State}, BLOCK, lfs};
use crate::{ops::{fmadds,fnmsubs,fctiwz},splice::vcos};

const COEFS:[[u32;5];4]=[
    [0xBFF620C5,0x3F6C7469,0x3F6C7469,0xBFF620C5,0x3F800000],
    [0xBECDB811,0xBE5A43B1,0xBE5A43B1,0xBECDB811,0x3F800000],
    [0xBFFBDFCD,0x3F77C5D9,0x3F77C5D9,0xBFFBDFCD,0x3F800000],
    [0xBFA1A207,0x3EB1E001,0x3EB1E001,0xBFA1A207,0x3F800000],
];
const TAU:f32=f32::from_bits(0x40C90FDB);
const INV_TAU:f32=f32::from_bits(0x3E22F983);

/// 824531C8, one VMX lane. Range reduction rounds to nearest even.
pub fn vsin(x:f32)->f32 {
    const C:[u32;11]=[0xBE2AAAAB,0x3C088889,0xB9500D01,0x3638EF1D,
        0xB2D7322B,0x2F309231,0xAB573F9F,0x274A963C,0xA317A4DA,0x1EB8DC78,0x9A3B0DA1];
    let mul=|a:f32,b:f32|lfs(lfs(a)*lfs(b));
    let r=lfs(lfs(x)-mul(TAU,mul(x,INV_TAU).round_ties_even()));
    let square=mul(r,r);
    let mut power=mul(square,r);
    let mut sum=r;
    for (i,bits) in C.into_iter().enumerate() {
        sum=lfs(mul(f32::from_bits(bits),power)+lfs(sum));
        if i!=10 {power=mul(power,square);}
    }
    sum
}

#[derive(Clone,Debug,Default)]
pub struct FrequencyShift {
    pub hz:f32,
    pub phase:f32,
    pub state:[State;4],
}
impl FrequencyShift {
    pub fn process(&mut self,rate:f32,samples:&mut [f32;BLOCK]) {
        let mut a=*samples;let mut b=*samples;
        iir2::biquad(&mut self.state[0],&COEFS[0].map(f32::from_bits),&mut a);
        iir2::biquad(&mut self.state[1],&COEFS[1].map(f32::from_bits),&mut a);
        iir2::biquad(&mut self.state[2],&COEFS[2].map(f32::from_bits),&mut b);
        iir2::biquad(&mut self.state[3],&COEFS[3].map(f32::from_bits),&mut b);
        let step=self.hz/rate*TAU;
        let mut phase=[self.phase,self.phase+step,fmadds(step,2.,self.phase),fmadds(step,3.,self.phase)];
        let advance=step*4.;
        for (group,out) in samples.chunks_exact_mut(4).enumerate() {
            for lane in 0..4 {
                let i=group*4+lane;
                out[lane]=lfs(lfs(a[i]*vcos(phase[lane]))-lfs(b[i]*vsin(phase[lane])));
                phase[lane]=lfs(phase[lane]+advance);
            }
        }
        let end=fmadds(step,256.,self.phase);
        self.phase=fnmsubs(fctiwz(end*INV_TAU)as f32,TAU,end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sine_reduction_and_signed_phase_are_finite() {
        for i in -1000..1000 {let x=i as f32*0.01;assert!((vsin(x)-x.sin()).abs()<0.000003);}
        for hz in [-1600.,0.,1600.] {
            let mut filter=FrequencyShift{hz,..Default::default()};
            let mut data=[0.;BLOCK];data[0]=1.;
            for _ in 0..100 {filter.process(48000.,&mut data);assert!(data.iter().all(|x|x.is_finite()));}
            assert!(filter.phase.abs()<TAU);
        }
    }
}

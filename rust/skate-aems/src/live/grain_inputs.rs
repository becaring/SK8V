//! Board surface ownership and grain parameters, from the TU3 instructions.
use std::path::Path;
use super::{components::helpers, driver::Component, frame::SkaterImage, grains::Grains, inputs, tuning::Tuning};
use crate::{eac::grain::{self, Inputs}, glue::env::Send};
use super::grains::Chain;

const CLASS: u64 = 0x7AB2_3C11_B6AD_A2DE;
const DEFAULT: u64 = 0xD7ED_BD36_2D7D_2152;

/// 824C8370: identities, not invented surface/recording names.
pub fn surface_key(surface: u32, soft: bool) -> u64 {
    match (surface, soft) {
        (1,false)=>0x7EB8_015B_4C02_405E, (1,true)=>0x943B_1CB0_5BA9_A6BA,
        (2,false)=>0x0372_1D0F_A99A_03C8, (2,true)=>0xB29F_ADBB_BC2F_39C2,
        (3,false)=>0x7C59_12FC_2DAB_F98C, (3,true)=>0xDC10_09D5_0E32_7F8F,
        (4,false)=>0xFFB5_E3E6_2E0B_4943, (4,true)=>0x607A_6BC3_D427_DA49,
        (5,false)=>0x7947_A259_F181_FDB4, (5,true)=>0x382B_1263_6ED9_D8DA,
        (6,false)=>0xB303_AED8_2415_30E2, (6,true)=>0x863C_58AC_34BA_D599,
        (9,_)=>0x1C9C_52CC_0E1C_D4CF, _=>DEFAULT,
    }
}

/// 824C5CA8. Call after inputs::slip, before the remaining board prep and
/// evaluator tick. Return AEMS rolling decisions to the normal Send consumer.
pub fn prepare(c: &mut Component, s: &SkaterImage, t: &Tuning, grains: &mut Grains,
    cache: &Path, values: &mut [i32;16]) -> Result<Vec<Send>,String> {
    if c.image.len()<1904 || c.c32(1500)>1 {return Err("invalid board grain component".into());}
    let local=t.g8(c.c32(16).wrapping_add(72))!=0;
    inputs::begin_surfaces(values);
    let mut sends=Vec::new();
    for iteration in 0..if local {2} else {1} {
        let selected=c.c32(1500) as usize;
        let mut pair=if iteration==0 {selected} else {1-selected};
        let mut other=1-pair;
        let surface=helpers(0x824C_82A8,&[0,pair as u32],c,s,t).0;
        let prior=c.c32(768+4*pair);
        if surface==prior {continue;}
        inputs::surface_changed(prior,values);
        if prior!=14 {
            if local && c.image[1496+other]==0 {
                std::mem::swap(&mut pair,&mut other);
                c.set32(1500,1-selected as u32);
            } else {
                if c.c32(1320+4*pair)==0 {
                    if c.c32(1312+4*pair)!=0 {sends.push(Send::release((1312+4*pair)as u32));}
                } else if c.image[1328+pair]!=0 {
                    grains.stop(2*pair);grains.stop(2*pair+1);c.set8(1328+pair,0);
                }
                c.set8(1496+pair,0);
            }
        }
        c.set32(768+4*pair,surface);
        if surface==14 {continue;}
        let key=surface_key(surface,s.r32(684)==1);
        c.set32(760,(key>>32)as u32);c.set32(764,key as u32);
        let handle=t.collection(CLASS,key);
        if handle==0 {return Err(format!("missing grain collection {key:016x}"));}
        c.set32(184+16*pair,handle);
        let stream=grain::stream_mode(surface as i32);
        if surface!=c.c32(768+4*other) {
            if stream {
                let surface=t.grain().and_then(|g|g.surfaces.get(&key)).ok_or("missing grain surface tuning")?;
                grains.load(cache,&surface.recording)?;
                for voice in 0..2 {grains.start(2*pair+voice,&surface.recording,surface.players[voice])?;}
                c.set8(1328+pair,1);
            } else {
                let wheel=match surface {7=>1,8=>2,10=>10,11=>12,12=>11,13=>9,_=>-1};
                c.set32(1488+4*pair,wheel as u32);
                // 824C4C18: wrapper +4..+48, initial intensity argument zero.
                sends.push(Send::create((1312+4*pair)as u32,
                    &[0,0,4096,0,wheel.clamp(0,15),0,(surface as i32).clamp(0,13),0,0,25000,0,32767]));
            }
            c.set8(1496+pair,1);
        }
        c.set32(1320+4*pair,stream as u32);
    }
    inputs::finish_surfaces(c,s,t,values);
    Ok(sends)
}

fn latch(c:&mut Component,s:&SkaterImage)->bool {
    if s.r8(340)!=0 {c.set8(1504,1);}
    else if c.image[1504]!=0 && matches!(s.r32(200),0|4) {c.set8(1504,0);}
    if c.image[1504]!=0 {return true;}
    if s.r8(372)!=0 {c.set8(1505,1);}
    else if c.image[1505]!=0 && s.r8(616)!=0 && s.r8(615)!=0 {c.set8(1505,0);}
    c.image[1505]!=0
}

/// 824C9058, before evaluation: update filter/shift/gain attributes using
/// the previous evaluator output, exactly where the native prep calls it.
pub fn prepare_chain(c:&mut Component,s:&SkaterImage,t:&Tuning,grains:&mut Grains)->Result<(),String> {
    let g=t.grain().ok_or("grain tuning absent")?;
    let keys=collection_keys(c,t);
    let selected=g.surfaces.get(&keys[c.c32(1500)as usize]).ok_or("selected grain curve absent")?;
    for pair in 0..2 {
        if c.image[1328+pair]==0 || c.c32(1320+4*pair)!=1 {continue;}
        let own=g.surfaces.get(&keys[pair]).ok_or("grain curve absent")?;
        let mut a=if latch(c,s) {f32::from_bits(*own.scalars.get(&0xF62B_C5EB_D8E5_DDE8).unwrap_or(&0))}else{0.};
        let mut b=own.curve.shift_b;
        if c.image[1156]==0 {let add=f32::from_bits(c.c32(1152));a+=add;b+=add;}
        let tilt=f32::from_bits(c.c32(1508));
        if tilt>0. {
            a=crate::ops::fmadds(selected.curve.shift_a,tilt,a);
            b=crate::ops::fmadds(tilt,f32::from_bits(*selected.scalars.get(&0x7FFF_3A8A_D448_09EF).unwrap_or(&0)),b);
        }
        // The instruction targets +4 (HI2) with parameter12, +8 (LI2)
        // with parameter11. Do not infer these from their parameter names.
        // Parameter 13 (824C9058) is the level of the second graph's aux
        // send (`*(mixer + 52)`), not of the dry path.
        let base=Chain {highpass:c.params.u15(12)as f32,lowpass:c.params.u15(11)as f32,shift:a};
        grains.set_chain(2*pair,base);grains.set_chain(2*pair+1,Chain{shift:b,..base});
    }
    Ok(())
}

fn collection_keys(c:&Component,t:&Tuning)->[u64;2] {
    std::array::from_fn(|pair|t.collection_entries().find_map(|((class,key),handle)|
        (class==CLASS && handle==c.c32(184+16*pair)).then_some(key)).unwrap_or(DEFAULT))
}

fn scalar(c:&Component,t:&Tuning,key:u64)->f32 {
    f32::from_bits(t.g32(t.attrib(c.c32(184+16*c.c32(1500)as usize),key)))
}
fn word(c:&Component,t:&Tuning,key:u64)->i32 {
    t.g32(t.attrib(c.c32(184+16*c.c32(1500)as usize),key))as i32
}
fn rf(c:&Component,at:usize)->f32 {f32::from_bits(c.c32(at))}
fn unit(x:f32)->f32 {let v=crate::glue::fsel(-x,0.,x);crate::glue::fsel(1.-v,v,1.)}
fn envelope_reset(c:&mut Component,base:usize) {c.image[base..base+124].fill(0);c.set8(base+120,1);}

/// Native board constructor 824C5058; host/controller ownership is initialized
/// separately by the driver. A finished envelope yields neutral speed/shift.
pub fn initialize(c:&mut Component,t:&Tuning) {
    for base in [912,1036,1340,1604,1760] {envelope_reset(c,base);}
    for pair in 0..2 {
        c.set32(768+4*pair,14);c.set8(1496+pair,0);c.set8(1328+pair,0);
        c.set32(1320+4*pair,0);c.set32(184+16*pair,0);
    }
    c.set32(1500,0);c.set8(1504,0);c.set8(1505,0);
    for offset in [1160,1164,1168,1508,1512,1900] {c.setf(offset,0.);}
    c.setf(1560,1.);
    // 824CA938 constructor: immutable root132 fields.
    for (offset,key) in [
        (1520,0x0D665393E2EDC605),(1524,0x28E708782445747F),
        (1528,0xE64C04ED542DABC8),(1532,0xD900C07BE7C5450F),
        (1536,0x88AA96B08FD16914),(1540,0x3FFB5107C82BA3E0),
        (1544,0xD3E8894CA25A4F71),(1548,0x55BEB30353F244A9),
        (1552,0x45516395725ED16B),(1564,0x281A501B22B6CCDF),
        (1568,0x54CDE019E31FC04E),(1572,0x36AE41817640FE04),
        (1576,0x71EE27313BD30F21),(1580,0x437D128B53669C34),
        (1584,0x02885338DD5D7DCA),(1728,0x2055BBF39C152FA9),
        (1732,0xF5240AFADA3B3FFC),(1736,0xF916E153393C5F24),
        (1740,0x0A36F90732016D85)] {
        c.set32(offset,t.required_word(132,key,0).unwrap_or(0));
    }
}
fn envelope_append(c:&mut Component,base:usize,from:f32,to:f32,ms:i32,linked:bool) {
    let count=c.c32(base+108)as usize;if count==5 {return;}
    let seconds=ms as f32*f32::from_bits(0x3A83126F);
    c.setf(base+4+count*4,if seconds>0. {seconds}else{f32::from_bits(0x3C23D70A)});
    c.setf(base+28+count*4,from);c.setf(base+52+count*4,to);
    c.set32(base+84+count*4,0);c.set8(base+76+count,linked as u8);
    c.set8(base+120,0);if count==0 {c.setf(base+116,from);}
    c.set32(base+108,(count+1)as u32);
}
fn envelope_tick(c:&mut Component,base:usize,dt:f32) {
    let count=c.c32(base+108)as usize;if c.image[base+120]!=0 || count==0 {return;}
    let at=c.c32(base+112)as usize;
    let elapsed=rf(c,base)+dt;c.setf(base,elapsed);
    let duration=rf(c,base+4+4*at);
    let end=rf(c,base+52+4*at);
    if elapsed>duration {
        c.setf(base+116,end);
        if at>=count-1 {c.set8(base+120,1);}
        else {
            c.setf(base,elapsed-duration);c.set32(base+112,(at+1)as u32);
            if c.image[base+76+at+1]!=0 {c.setf(base+28+4*(at+1),end);}
        }
    } else {
        let start=rf(c,base+28+4*at);
        // All segments this board constructor adds have native shape zero.
        c.setf(base+116,crate::ops::fmadds(end-start,elapsed/duration,start));
    }
}

/// 824C6218..66D8: contact-edge envelopes. Call after input4 and before
/// the rattle lifecycle, then finish_prep after that lifecycle.
pub fn contact_envelopes(c:&mut Component,s:&SkaterImage,t:&Tuning) {
    if s.r8(335)==0 {return;}
    let fraction=unit(((s.rf(208)-1.)*f32::from_bits(0x40666666))/scalar(c,t,0x2D75_1DEB_89BB_5E33));
    let from=if c.image[1032]!=0 {1.}else{rf(c,1028)};
    let low=scalar(c,t,0xE239_B03F_0E89_0686);
    let target=crate::ops::fmadds(scalar(c,t,0xB87E_CDDA_AB0F_8404)-low,fraction,low);
    let times=[word(c,t,0xB3D7_4688_20AF_C661),word(c,t,0xDAC9_DA91_0EF0_316C),word(c,t,0x0C3D_5DBC_262E_D276)];
    envelope_reset(c,912);envelope_append(c,912,from,target,times[0],false);
    envelope_append(c,912,0.,target,times[1],true);envelope_append(c,912,0.,1.,times[2],true);
    let low=scalar(c,t,0xC658_A792_3FC7_B99E);
    let target=crate::ops::fmadds(scalar(c,t,0xA15A_D56E_225A_DBA6)-low,fraction,low);
    let times=[word(c,t,0x09A5_CC79_BA21_78E7),word(c,t,0x3206_FD96_427E_A4D2),word(c,t,0xDB59_7F67_2CA4_7138)];
    envelope_reset(c,1036);envelope_append(c,1036,0.,target,times[0],false);
    envelope_append(c,1036,0.,target,times[1],true);envelope_append(c,1036,0.,0.,times[2],true);
}

fn dot(a:[f32;3],b:[f32;3])->f32 {(a[2]*b[2]+a[1]*b[1])+a[0]*b[0]}
fn reciprocal_length(q:f32)->f32 {
    let mut inverse=f32::from_bits(crate::ppc::vmx::rsqrte_bits(q.to_bits()));
    for _ in 0..2 {inverse=(inverse*0.5)*(1.-q*(inverse*inverse))+inverse;}
    inverse
}
fn vector(s:&SkaterImage,at:usize)->[f32;3] {std::array::from_fn(|i|s.rf(at+4*i))}
fn slew(from:f32,to:f32,step:f32)->f32 {
    if to-from>step {from+step}else if from-to>step {from-step}else{to}
}

/// 824C68C0..6A74, including 824C8588: envelope time, signed slide blend,
/// and frame-stepped backwards crossfade. Call before prepare_chain.
pub fn finish_prep(c:&mut Component,s:&SkaterImage,t:&Tuning,dt:f32)->Result<(),String> {
    if c.image[1032]==0 {envelope_tick(c,912,dt);}
    if c.image[1156]==0 {envelope_tick(c,1036,dt);}
    let g=t.grain().ok_or("grain tuning absent")?;
    let keys=collection_keys(c,t);
    let curve=&g.surfaces.get(&keys[c.c32(1500)as usize]).ok_or("selected grain curve absent")?.curve;
    let v=vector(s,96);let q=dot(v,v);
    let length=if q==0. {0.}else{q*reciprocal_length(q)};
    let scaled=length*f32::from_bits(0x3E75C28F);
    let positive=crate::glue::fsel(-scaled,0.,scaled);
    let cap=curve.blend_cap;
    let bounded=crate::glue::fsel(cap-positive,positive,cap);
    let mut target=bounded*s.rf(204);
    if target>cap {target=cap;}
    if target< -cap {target= -cap;}
    if latch(c,s) {target=0.;}
    let old=rf(c,1160);
    let step=scalar(c,t,if target>old {0x281F_F010_8147_5899}else{0x6376_4B8C_7EB9_EC9B});
    let blend=slew(old,target,step);c.setf(1160,blend);c.setf(1164,blend.abs());
    let backwards=if s.r8(336)!=0 {
        let other=vector(s,128);
        let a=v.map(|x|x*reciprocal_length(q));let b=other.map(|x|x*reciprocal_length(dot(other,other)));
        dot(a,b)<0.
    }else{false};
    c.setf(1168,slew(rf(c,1168),if backwards {1.}else{0.},f32::from_bits(0x3D4CCCCD)));
    Ok(())
}

fn area_target(c:&Component,grains:&mut Grains)->Result<(f32,i32),String> {
    let mut level=rf(c,1472);
    if level!=rf(c,1476) {
        let span=crate::ops::fctiwz((rf(c,1476)-level)*100.);
        if span==0 {return Err("native area envelope has sub-cent range".into());}
        let draw=grains.random()%span.unsigned_abs();
        level+=draw as f32*if span>0 {0.01}else{-0.01};
    }
    let low=c.c32(1480)as i32;let high=c.c32(1484)as i32;
    let time=if low==high {low}else{low.wrapping_add((grains.random()%high.wrapping_sub(low).unsigned_abs())as i32)};
    Ok((level,time))
}

/// 824CA448: surface-pattern gain envelope, after squeak/aux rolling prep.
pub fn area_prepare(c:&mut Component,s:&SkaterImage,t:&Tuning,grains:&mut Grains,dt:f32)->Result<(),String> {
    if t.g8(c.c32(16).wrapping_add(72))==0 {return Ok(());}
    let pattern=s.r32(636);
    if pattern==c.c32(1336) {if c.image[1460]==0 {envelope_tick(c,1340,dt);}return Ok(());}
    envelope_reset(c,1340);c.set32(1336,pattern);c.set8(1464,0);
    if pattern==0 {return Ok(());}
    let key=if (1..=15).contains(&pattern) {
        crate::glue::seams::name_hash(crate::glue::seams::SEAM_TYPES[pattern as usize-1].as_bytes())
    }else{0};
    let handle=t.collection(0x7242_F328_31ED_3332,key);
    for (offset,key) in [(1472,0xFA3A_5780_1765_A2F8),(1476,0x32A9_692F_1B82_6274),
        (1480,0xF713_CB54_7B1D_F920),(1484,0x0608_B312_9FF8_1F12)] {
        c.set32(offset,t.g32(t.attrib(handle,key)));
    }
    if rf(c,1472)!=1. || rf(c,1476)!=1. {
        c.set8(1464,1);let (level,time)=area_target(c,grains)?;
        envelope_reset(c,1340);envelope_append(c,1340,1.,level,time,false);c.setf(1468,level);
    }
    Ok(())
}

fn area_publish(c:&mut Component,grains:&mut Grains)->Result<(),String> {
    if c.image[1464]!=0 && c.image[1460]!=0 {
        let (level,time)=area_target(c,grains)?;
        let old=rf(c,1468);envelope_reset(c,1340);
        if old<1. {envelope_append(c,1340,old,1.,time,false);c.setf(1468,1.);}
        else {envelope_append(c,1340,1.,level,time,false);c.setf(1468,level);}
    }
    Ok(())
}

/// 824C6BD8 stream block publication; parameters must already be evaluated.
pub fn publish(c:&mut Component,s:&SkaterImage,t:&Tuning,grains:&mut Grains)->Result<(),String> {
    let g=t.grain().ok_or("grain tuning absent")?;
    let keys=collection_keys(c,t);
    let selected=g.surfaces.get(&keys[c.c32(1500)as usize]).ok_or("selected grain curve absent")?;
    let speed=s.rf(208)*if c.image[1032]!=0 {1.} else {f32::from_bits(c.c32(1028))};
    for pair in 0..2 {
        if c.image[1496+pair]==0 || c.c32(1320+pair*4)!=1 || c.image[1328+pair]==0 {continue;}
        let own=g.surfaces.get(&keys[pair]).ok_or("grain curve absent")?;
        let latched=latch(c,s);
        let i=Inputs {speed,level_a:c.params.u15(1),level_b:c.params.u15(2),pitch:c.params.pitch(3),
            blend:f32::from_bits(c.c32(1164)),backwards:f32::from_bits(c.c32(1168)),latched,
            latch_scale:f32::from_bits(*own.scalars.get(&0x6BDC_44AE_7C3C_79D0).unwrap_or(&0)),
            area:if c.image[1464]!=0 {Some(f32::from_bits(c.c32(1456)))}else{None},
            tilt:f32::from_bits(c.c32(1508))};
        let (a,b)=grain::blocks(&i,&own.curve,&selected.curve);
        grains.set_block(2*pair,&a);grains.set_block(2*pair+1,&b);
    }
    area_publish(c,grains)
}

/// 824CAEC0 + 824CB078 + 824CB180: local board speed modulation.
/// The extra DC/high-shelf parallel path is not folded into the dry path.
pub fn speed_modulation(c:&mut Component,s:&SkaterImage,t:&Tuning,g:&mut Grains,dt:f32)->Result<(),String> {
    if t.g8(c.c32(16).wrapping_add(72))==0 {
        c.setf(1592,1.);c.setf(1748,1.);c.setf(1560,1.);return Ok(());
    }
    let speed=s.rf(208)*3.6;
    let send=if speed>=rf(c,1524) {rf(c,1532)}else if speed>=rf(c,1520) {
        ((speed-rf(c,1520))/(rf(c,1524)-rf(c,1520)))*rf(c,1532)
    }else{0.};c.setf(1556,send);
    if speed>=rf(c,1540) {c.setf(1560,rf(c,1544));}
    else if speed>=rf(c,1536) {
        c.setf(1560,crate::ops::fnmsubs((speed-rf(c,1536))/(rf(c,1540)-rf(c,1536)),1.-rf(c,1544),1.));
    }
    for base in [1572,1728] {
        let env=base+32;
        if c.image[env+120]!=0 {
            let span=c.c32(base+4).wrapping_sub(c.c32(base));
            if span==0 {return Err("native grain modulation time range is zero".into());}
            let time=c.c32(base).wrapping_add(g.random()%span)as i32;
            let low=rf(c,base+8);
            let span=crate::ops::fctiwz((rf(c,base+12)-low)*100.)as u32;
            if span==0 {return Err("native grain modulation level range is zero".into());}
            let mut target=crate::ops::fmadds((g.random()%span)as i32 as f32,0.01,low);
            let old=rf(c,base+16);if old>=0. {target*= -1.;}
            envelope_reset(c,env);envelope_append(c,env,old,target,time,false);c.setf(base+16,target);
        }else{envelope_tick(c,env,dt);}
    }
    let fraction=if speed>=rf(c,1568) {1.}else if speed>=rf(c,1564) {
        (speed-rf(c,1564))/(rf(c,1568)-rf(c,1564))
    }else{0.};
    for voice in 0..2 {
        let base=1592+voice*156;
        let raw=crate::ops::fmadds(rf(c,1720+voice*156),fraction,1.);
        c.setf(base,raw*rf(c,1560));c.setf(base+8,raw);
        // Native compares raw target, so changes to the speed scale alone do
        // not issue a new gain command.
        if raw!=rf(c,base+4) {
            c.setf(base+4,raw);
            for pair in 0..2 {g.set_modulation(pair*2+voice,rf(c,base));}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn envelope_boundary_advances_only_one_segment_and_links_from_previous_end() {
        let mut c=Component::new("envelope fixture",1904,vec![]);
        envelope_reset(&mut c,912);
        envelope_append(&mut c,912,1.,3.,100,false);
        envelope_append(&mut c,912,99.,5.,100,true);
        envelope_tick(&mut c,912,0.1);
        assert_eq!(c.c32(912+112),0);assert_eq!(rf(&c,1028),3.);
        assert_eq!(c.image[1032],0);
        envelope_tick(&mut c,912,0.15);
        assert_eq!(c.c32(912+112),1);assert_eq!(rf(&c,912+32),3.);
        assert_eq!(rf(&c,1028),3.);assert_eq!(c.image[1032],0);
        envelope_tick(&mut c,912,0.);
        assert_eq!(rf(&c,1028),5.);assert_eq!(c.image[1032],1);
    }
    #[test]
    fn nonpositive_envelope_duration_uses_native_minimum() {
        let mut c=Component::new("envelope fixture",1904,vec![]);
        envelope_reset(&mut c,912);envelope_append(&mut c,912,0.,1.,0,false);
        assert_eq!(c.c32(916),0x3C23D70A);
        envelope_tick(&mut c,912,0.005);assert_eq!(rf(&c,1028),0.5);
    }
}

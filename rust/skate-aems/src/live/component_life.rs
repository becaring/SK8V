//! Player audio object lifetimes, expressed as message decisions. Native
//! entry addresses identify the TU3 routines;
//! guest allocation, pointers and game-global ownership are not reproduced.
use super::{creation::Message, driver::Component, frame::SkaterImage, tuning::Tuning};
use crate::glue::env::Env;
use crate::{glue::{env::Send, foley}, ops::fctiwz};

fn value(t: &Tuning, root: u32, key: u64) -> i32 { t.g32(t.tuning_at(root,key,0)) as i32 }
fn float(t: &Tuning, root: u32, key: u64) -> f32 { f32::from_bits(value(t,root,key) as u32) }

fn fraction(value: f32) -> f32 {
    let v=crate::glue::fsel(-value,0.,value);
    crate::glue::fsel(1.-v,v,1.)
}

/// 824CB3C8: ownership begins on either slide mode and ends on no contact.
pub fn board_slide(c: &mut Component, s: &SkaterImage, t: &Tuning) -> Vec<Send> {
    match (c.c32(1884)!=0,s.r32(780)) {
        (false,mode) if mode!=0 => vec![Send::create(1884,&Message::BoardSlide.pack(&[
            value(t,140,0xF2B4_4F93_BD91_662E),(mode==2) as i32]))],
        (true,0) => vec![Send::release(1884)],
        _ => vec![],
    }
}

/// 824C7738: two-wheel squeak direction changes replace the voice. Falling
/// below the angular trigger alone leaves a running voice alive.
pub fn squeaks(c: &mut Component, s: &SkaterImage, t: &Tuning) -> Vec<Send> {
    let exists=c.c32(1292)!=0;
    if s.r8(615)==0 || s.r8(616)==0 || (s.r32(200) as i32)<=1 {
        return if exists {vec![Send::release(1292)]} else {vec![]};
    }
    if fctiwz(s.rf(264).abs()*114.591552734375) < value(t,4,0xA129_B33B_4A2C_7961) {return vec![];}
    let direction=(s.rf(264)>=0.) as u8;
    let replaced=exists && c.image[1296]!=direction;
    c.set8(1296,direction);
    let mut actions=Vec::new();
    if replaced {actions.push(Send::release(1292));}
    if !exists || replaced {
        let amount=fctiwz(fraction(s.rf(208)*0.08)*1000.);
        let angular=fctiwz((s.rf(488).abs()/float(t,4,0xAC87_D592_E760_1134))*1000.);
        let angular=if angular>1000 {1000}else if angular<50 {0}else {angular};
        actions.push(Send::create(1292,&Message::Squeaks.pack(&[amount,angular,value(t,140,0x22D0_D4A5_A14F_FF7D)])));
    }
    actions
}

/// 824BB540: footsteps while pushing, foot braking and planted-foot moves
/// share the drag class but select different source parameters.
pub fn foot_drag(c: &mut Component,s: &SkaterImage,t: &Tuning)->Vec<Send> {
    let local=t.g8(c.c32(28).wrapping_add(72))!=0 && s.r8(310)!=0;
    if c.c32(128)!=0 || !(local || s.r8(336)!=0 || s.r8(339)!=0) {return vec![];}
    let level=if local {500}else {fctiwz(fraction(((s.rf(208)-float(t,24,0xE5A6_D8AC_6EB9_B5AB))/float(t,24,0xB2C8_1577_8204_08BE))*3.6)*10000.)};
    let other=s.r8(339)!=0;
    let style=super::components::helpers(0x824B_A390,&[0,other as u32],c,s,t).0 as i32;
    vec![Send::create(128,&Message::FootDrag.pack(&[level,style,
        value(t,24,0x9717_1DE6_035D_6069),value(t,24,0x194E_F41D_A225_4153),
        value(t,24,0x6242_BC04_80B0_9A33),value(t,24,0x9FF8_8541_CAB4_61C7),
        (s.r8(336)==0) as i32,value(t,140,if other {0x2DBD_9ED0_AD82_4844}else{0xC58C_E169_C133_20CA})]))]
}

/// 824C7438: wheel slip creates once; the established class handler owns
/// subsequent ramp updates and release after the source gate drops.
pub fn skid(c:&mut Component,s:&SkaterImage,t:&Tuning)->Vec<Send> {
    if c.c32(1288)!=0 || super::components::helpers(0x824C_72F0,&[],c,s,t).0==0 {return vec![];}
    let style=super::components::helpers(0x824C_7388,&[],c,s,t).0 as i32;
    let controller=c.c32(16);
    let local=t.g8(controller.wrapping_add(72))!=0;
    // The foreign host has no Skate challenge/session collection. Its
    // optional custom-board-material flag remains disabled.
    vec![Send::create(1288,&Message::Skid.pack(&[
        fctiwz(fraction(s.rf(208)*0.08)*10000.),(s.r32(684)==1)as i32,style,
        fctiwz(s.rf(232)*90.),value(t,84,0xFB10_048C_CDD6_ADFA),value(t,84,0x8CE4_2E5A_9388_A4C8),
        0,(local && t.g32(controller.wrapping_add(64))==0)as i32,local as i32,
        if local {c.params.u15(20)}else{0},value(t,140,0xF524_50E5_0425_0254)]))]
}

/// 824C13D0 and 824C9830: persistent seam and auxiliary rolling wrappers.
pub fn persistent(c:&mut Component,s:&SkaterImage,t:&Tuning)->Vec<Send> {
    let mut sends=Vec::new();
    if c.name=="seams" {
        for wheel in 0..4 {
            let slot=52+4*wheel;
            if c.c32(slot)!=0 {continue;}
            let params=c.params;
            let class=crate::glue::seams::wheel_class(&mut Env{comp:c,skater:s,tuning:t,params},wheel as i32,false);
            sends.push(Send::create(slot as u32,&Message::Seams.pack(&[class,(s.r32(684)==1)as i32,wheel as i32,value(t,140,0x5A83_7C61_3E3F_41DC)])));
            c.set8(68+wheel,1);
        }
    } else if c.name=="board" {
        for (slot,wheel) in [(1304,0),(1308,3)] {
            if c.c32(slot)!=0 {continue;}
            let max=super::components::helpers(0x824C_97B8,&[wheel],c,s,t).1;
            let level=fctiwz(fraction((s.rf(208)/max)*3.6)*10000.).clamp(0,10000);
            sends.push(Send::create(slot as u32,&[0,0,4096,level,wheel as i32,0,3,0,0,25000,0,32767]));
        }
    }
    sends
}

/// 824C28B0: grind envelope inputs, normalized held speed, and initial
/// wrappers. Subsequent grind-kind changes are owned by glue::grind::update.
pub fn grind(c:&mut Component,s:&SkaterImage,t:&Tuning)->Vec<Send> {
    use crate::glue::grind;
    let active=s.r8(341)!=0;
    c.inputs[0]=if active {32767}else{0};
    c.inputs[1]=if s.r8(342)!=0 && !active {32767}else{0};
    if active {
        c.set32(140,fctiwz(fraction(((s.rf(208)-0.5)/float(t,40,0x4890_392C_9182_9954))*3.6)*10000.).min(9000) as u32);
    }
    if c.c32(36)!=0 {
        return if active {vec![]}else{[36,40].into_iter().filter(|slot|c.c32(*slot)!=0).map(|slot|Send::release(slot as u32)).collect()};
    }
    if !active {return vec![];}
    let params=c.params;
    let mut env=Env{comp:c,skater:s,tuning:t,params};
    let class=if s.r32(692)==143 {4}else{grind::surface_class(&mut env,s.r32(692) as i32)};
    if class==14 {return vec![];}
    let collection=grind::surface_collection(&mut env,class);
    let c=env.comp;
    let kind=s.r32(192);c.set32(56,kind);
    let mode=match kind {1|2|4=>0,5=>3,_=>2};
    let local=t.g8(c.c32(16).wrapping_add(72))!=0;
    let player_zero=local && t.g32(c.c32(16).wrapping_add(64))==0;
    let mut sends=Vec::new();
    for (slot,mode) in [(36,mode),(40,1)] {
        if slot==40 && kind!=0 {continue;}
        let key=[0x0ECE_CDAC_28B2_B979,0x5807_0BF5_1180_9903,0x72BA_0780_A8FA_25D6,0x721A_50C8_0028_AD69][mode];
        let volume=fctiwz(f32::from_bits(t.g32(t.attrib(collection,key)))*32767.);
        sends.push(Send::create(slot,&grind::create_words(c.c32(140)as i32,class,mode as i32,volume,0,
            player_zero as i32,local as i32,if local {c.params.u15(6)}else{0},value(t,140,0xD489_344C_EDEE_5036))));
    }
    sends
}

/// 824C66D8..824C68BC: a new push-start pulse replaces the rattle, using
/// the currently selected stream's speed scale and latched surface identity.
pub fn rattle(c:&mut Component,s:&SkaterImage,t:&Tuning)->Vec<Send> {
    if s.r8(335)==0 {return vec![];}
    let mut sends=Vec::new();
    if c.c32(1300)!=0 {sends.push(Send::release(1300));}
    let pair=c.c32(1500)as usize;
    if c.c32(1320+4*pair)!=1 {return sends;}
    let collection=c.c32(184+16*pair);
    let scale=f32::from_bits(t.g32(t.attrib(collection,0x1227_5AA8_AC4A_63FB)));
    let level=fctiwz(fraction(((s.rf(208)-1.)*3.6)/scale)*10000.);
    let key=((c.c32(760)as u64)<<32)|c.c32(764)as u64;
    let material=match key {0x7C59_12FC_2DAB_F98C=>1,0x0372_1D0F_A99A_03C8=>2,
        0xFFB5_E3E6_2E0B_4943=>3,0x7947_A259_F181_FDB4=>4,0xB303_AED8_2415_30E2=>5,_=>0};
    sends.push(Send::create(1300,&Message::Rattle.pack(&[level,material,value(t,140,0xC048_3297_8CDE_D925)])));
    sends
}

/// 824C9F68: the special spidercrack auxiliary rolling layer.
pub fn seam_roll(c:&mut Component,s:&SkaterImage,t:&Tuning)->Vec<Send> {
    let _=super::components::helpers(0x824C_82A8,&[0,0],c,s,t);
    let pattern=s.r32(if c.image[1504]!=0 && s.r8(464)==0 {648}else{636});
    if c.c32(1332)!=0 {return if pattern==1 {vec![]}else{vec![Send::release(1332)]};}
    if pattern!=1 {return vec![];}
    let params=c.params;
    let level=crate::glue::rolling::wheel_speed(&mut Env{comp:c,skater:s,tuning:t,params},5);
    vec![Send::create(1332,&crate::glue::rolling::create_words(level,5,3))]
}

/// 824E7980: rattle uses ground speed, wind uses COM speed and bail thresholds.
pub fn speed(c: &mut Component, s: &SkaterImage, t: &Tuning) -> Vec<Send> {
    if t.g8(c.c32(28).wrapping_add(72))==0 {return vec![];}
    let mut actions=Vec::new();
    for (slot,speed,lo,hi) in [(36,s.rf(208)*3.6,52,56),
        (40,s.rf(212)*3.6,if s.r8(676)!=0 {68}else{60},if s.r8(676)!=0 {72}else{64})] {
        let threshold=f32::from_bits(c.c32(lo));
        if speed < threshold {
            if c.c32(slot)!=0 {actions.push(Send::release(slot as u32));}
        } else if c.c32(slot)==0 {
            let end=f32::from_bits(c.c32(hi));
            let amount=fctiwz(fraction((speed-threshold)/(end-threshold))*1000.);
            let words=if slot==36 {
                Message::SpeedRattle.pack(&[amount,value(t,140,0x4D30_A6CA_8C9E_1ABC),c.c32(92)as i32,c.c32(96)as i32])
            } else {
                Message::Wind.pack(&[amount,value(t,140,0xC1DC_8556_BA66_CDCD),c.c32(76)as i32,
                    c.c32(80)as i32,c.c32(84)as i32,c.c32(88)as i32])
            };
            actions.push(Send::create(slot as u32,&words));
        }
    }
    actions
}

/// 824DBF10 and 824DC0E8: distinct fall-edge and body-contact lifetimes.
pub fn body(c: &mut Component, s: &SkaterImage, t: &Tuning) -> Vec<Send> {
    let mut actions=Vec::new();
    let bailed=s.r8(676)!=0;
    let entered=c.image[36]==0 && bailed;
    let divisor=float(t,136,0x0A9A_9BD1_150F_E838);
    let inv=1./divisor;
    let initial=fctiwz((s.rf(328)*inv)*1000.);
    let impact=fctiwz((s.rf(672)*inv)*1000.);
    c.set32(44,initial.max(impact) as u32);
    if c.c32(40)!=0 {
        if s.r8(677)!=0 || !bailed {actions.push(Send::release(40));}
    } else if entered {
        actions.push(Send::create(40,&Message::ClothFalls.pack(&[initial,value(t,140,0xE633_C8F0_09CA_EFFC)])));
    }
    c.set8(36,bailed as u8);
    let params=c.params;
    let (class,level,touching)=foley::body_contacts(&mut Env{comp:c,skater:s,tuning:t,params});
    if c.c32(60)!=0 {
        if level <= value(t,36,0x746E_A8EF_187E_1571) || !touching {actions.push(Send::release(60));}
    } else if touching && level > value(t,36,0xB021_DB33_8B89_D0F2) {
        actions.push(Send::create(60,&Message::BodySlide.pack(&[level,class,bailed as i32,value(t,140,0x4A02_2FEF_9905_D8F4)])));
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn board_slide_lifetime_follows_contact_without_restarting_for_mode_changes() {
        let mut c=Component::new("board",2048,vec![]);let mut s=SkaterImage::default();let t=Tuning::default();
        assert!(board_slide(&mut c,&s,&t).is_empty());
        s.w32(780,2);let sends=board_slide(&mut c,&s,&t);
        assert_eq!(sends.len(),1);assert_eq!(sends[0].words[10],1);
        c.set32(1884,1);s.w32(780,1);assert!(board_slide(&mut c,&s,&t).is_empty());
        s.w32(780,0);assert!(board_slide(&mut c,&s,&t)[0].release);
    }
    #[test]
    fn squeak_direction_change_replaces_voice_but_low_angle_holds_it() {
        let mut c=Component::new("board",2048,vec![]);let mut s=SkaterImage::default();let mut t=Tuning::default();
        t.set_tuned(4,0xA129_B33B_4A2C_7961,0,10);
        t.set_tuned(4,0xAC87_D592_E760_1134,0,1f32.to_bits());
        s.w8(615,1);s.w8(616,1);s.w32(200,2);s.wf(264,1.);
        assert!(squeaks(&mut c,&s,&t)[0].create);
        c.set32(1292,1);s.wf(264,0.);assert!(squeaks(&mut c,&s,&t).is_empty());
        s.wf(264,-1.);let sends=squeaks(&mut c,&s,&t);
        assert_eq!(sends.len(),2);assert!(sends[0].release);assert!(sends[1].create);
        s.w8(615,0);assert!(squeaks(&mut c,&s,&t)[0].release);
    }
    #[test]
    fn speed_objects_start_at_their_threshold_and_release_below_it() {
        let mut c=Component::new("speed",192,vec![]);
        c.setf(52,18.); c.setf(56,36.); c.setf(60,18.); c.setf(64,36.);
        let mut s=SkaterImage::default(); let mut t=Tuning::default();
        let controller=t.alloc(80);t.w8(controller+72,1);c.set32(28,controller);
        s.wf(208,5.); s.wf(212,5.);
        let start=speed(&mut c,&s,&t);
        assert_eq!(start.len(),2);
        assert!(start.iter().all(|x|x.create));
        c.set32(36,1); c.set32(40,1);
        assert!(speed(&mut c,&s,&t).is_empty(),"steady voices must not restart");
        s.wf(208,4.); s.wf(212,4.);
        let end=speed(&mut c,&s,&t);
        assert_eq!(end.iter().map(|x|x.slot).collect::<Vec<_>>(),[36,40]);
        assert!(end.iter().all(|x|x.release));
    }
    #[test]
    fn cloth_fall_uses_rising_bail_state_and_releases_on_recovery() {
        let mut c=Component::new("body",192,vec![]);let mut s=SkaterImage::default();let mut t=Tuning::default();
        t.set_tuned(136,0x0A9A_9BD1_150F_E838,0,10f32.to_bits());
        t.set_tuned(36,0x7871_71EC_D02D_BBC3,0,10f32.to_bits());
        s.w8(676,1);s.wf(328,3.);
        let start=body(&mut c,&s,&t);
        assert_eq!(start.len(),1);assert_eq!(start[0].slot,40);assert!(start[0].create);
        c.set32(40,1);
        assert!(body(&mut c,&s,&t).is_empty());
        s.w8(676,0);
        assert!(body(&mut c,&s,&t).iter().any(|v|v.slot==40 && v.release));
    }
}

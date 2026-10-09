//! Skate 3's sound components, live: which objects exist when, with which
//! create words, and which handlers update them. (Filled from the decode of
//! the game-variables layer; see docs/AEMS.md "Live engine".)

use super::driver::{Component, Driver};
use crate::ops::fsel;
use super::frame::SkaterImage;
use super::tuning::Tuning;
use crate::live::engine::Voices;
use crate::world::World;

#[derive(Default, Clone, Debug)]
pub struct State {
    pub initialized: bool,
    /// `0x82772B88`'s ring of the centre of mass's vertical velocity
    /// (`this+680..692`, index `+696`).
    pub fall: [f32; 4],
    pub fall_at: usize,
}

/// `0x82772B88` and its fill (`0x824B0DA8`): skater `+300`, the landing
/// class 1..4 from the fastest fall (skater `+100`) over the last four
/// frames against root 92 `7385078DD3C063BA[0..2]`; 1 during a trick other
/// than 31 (`+343`/`+348`) or once `+720` has run out.
pub fn landing_class(state: &mut State, s: &mut SkaterImage, t: &Tuning) {
    state.fall[state.fall_at] = s.rf(100);
    state.fall_at = (state.fall_at + 1) % 4;
    let [r0, r1, r2, r3] = state.fall;
    let m = fsel(-r0, r0, 0.0);
    let m = fsel(m - r1, r1, m);
    let m = fsel(m - r2, r2, m);
    let m = fsel(m - r3, r3, m).abs();
    let limit = |i| f32::from_bits(t.g32(t.tuning_at(92, 0x7385_078D_D3C0_63BA, i)));
    let mut class = if !(m <= limit(2)) { 4 } else if !(m <= limit(1)) { 3 } else if m > limit(0) { 2 } else { 1 };
    if s.r8(343) != 0 && s.r32(348) != 31 {
        class = 1;
    }
    if s.r32(720) == 0 {
        class = 1;
    }
    s.w32(300, class);
}

/// Root 36's body impulse speed curve: eight knots at `+16`, their values at
/// `+48` (`0x824B18A8` reads the collection's layout at offset 0, this record).
pub const IMPULSE_SPEED_CURVE: u64 = 0x8B16_4823_E008_749C;

/// The fill (`0x824B0DA8`, loop `0x824B18A8`): each of the eight body region
/// impulses (`+496 + 4k`, copied from the packet clamped to 0.001..1) times
/// root 36's curve at the previous frame's centre-of-mass speed (`+216`,
/// before the fill copies `+212` there). The curve rises from 1 at rest to
/// 5 from 0.95 m/s. Production tuning must hold the record (`validate_live_roots`);
/// a tuning without it (unit fixtures) leaves the impulses unscaled.
pub fn impulse_speed_scale(s: &mut SkaterImage, t: &Tuning) {
    let curve = t.tuning_at(36, IMPULSE_SPEED_CURVE, 0);
    if curve == super::tuning::DEFAULT { return; }
    let scale = super::foley_life::knots(t, curve + 16, curve + 48, 8, s.rf(216));
    for k in 0..8 {
        s.wf(496 + 4 * k, scale * s.rf(496 + 4 * k));
    }
}

/// Local player-controller gates in the foreign host, plus values loaded
/// from the source tuning. Constructors never use trace snapshots.
pub fn initialize(components:&mut [Component],t:&mut Tuning)->Result<(),String> {
    let existing=components.first().map_or(0,|c|c.c32(16));
    let controller=if existing!=0 {existing}else{t.alloc(80)};
    t.w8(controller+52,1);t.w8(controller+72,1);t.w32(controller+64,0);
    for c in components {
        c.image.fill(0);c.inputs.fill(0);c.params=Default::default();
        c.set32(16,controller);c.set32(28,controller);
        match c.name {
            "board" => super::grain_inputs::initialize(c,t),
            "foot_drag" => super::body_impacts::initialize(c,t),
            "seams" => {
                c.set32(36,t.collection(0x7242_F328_31ED_3332,0xD7ED_BD36_2D7D_2152));
                c.set32(40,t.collection_layout(c.c32(36)));
                for offset in (72..104).step_by(4) {c.set32(offset,i32::MAX as u32);}
                for offset in (112..128).step_by(4) {c.set32(offset,143);}
            },
            "speed" => {
                for (off,key) in [(44,0xEB22_4272_A924_C135),(48,0x1BBA_9174_BD1B_2D88),
                    (52,0xF57A_74AF_D22A_D030),(56,0x1227_5AA8_AC4A_63FB),
                    (60,0x8B4D_ECD6_46B1_3210),(64,0x73E9_A42D_88C5_1169),
                    (68,0x3BDF_AC29_8128_131F),(72,0x7BCB_09DF_227E_DDAC),
                    (76,0x4381_FEC9_776F_5EC9),(80,0xF7A8_067C_AE4C_51D3),
                    (84,0xD271_5B3A_D1BB_CDB5),(88,0xC689_DD34_34BC_4170),
                    (92,0x75F1_4A57_A506_E686),(96,0x5F0E_3DDE_232C_2BF3),
                    (136,0x7508_154F_F73D_DCED),(140,0x1185_E9A6_9919_B051)] {
                    c.set32(off,t.required_word(132,key,0)?);
                }
                c.setf(132,t.required_word(132,0x9BC1_3FA1_9CC4_DF00,0)? as i32 as f32);
            },
            _=>{},
        }
    }
    Ok(())
}

pub fn build(world: &mut World, problems: &mut Vec<String>) -> Vec<Component> {
    use crate::glue::{board, foley, grind, rolling, seams, speed};
    // Component identity, banks and wrapper slots follow the TU3 component
    // vtables. No guest pointers or captured bytes initialize live objects.
    let mut result = Vec::new();
    let mut add = |name, bank, size, handlers, slots: &[(u32, &str)]| {
        let mut c = Component::new(name, size, handlers);
        c.bank = bank;
        for &(slot, class) in slots {
            match world.reference(1, class) {
                Some(reference) => { c.classes.insert(slot, reference); }
                None => problems.push(format!("{name}: required AEMS class {class} is not loaded")),
            }
        }
        result.push(c);
    };
    add("board", 0, 2048, vec![rolling::update_6bd8, rolling::update_9948,
        rolling::update_a038, board::rattle, board::wheels_skid, board::squeaks, board::board_slide],
        &[(1288,"Class_wheels_skid"),(1292,"Class_Squeaks"),(1300,"Rolling_Rattle_Class"),
          (1304,"Class_rolling"),(1308,"Class_rolling"),(1312,"Class_rolling"),
          (1316,"Class_rolling"),(1332,"Class_rolling"),(1884,"c_board_slide")]);
    add("foot_drag", 1, 512, vec![board::foot_drag], &[(128,"Class_foot_drag")]);
    add("grind", 3, 256, vec![grind::update], &[(36,"Class_grind"),(40,"Class_grind")]);
    add("seams", 4, 192, vec![seams::clicks,board::seams],
        &[(52,"Class_Seams"),(56,"Class_Seams"),(60,"Class_Seams"),(64,"Class_Seams")]);
    add("flips", 5, 192, vec![board::flips,foley::cloth_trick_a,foley::cloth_trick_b],
        &[(36,"Class_Flips"),(40,"cloth_trick"),(44,"cloth_trick")]);
    add("body", 6, 192, vec![foley::body_slide,foley::cloth_falls],
        &[(40,"c_cloth_falls"),(60,"c_body_slide")]);
    add("speed", 8, 192, vec![speed::update],
        &[(36,"SenseOfSpeed_rattle"),(40,"SenseOfSpeed_wind")]);
    add("footsteps", 9, 512, vec![foley::player_footsteps],
        &[(36,"playercharacter_footstep"),(220,"playercharacter_footstep")]);
    result
}

pub fn update(d: &mut Driver, world: &mut World, vs: &mut Voices,
    grains:&mut super::grains::Grains,splices:&mut super::splices::Splices,cache:&std::path::Path)->Result<(),String> {
    use super::{component_life as life,driver,inputs,grain_inputs,foley_life};
    let dt=1./60.;
    if d.evaluator.is_none() {return Err("audio evaluator is not configured".into());}
    {
        let evaluator=d.evaluator.as_mut().unwrap();
        super::mix_inputs::settings(evaluator);
        super::mix_inputs::player(evaluator,&d.skater);
    }
    for c in &mut d.components {
        let s=&d.skater;let t=&d.tuning;
        if !d.state.initialized {
            match c.name {"flips"=>foley_life::init_flips(c),"footsteps"=>foley_life::init_footsteps(c),_=>{}}
            let sends=life::persistent(c,s,t);
            driver::apply(world,vs,c,sends,&mut d.problems);
        }
        let mut sends=Vec::new();
        match c.name {
            "board"=>{
                let mut values=c.inputs;
                inputs::slip(c,s,t,&mut values)?;
                sends.extend(grain_inputs::prepare(c,s,t,grains,cache,&mut values)?);
                inputs::airborne(s,&mut values);
                grain_inputs::contact_envelopes(c,s,t);
                sends.extend(life::rattle(c,s,t));
                grain_inputs::finish_prep(c,s,t,dt)?;
                grain_inputs::prepare_chain(c,s,t,grains)?;
                inputs::skid(c,s,t,&mut values);
                sends.extend(life::skid(c,s,t));sends.extend(life::squeaks(c,s,t));
                sends.extend(life::seam_roll(c,s,t));
                grain_inputs::area_prepare(c,s,t,grains,dt)?;
                grain_inputs::speed_modulation(c,s,t,grains,dt)?;
                sends.extend(life::board_slide(c,s,t));
                inputs::delta(c,t,dt,&mut values)?;c.inputs=values;
            },
            "foot_drag"=>{
                super::board_foley::update(c,s,t,splices,&mut d.collisions,dt);
                sends.extend(life::foot_drag(c,s,t));
            },
            "grind"=>sends.extend(life::grind(c,s,t)),
            "body"=>{
                sends.extend(life::body(c,s,t));
                super::body_foley::update(c,s,t,splices,dt);
            },
            "speed"=>sends.extend(life::speed(c,s,t)),
            "flips"=>sends.extend(foley_life::prep_flips(c,s,t,dt,Default::default()).sends),
            "footsteps"=>sends.extend(foley_life::prep_footsteps(c,s,t,splices,dt).sends),
            _=>{},
        }
        driver::apply(world,vs,c,sends,&mut d.problems);
        let evaluator=d.evaluator.as_mut().unwrap();
        let id=0x4001_0000|((c.bank as u32)<<4);
        evaluator.set_inputs(id,c.inputs);
    }
    d.state.initialized=true;
    let evaluator=d.evaluator.as_mut().unwrap();
    d.collisions.inputs(evaluator);
    evaluator.tick(dt)?;
    for c in &mut d.components {c.params=evaluator.params(0x4001_0000|((c.bank as u32)<<4));}
    for index in 0..d.components.len() {
        if d.components[index].name=="board" {
            grain_inputs::publish(&mut d.components[index],&d.skater,&d.tuning,grains)?;
        }
        if d.components[index].name=="foot_drag" {
            super::board_foley::instances(&mut d.components[index],&d.skater,&d.tuning,splices,dt);
        }
        d.run(index,world,vs);
        if d.components[index].name=="body" {
            super::body_foley::instances(&mut d.components[index],&d.tuning,splices,dt);
        }
        if d.components[index].name=="footsteps" {
            super::footsteps::instances(&mut d.components[index],&d.skater,&d.tuning,splices,dt);
        }
    }
    d.collisions.update(d.evaluator.as_ref().unwrap(),&d.tuning,splices,dt);
    splices.service();
    Ok(())
}

/// Game helpers the handlers call, computed from the skater state.
pub fn helpers(f: u32, args: &[u32], c: &mut Component, s: &SkaterImage, tuning: &Tuning) -> (u32, f32) {
    // Behavior adapters for the helpers at the named VAs.
    let surface_field = |surface: i32, offset: u32| {
        let index = if (0..94).contains(&surface) { surface as u32 } else { 94 };
        tuning.g32(tuning.tuning_at(64, 0x4CA6_0755_8B1C_F440, index).wrapping_add(offset))
    };
    let which = args.get(1).copied().unwrap_or(0);
    match f {
        0x824C_97B8 => {
            let at = tuning.tuning_at(56, 0x880C_82E8_EF64_7EC4, args.first().copied().unwrap_or(0));
            (0, f32::from_bits(tuning.g32(at)))
        }
        0x824C_82A8 => {
            // 824CA688: manual latch survives until the published ground mode
            // is zero or four. The helper mutates its owning component.
            let flagged = tuning.g8(c.c32(16).wrapping_add(72)) != 0;
            if flagged {
                if s.r8(340) != 0 { c.set8(1504, 1); }
                else if c.image[1504] != 0 && matches!(s.r32(200), 0 | 4) { c.set8(1504, 0); }
                let contact = if which == c.c32(1500) { 464 } else { 467 };
                if c.image[1504] != 0 && s.r8(contact) == 0 { return (14, 0.); }
            }
            if s.r8(341) != 0 { return (14, 0.); }
            let surface = s.r32(if which == c.c32(1500) { 620 } else { 632 }) as i32;
            (if surface >= 143 { 3 } else { surface_field(surface, 4) }, 0.)
        }
        0x824C_72F0 => {
            // TU3 82165A10 is 0.0: preserve the native unordered/NaN branch.
            let active = if s.rf(232) > 0. || s.rf(232).is_nan() {
                if s.r8(341) != 0 { s.r32(192) == 4 }
                else { !(s.r8(716) != 0 && s.r8(308) != 0) && matches!(s.r32(348) as i32, -1 | 35) }
            } else { s.r8(690) != 0 || c.c32(1516) as i32 > 0 };
            (active as u32, 0.)
        }
        0x824C_7388 => {
            let surface = s.r32(620) as i32;
            (if surface >= 143 { 0 } else { surface_field(surface, 12) }, 0.)
        }
        0x824B_A390 => {
            let other = which as u8 != 0;
            let surface = s.r32(if other { 628 } else { 620 }) as i32;
            let style = if surface >= 143 { 0 } else { surface_field(surface, 20) };
            (if other && style == 1 { 0 } else { style }, 0.)
        }
        _ => (0, 0.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_impulses_scale_by_the_previous_speed_curve() {
        // A synthetic eight-knot record in root 36's layout (header, x at
        // +16, y at +48); the owned tuning supplies the real one.
        let mut t = Tuning::default();
        let at = t.set_tuned_record(36, IMPULSE_SPEED_CURVE, 0, 80);
        let xs = [0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        let ys = [1.0f32, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5];
        for i in 0..8u32 {
            t.w32(at + 16 + 4 * i, xs[i as usize].to_bits());
            t.w32(at + 48 + 4 * i, ys[i as usize].to_bits());
        }
        let mut s = SkaterImage::default();
        s.wf(496, 0.001); s.wf(500, 1.0); s.wf(504, 0.25);
        s.wf(216, 30.0);
        impulse_speed_scale(&mut s, &t);
        // From the last knot on: the last value, for all eight regions.
        assert_eq!((s.rf(496), s.rf(500), s.rf(504)), (0.001f32 * 4.5, 4.5, 1.125));
        let mut s = SkaterImage::default();
        s.wf(500, 0.5); s.wf(216, 0.0);
        impulse_speed_scale(&mut s, &t);
        assert_eq!(s.rf(500), 0.5);
        // Between knots, and the previous frame's speed (+216), not +212.
        let mut s = SkaterImage::default();
        s.wf(500, 1.0); s.wf(216, 2.5); s.wf(212, 7.0);
        impulse_speed_scale(&mut s, &t);
        assert_eq!(s.rf(500), 2.25);
        // Without the record (fixtures) nothing changes.
        let mut s = SkaterImage::default();
        s.wf(500, 0.5); s.wf(216, 30.0);
        impulse_speed_scale(&mut s, &Tuning::default());
        assert_eq!(s.rf(500), 0.5);
    }
    #[test]
    fn player_component_catalog_covers_each_gameplay_voice_family() {
        let mut problems = Vec::new();
        let components = build(&mut World::default(), &mut problems);
        let names: Vec<_> = components.iter().map(|c| c.name).collect();
        assert_eq!(names, ["board", "foot_drag", "grind", "seams", "flips", "body", "speed", "footsteps"]);
        assert!(components.iter().all(|c| !c.handlers.is_empty()));
        assert!(components.iter().all(|c| c.objects.is_empty()));
        assert!(!problems.is_empty(), "missing banks must be reported before the driver can become ready");
    }
    #[test]
    fn helpers_keep_manual_contact_gate_and_surface_roles_distinct() {
        let mut c = Component::new("board", 2048, vec![]);
        let mut t = Tuning::default();
        let gate = t.alloc(80); t.w8(gate + 72, 1); c.set32(16, gate);
        let row = t.set_tuned_record(64, 0x4CA6_0755_8B1C_F440, 7, 48);
        t.w32(row + 4, 6); t.w32(row + 12, 2); t.w32(row + 20, 1);
        let mut s = SkaterImage::default(); s.w32(620, 7); s.w32(628, 7); s.w8(340, 1);
        assert_eq!(helpers(0x824C_82A8, &[0,0], &mut c, &s, &t).0, 14);
        s.w8(464, 1);
        assert_eq!(helpers(0x824C_82A8, &[0,0], &mut c, &s, &t).0, 6);
        assert_eq!(helpers(0x824C_7388, &[], &mut c, &s, &t).0, 2);
        assert_eq!(helpers(0x824B_A390, &[0,0], &mut c, &s, &t).0, 1);
        assert_eq!(helpers(0x824B_A390, &[0,1], &mut c, &s, &t).0, 0);
        s.w8(340, 0); s.w32(200, 4); s.w8(464, 0);
        assert_eq!(helpers(0x824C_82A8, &[0,0], &mut c, &s, &t).0, 6);
        assert_eq!(c.image[1504], 0);
    }
    #[test]
    fn landing_class_takes_the_fastest_fall_of_four_frames() {
        let mut t = Tuning::default();
        for (i, v) in [2.0f32, 4.0, 6.0].into_iter().enumerate() {
            t.set_tuned(92, 0x7385_078D_D3C0_63BA, i as u32, v.to_bits());
        }
        let mut state = State::default();
        let mut s = SkaterImage::default();
        s.w32(720, 5);
        // Rising velocities count as zero.
        s.wf(100, 3.0);
        landing_class(&mut state, &mut s, &t);
        assert_eq!(s.r32(300), 1);
        s.wf(100, -5.0);
        landing_class(&mut state, &mut s, &t);
        assert_eq!(s.r32(300), 3);
        // Kept for four frames, then gone.
        s.wf(100, 0.0);
        for _ in 0..3 {
            landing_class(&mut state, &mut s, &t);
            assert_eq!(s.r32(300), 3);
        }
        landing_class(&mut state, &mut s, &t);
        assert_eq!(s.r32(300), 1);
        s.wf(100, -7.0);
        landing_class(&mut state, &mut s, &t);
        assert_eq!(s.r32(300), 4);
        s.w32(720, 0);
        landing_class(&mut state, &mut s, &t);
        assert_eq!(s.r32(300), 1);
    }
    #[test]
    fn skid_release_uses_slip_grind_family_and_retained_ramp() {
        let mut c = Component::new("board", 2048, vec![]);
        let t = Tuning::default(); let mut s = SkaterImage::default();
        assert_eq!(helpers(0x824C_72F0, &[], &mut c, &s, &t).0, 0);
        c.set32(1516, 5);
        assert_eq!(helpers(0x824C_72F0, &[], &mut c, &s, &t).0, 1);
        s.wf(232, 0.5); s.w8(341, 1); s.w32(192, 4);
        assert_eq!(helpers(0x824C_72F0, &[], &mut c, &s, &t).0, 1);
        s.w32(192, 1);
        assert_eq!(helpers(0x824C_72F0, &[], &mut c, &s, &t).0, 0);
    }
}

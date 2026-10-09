//! Synthetic tuning only: no retail values.
use super::*;
use skate_host::bridge::hom::{HomBone, HomInput};

fn ramp(x0: f32, x1: f32, y0: f32, y1: f32) -> Graph {
    let mut g = Graph::default();
    for i in 0..8 {
        let t = i as f32 / 7.0;
        g.x[i] = x0 + (x1 - x0) * t;
        g.y[i] = y0 + (y1 - y0) * t;
    }
    g
}

/// Every bone: levels at damage 10, 20, .. 60 worth 1, 2, 4, 8, 16, 32 points;
/// bone 0 passes half its damage to bone 1, bone 1 a quarter back to bone 0
/// and half on to bone 2. Graphs map 0..100 onto 0..1000 points.
fn tuning() -> Tuning {
    let mut bones = [Bone::default(); 25];
    for b in &mut bones {
        b.neighbours = [(-1, 0.0); 4];
        for l in 0..6 {
            b.levels[l] = (1 << l, 10.0 * (l + 1) as f32);
        }
    }
    bones[0].neighbours[0] = (1, 0.5);
    bones[1].neighbours[0] = (0, 0.25);
    bones[1].neighbours[1] = (2, 0.5);
    Tuning {
        bones,
        graphs: [ramp(0.0, 100.0, 0.0, 1000.0); 12],
        points_limit: 1000,
        leg_secondary: 0.5,
        impulse_scale: 1.0,
        tweaks: vec![
            Tweak { id: 0, direction: [0.0, 1.0], hold_ticks: 3, points: 50 },
            Tweak { id: 2, direction: [0.0, -1.0], hold_ticks: 3, points: 70 },
        ],
        tweak_deadzone: 0.5,
        entities: [
            EntityPoints { table: vec![], default: 100 },
            EntityPoints { table: vec![(-1, 300)], default: 200 },
            EntityPoints { table: vec![], default: 400 },
        ],
    }
}

fn input(category: u32) -> HomInput {
    HomInput {
        category,
        pelvis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        ..Default::default()
    }
}

#[test]
fn graphs_clamp_and_interpolate() {
    let g = ramp(0.0, 70.0, 0.0, 700.0);
    assert_eq!(g.eval(-5.0), 0.0);
    assert_eq!(g.eval(80.0), 700.0);
    assert_eq!(g.eval(70.0), 700.0);
    assert!((g.eval(15.0) - 150.0).abs() < 1e-3);
    let mut step = g;
    step.x[3] = step.x[2];
    assert_eq!(step.eval(step.x[2]), step.y[3]);
}

#[test]
fn damage_spreads_to_neighbours_but_not_back_to_its_source() {
    let mut s = Scorer::new(tuning());
    s.damage(0, 40.0, 0, 0);
    assert_eq!(s.slots[0].damage, 40.0);
    assert_eq!(s.slots[1].damage, 20.0);
    // Bone 1 spreads on to bone 2 but not back to bone 0, its source.
    assert_eq!(s.slots[2].damage, 10.0);
    // Spread below the epsilon stops (0.15 would pass 0.075 on).
    s.damage(1, 0.15, 1, 0);
    assert_eq!(s.slots[2].damage, 10.0);
    assert_eq!(s.slots[1].damage, 20.15);
}

#[test]
fn levels_need_a_positive_threshold_and_points_sum_per_level() {
    let mut t = tuning();
    t.bones[3].levels[5].1 = 0.0;
    assert_eq!(t.level(3, 1000.0), 4);
    assert_eq!(t.level(0, 10.0), -1);
    assert_eq!(t.level(0, 10.5), 0);
    let mut s = Scorer::new(t);
    s.slots[0].level = 2;
    s.slots[1].level = 2;
    s.slots[2].level = 5;
    assert_eq!(s.bone_points(None), 4 + 4 + 32);
    assert_eq!(s.bone_points(Some(2)), 8);
}

fn contact(force: f32) -> HomBone {
    HomBone { force, normal: [0.0, 1.0, 0.0], world: true, ..Default::default() }
}

#[test]
fn a_broken_bone_starts_thirty_ticks_of_slow_motion() {
    let mut s = Scorer::new(tuning());
    let mut i = input(CATEGORY_WIPEOUT);
    let out = s.tick(&i, &[], [false; 5]);
    assert!(out.started && out.reset && out.published);
    assert_eq!(out.broken_bone_duration, 0.0);
    // Force 27.5 doubles to 55 damage: bone 5 (no neighbours) reaches level 4.
    i.bones[5] = contact(27.5);
    let out = s.tick(&i, &[], [false; 5]);
    assert_eq!(out.newly_broken, 1);
    assert_eq!(out.broken_bone_duration, 1.0);
    i.bones[5] = HomBone::default();
    let mut slow = 1;
    while s.tick(&i, &[], [false; 5]).broken_bone_duration > 0.0 {
        slow += 1;
        assert!(slow < 100);
    }
    assert_eq!(slow, BROKEN_TICKS as usize);
    // The level stays; the published level lags one tick behind.
    assert_eq!(s.results().levels[5], 4);
    assert_eq!(s.results().bone_points, 16);
    assert_eq!(s.hud().score, s.results().total as i32 as f32);
    // Leaving the wipeout ends the slow motion at once and republishes.
    let out = s.tick(&input(1), &[], [false; 5]);
    assert!(out.ended && out.published);
    assert_eq!(out.broken_bone_duration, 0.0);
}

#[test]
fn accumulated_damage_decays_and_the_peak_scores() {
    let mut s = Scorer::new(tuning());
    let mut i = input(CATEGORY_WIPEOUT);
    i.bones[7] = contact(3.0); // 6 damage per tick
    for _ in 0..40 {
        s.tick(&i, &[], [false; 5]);
    }
    // After decay acc converges to 0.8 (acc + 6) = 24, so the peak within
    // a tick approaches 30 from below: level 1, never level 2.
    assert!(s.slots[7].damage > 29.9 && s.slots[7].damage < 30.0, "{}", s.slots[7].damage);
    assert_eq!(s.slots[7].level, 1);
}

#[test]
fn a_hard_landing_before_the_bail_hurts_the_planted_leg() {
    let mut s = Scorer::new(tuning());
    let mut i = input(1);
    i.hard_landing_kind = 1;
    i.hard_landing_value = 50.0;
    s.tick(&i, &[], [false; 5]);
    let out = s.tick(&input(CATEGORY_WIPEOUT), &[], [false; 5]);
    assert!(out.started);
    // Landing graph 50 -> 500 damage on the left shin, half on the thigh.
    assert_eq!(s.slots[LEFT_SHIN].damage, 500.0);
    assert_eq!(s.slots[LEFT_THIGH].damage, 250.0);
    assert_eq!(s.slots[RIGHT_SHIN].damage, 0.0);
    assert_eq!(s.slots[LEFT_SHIN].level, 5);
}

#[test]
fn struck_entities_score_once_with_their_kind_points() {
    let mut s = Scorer::new(tuning());
    let i = input(CATEGORY_WIPEOUT);
    let car = EntityHit { id: 7, kind: EntityKind::Vehicle };
    let ped = EntityHit { id: 9, kind: EntityKind::Pedestrian };
    s.tick(&i, &[car, ped], [false; 5]);
    assert_eq!(s.hud().car_hits, 1);
    assert_eq!(s.hud().car_scores[0], 300.0); // the type table's -1 entry
    assert_eq!(s.hud().ped_hits, 1);
    assert_eq!(s.hud().ped_scores[0], 100.0); // the default
    s.tick(&i, &[car], [false; 5]);
    assert_eq!(s.hud().car_hits, 0);
    assert_eq!(s.results().entities[1].1, 300);
    // Nine distinct cars: only eight are kept.
    let cars: Vec<_> = (100..109).map(|id| EntityHit { id, kind: EntityKind::Vehicle }).collect();
    s.tick(&i, &cars, [false; 5]);
    assert_eq!(s.results().entities[1].0.len(), ENTITY_SLOTS);
    assert_eq!(s.hud().car_hits, 7);
}

#[test]
fn airborne_body_segments_score_airtime_and_drop() {
    let mut s = Scorer::new(tuning());
    let mut i = input(CATEGORY_WIPEOUT);
    i.com_position = [0.0, 10.0, 0.0];
    s.tick(&i, &[], [false; 5]);
    // No bone rests on the world: the body is in the air for 60 ticks and
    // falls 6 m, then lands on its back.
    for k in 0..60 {
        i.com_position[1] = 10.0 - 0.1 * (k + 1) as f32;
        s.tick(&i, &[], [false; 5]);
    }
    i.bones[10] = contact(0.0);
    s.tick(&i, &[], [false; 5]);
    let r = s.results();
    // The landing tick still counts (82DAECB8 runs before 82DAEBC0).
    assert_eq!(r.max_air_ticks, 62);
    assert!((r.max_drop - 6.0).abs() < 1e-3, "{}", r.max_drop);
    assert_eq!(r.total_drop_points, 60);
    assert_eq!(r.total_air_points, 10);
    assert_eq!(s.hud().max_air_seconds, 62.0 * TICK);
}

#[test]
fn going_airborne_resets_and_a_held_tweak_scores_once() {
    let mut s = Scorer::new(tuning());
    let mut i = input(CATEGORY_WIPEOUT);
    i.bones[5] = contact(27.5);
    s.tick(&i, &[], [false; 5]);
    assert!(s.bone_points(None) > 0);
    // Airborne (Air) resets the scorer: the next bail starts from zero.
    let mut air = input(CATEGORY_AIR);
    assert!(s.tick(&air, &[], [false; 5]).reset);
    assert_eq!(s.bone_points(None), 0);
    // Holding the stick up in the air scores tweak 0 after its hold time.
    air.stick = [0.0, 1.0];
    for _ in 0..6 {
        s.tick(&air, &[], [false; 5]);
    }
    assert_eq!(s.tweak_points, 50);
    assert_eq!(s.tweaks_scored, 1);
    // Inside the deadzone the hold restarts.
    air.stick = [0.0, 0.2];
    s.tick(&air, &[], [false; 5]);
    assert_eq!(s.tweak_hold, 0);
}

#[test]
fn a_reversing_rotation_banks_its_turn() {
    let mut s = Scorer::new(tuning());
    let mut i = input(CATEGORY_WIPEOUT);
    s.tick(&i, &[], [false; 5]);
    let yaw = |deg: f32| {
        let (sn, c) = deg.to_radians().sin_cos();
        // Pelvis turning about its own z axis.
        [[c, sn, 0.0], [-sn, c, 0.0], [0.0, 0.0, 1.0]]
    };
    let mut angle = 0.0f32;
    for _ in 0..12 {
        angle += 10.0;
        i.pelvis = yaw(angle);
        s.tick(&i, &[], [false; 5]);
    }
    assert!((s.spin_total - 120.0).abs() < 0.5, "{}", s.spin_total);
    assert_eq!(s.spin_sum, 0.0);
    angle -= 10.0;
    i.pelvis = yaw(angle);
    s.tick(&i, &[], [false; 5]);
    assert!((s.spin_sum - 120.0).abs() < 0.5, "{}", s.spin_sum);
    assert!(s.results().max_spin_degrees >= 120.0 - 0.5);
}

#[test]
fn tuning_parses_from_collections_json() {
    let t = tuning();
    let hex = |words: &[u32]| words.iter().map(|w| format!("{w:08X}")).collect::<String>();
    let f = |v: f32| v.to_bits();
    let mut set = Vec::new();
    for b in &t.bones {
        set.push(0);
        for (n, k) in b.neighbours {
            set.push(n as u32);
            set.push(f(k));
        }
        for (p, th) in b.levels {
            set.push(p);
            set.push(f(th));
        }
    }
    let mut fields = serde_json::Map::new();
    let mut put = |name: &str, data: String| {
        fields.insert(name.into(), serde_json::json!({"type": "x", "data": data}));
    };
    put(field::BONES, hex(&set));
    for (i, name) in field::GRAPHS.iter().enumerate() {
        let g = &t.graphs[i];
        let mut w = vec![0, 0, 0, 0];
        w.extend(g.x.iter().map(|v| f(*v)));
        w.extend(g.y.iter().map(|v| f(*v)));
        put(name, hex(&w));
    }
    put(field::POINTS_LIMIT, hex(&[t.points_limit]));
    put(field::LEG_SECONDARY, hex(&[f(t.leg_secondary)]));
    put(field::IMPULSE_SCALE, hex(&[f(t.impulse_scale)]));
    put(field::TWEAK_DEADZONE, hex(&[f(t.tweak_deadzone)]));
    for (k, (_, default)) in field::ENTITIES.iter().enumerate() {
        put(default, hex(&[t.entities[k].default]));
    }
    let tweaks: Vec<String> = t
        .tweaks
        .iter()
        .map(|w| hex(&[w.id, 0, f(w.direction[0]), f(w.direction[1]), w.hold_ticks, w.points]))
        .collect();
    fields.insert(field::TWEAKS.into(), serde_json::json!({"type": "x", "data": "", "array": {"items": tweaks}}));
    for (k, (table, _)) in field::ENTITIES.iter().enumerate() {
        let items: Vec<String> = t.entities[k].table.iter().map(|(a, p)| hex(&[*a as u32, *p])).collect();
        fields.insert((*table).into(), serde_json::json!({"type": "x", "data": "", "array": {"items": items}}));
    }
    let json = serde_json::json!({"collections": [
        {"class": "Other", "key": "default", "fields": {}},
        {"class": field::CLASS, "key": field::KEY, "fields": fields},
    ]});
    assert_eq!(Tuning::from_collections(&json).unwrap(), t);
    let missing = serde_json::json!({"collections": []});
    assert!(Tuning::from_collections(&missing).is_err());
}

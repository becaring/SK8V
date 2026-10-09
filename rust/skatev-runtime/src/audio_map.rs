//! Skate session state to Skate 3's audio game variables (`skate_aems::live::Frame`).

use skate_aems::live::{Frame, frame::Field};

/// Native audio state persists for the lifetime of the physical player.
pub struct State {
    body_contact_surface: [u32; 8],
    impulse_history: [[f32; 8]; 4],
    impulse_slot: usize,
    board_impact_history: [f32; 4],
    board_impact_slot: usize,
    push_left: bool,
    push_right: bool,
    grind_family: u32,
    grind_surface: u32,
    grind_impact: f32,
    grinding: bool,
    landing_level: f32,
    wheel_contact: [bool; 4],
    wheel_air_frames: [u32; 4],
    wheel_landed: [bool; 4],
    wheel_velocity: [[f32; 4]; 4],
    wheel_impact: [f32; 4],
    wheel_class: [u32; 4],
    wheel_air_level: [f32; 4],
    foot_flags: [bool; 2],
    foot_heights: [f32; 2],
    foot_surface: u16,
    foot_events: [bool; 2],
    state_80_frames: u32,
    state_80_latch: bool,
    /// Skater311/312/316: the drag latch's armed flag, clock and arm time.
    drag_armed: bool,
    drag_clock: f32,
    drag_armed_at: f32,
    /// Packet152 bit23, kept while Grinds152 is -1.
    grind_scorable_234: bool,
    /// Skater720.
    filtered_timer: i32,
}
impl Default for State {
    fn default() -> Self {
        Self { body_contact_surface: [0; 8], impulse_history: [[0.0; 8]; 4], impulse_slot: 0,
            board_impact_history: [0.0; 4], board_impact_slot: 0, push_left: false, push_right: false, grind_family: u32::MAX,
            grind_surface: 0, grind_impact: 0.0, grinding: false, landing_level: 0.0,
            wheel_contact: [true; 4], wheel_air_frames: [0; 4], wheel_landed: [false; 4],
            wheel_velocity: [[0.0; 4]; 4], wheel_impact: [0.0; 4],
            wheel_class: [0; 4], wheel_air_level: [0.0; 4],
            foot_flags: [false; 2], foot_heights: [0.0; 2], foot_surface: 0, foot_events: [false; 2],
            state_80_frames: 0, state_80_latch: false,
            drag_armed: false, drag_clock: 0.0, drag_armed_at: 0.0,
            grind_scorable_234: false, filtered_timer: 0 }
    }
}
impl State {
    pub fn frame(&mut self, s: &skate_host::bridge::Session) -> Frame {
        let mut f = self.from_input(&s.audio_input(), s.period());
        // The combo multiplier the HUD shows (82666BC0 reads it each frame).
        f.fields.push((skate_aems::live::ui_sounds::MULTIPLIER_FIELD, Field::F32(s.score().multiplier)));
        f
    }
    fn from_input(&mut self, a: &skate_host::bridge::audio::AudioInput, dt: f32) -> Frame {
        //824B0F80..1018: start reads the PREVIOUS feet flags. Right push
        //does not require contact; left push requires State56 contact.
        let start = !self.push_left && !self.push_right && a.pushing;
        self.push_right = a.pushing && a.push_right_toe;
        self.push_left = a.pushing && !a.push_right_toe && a.push_contact;
        let mut f = from_input(a);
        // Room for the fields appended below.
        f.fields.reserve(128);
        // Owned TU3 Audio conditioner827731C8: retain family/surface only
        // on Grinds316, and retain the last positive impact independently.
        if a.grind_contact != 0 {
            self.grind_family = a.grind_family;
            self.grind_surface = a.grind_surface;
        }
        if a.grind_impact > 0.0 { self.grind_impact = a.grind_impact; }
        let previous_grinding = self.grinding;
        self.grinding = a.state_category == 400 || (a.state_id == 701 && a.grind_flag_323 != 0);
        let v = a.landing_velocity_delta;
        let speed = skate_host::bridge::audio::native_length(v);
        //827727D8..2810, literals821BCD60 and822F8F44.
        let landing_level = if speed < 0.0 { 0.0 }
            else if speed > f32::from_bits(0x4029_999a) { 1.0 }
            else { speed * f32::from_bits(0x3ec1_3521) };
        //827A2284 emits-1 outside Air440;824B1084 preserves the old level.
        if a.landing_level_valid { self.landing_level = landing_level; }
        f.fields.extend([
            (192, Field::U32(self.grind_family)),
            (228, Field::F32(self.grind_impact)),
            (232, Field::F32(a.side_slip)),
            (341, Field::U8(self.grinding as u8)),
            (342, Field::U8(previous_grinding as u8)),
            (468, Field::F32(self.landing_level)),
            (480, Field::F32(a.local_deck_angular_velocity[0])),
            (484, Field::F32(a.local_deck_angular_velocity[1])),
            (488, Field::F32(a.local_deck_angular_velocity[2])),
            (692, Field::U32(surface_index(self.grind_surface))),
            (333, Field::U8(self.push_left as u8)),
            (334, Field::U8(self.push_right as u8)),
            (335, Field::U8(start as u8)),
        ]);
        //Collision80 -> conditioner82773298 -> Audio92 -> packet320 -> 496,
        //116-byte copies. The conditioner stores the current block in a
        //four-entry ring and replaces only the eight impulses (80..108) by
        //their maximum over the ring, compared in ring order.
        self.impulse_history[self.impulse_slot] = a.body_contact_impulse;
        self.impulse_slot = (self.impulse_slot + 1) % 4;
        for i in 0..8 {
            let mut impulse = self.impulse_history[0][i];
            for entry in &self.impulse_history[1..] {
                if entry[i] > impulse { impulse = entry[i]; }
            }
            if a.body_contact_valid[i] { self.body_contact_surface[i] = a.body_contact_surface[i]; }
            f.fields.push((496 + 4 * i, Field::F32(impulse)));
            f.fields.push((528 + 4 * i, Field::F32(a.body_contact_tangent_speed[i])));
            f.fields.push((560 + 4 * i, Field::U32(self.body_contact_surface[i])));
        }
        //Collision24 -> Audio36 -> 668: the same conditioner keeps its own
        //four-entry ring (this644..656, index660) of the board impact level.
        self.board_impact_history[self.board_impact_slot] = a.board_impact_level;
        self.board_impact_slot = (self.board_impact_slot + 1) % 4;
        let mut board_impact = self.board_impact_history[0];
        for &entry in &self.board_impact_history[1..] {
            if entry > board_impact { board_impact = entry; }
        }
        f.fields.extend([
            //Collision20: the deck's slide speed along its contact.
            (664, Field::F32(a.board_slide_speed)),
            (668, Field::F32(board_impact)),
            (596, Field::F32(a.body_group_8_force)),
            (600, Field::F32(a.body_skater_force)),
            (604, Field::U32(a.body_other_skater as u32)),
            (608, Field::F32(a.body_group_11_force)),
        ]);
        for i in 0..4 {
            for j in 0..3 { f.fields.push((384 + 16 * i + 4 * j, Field::F32(a.wheel_position[i][j]))); }
            //82773088..31B4: landed is held until a wheel has spent more
            //than five complete frames off contact, then set on recontact.
            if !self.wheel_contact[i] && self.wheel_air_frames[i] > 5 {
                self.wheel_landed[i] = a.wheel_contact[i] != 0;
                if self.wheel_landed[i] {
                    let v = self.wheel_velocity[i]; let n = a.wheel_normal[i];
                    let closing = -skate_host::bridge::audio::native_dot(v, n);
                    self.wheel_impact[i] = (closing.max(0.0) / a.wheel_impact_scale).min(1.0);
                }
            }
            self.wheel_velocity[i] = a.wheel_velocity[i];
            self.wheel_contact[i] = a.wheel_contact[i] != 0;
            self.wheel_air_frames[i] = if self.wheel_contact[i] { 0 }
                else { self.wheel_air_frames[i].wrapping_add(1) };
            f.fields.push((464 + i, Field::U8(self.wheel_landed[i] as u8)));
            if a.wheel_classes_from_air {
                //824B1198/2350 classifies the PREVIOUS per-wheel airtime;
                //new airtime replaces it only on active air/land/grind.
                let level = if a.air_no_wheel_contact { (a.air_time * 0.5).clamp(0.0, 1.0) } else { 0.0 };
                if level > 0.0 || self.wheel_landed[i] || self.grinding {
                    self.wheel_class[i] = classify(self.wheel_air_level[i], a.wheel_air_thresholds);
                    self.wheel_air_level[i] = level;
                }
            } else if self.wheel_landed[i] {
                //827A21C8..2278 leaves packet class bits unchanged otherwise.
                self.wheel_class[i] = classify(self.wheel_impact[i], a.wheel_impact_thresholds);
            }
            f.fields.push((448 + i * 4, Field::U32(self.wheel_class[i])));
            f.fields.push((620 + i * 4, Field::U32(surface_index(a.part_audio_surfaces[i]))));
            f.fields.push((636 + i * 4, Field::U32(a.wheel_seam_patterns[i])));
        }
        for i in 0..3 {
            f.fields.push((652 + i * 4, Field::U32(surface_index(a.part_audio_surfaces[i + 4]))));
        }
        //827729B8: retain nonzero16-bit foot surfaces and the height at
        //each306/307 rising edge.827A2714 maps selected foot to events2..5.
        if a.foot_surface_tag as u16 != 0 { self.foot_surface = a.foot_surface_tag as u16; }
        for i in 0..2 {
            let current = a.foot_flags[i] != 0;
            if current && !self.foot_flags[i] { self.foot_heights[i] = a.foot_height[i]; }
            self.foot_flags[i] = current;
        }
        let height_difference = self.foot_heights[1] - self.foot_heights[0];
        if height_difference.abs() > f32::from_bits(0x3d8f_5c29) {
            let lower = if self.foot_flags[1] { Some(height_difference > 0.0) }
                else if self.foot_flags[0] { Some(height_difference <= 0.0) } else { None };
            if let Some(first) = lower {
                //82772B38/64 sets one flag without clearing its companion.
                self.foot_events[usize::from(!first)] = true;
            }
        } else {
            self.foot_events = [false; 2];
        }
        let base = if (self.foot_surface >> 7) & 31 == 8 { 4 } else { 2 };
        let event = if self.foot_events[0] { base } else if self.foot_events[1] { base + 1 } else { 1 };
        f.fields.push((740, Field::U32(event)));
        // Fill824B0DA8 footstep variables from packet827A1B78.
        // 716: category of the state (82D2D908) is the off-board biped500.
        f.fields.push((716, Field::U8((category(a.state_id) == 500) as u8)));
        // Packet152 bits2/3: OffBoard307 || Air450 and OffBoard306 || Air449;
        // the fill adds the right push/foot brake (724) and left push (725).
        let right = a.foot_flags[1] != 0 || a.footplant_contact[1] != 0;
        let left = a.foot_flags[0] != 0 || a.footplant_contact[0] != 0;
        f.fields.push((724, Field::U8((right || self.push_right || a.foot_brake) as u8)));
        f.fields.push((725, Field::U8((left || self.push_left) as u8)));
        // Packet200/202 are OffBoard56/52 (both Processed2596), replaced by
        // Air224 while Air448; the fill stores 1 for a zero tag.
        let tag = if a.footplant_active != 0 { a.footplant_surface } else { a.foot_surface_tag } as u16;
        let tag = if tag == 0 { 1 } else { tag };
        f.fields.push((728, Field::U32(foot_surface_index(tag))));
        f.fields.push((732, Field::U32(foot_surface_index(tag))));
        // 768: packet156 bit7, Air448.
        f.fields.push((768, Field::U8((a.footplant_active != 0) as u8)));
        //824B17F0..1838: 776 holds State80 (packet152 bit5) until five
        //frames without it have been counted.
        if a.state_flag_80 {
            self.state_80_latch = true;
            self.state_80_frames = 0;
        } else if self.state_80_frames < 5 {
            self.state_80_frames += 1;
        } else {
            self.state_80_latch = false;
        }
        f.fields.push((776, Field::U8(self.state_80_latch as u8)));
        self.foley_latches(a, dt, &mut f);
        f
    }
    /// Fill824B0DA8's board foley latches from packet827A1B78.
    fn foley_latches(&mut self, a: &skate_host::bridge::audio::AudioInput, dt: f32, f: &mut Frame) {
        // 824B13E0..1470: 310 (drag A) rises once OffBoard309 (unmounting)
        // has lasted longer than (1 - min(max(s208 * 0.12, 0), 1)) * 0.4 s;
        // the clock312 runs always (literals8208EA70 and82181B90).
        let mut drag = false;
        if a.unmounting {
            if !self.drag_armed {
                self.drag_armed = true;
                self.drag_armed_at = self.drag_clock;
            }
            let x = a.board_ground_speed * f32::from_bits(0x3df5_c28f);
            let m = fsel(-x, 0.0, x);
            if self.drag_clock > self.drag_armed_at {
                let m = fsel(1.0 - m, m, 1.0);
                if self.drag_clock - self.drag_armed_at > (1.0 - m) * f32::from_bits(0x3ecc_cccd) {
                    drag = true;
                }
            }
        } else {
            self.drag_armed = false;
        }
        self.drag_clock += dt;
        // Packet152 bit23: Grinds152 is234; the packet keeps it at -1.
        if a.grind_scorable_id != u32::MAX {
            self.grind_scorable_234 = a.grind_scorable_id == 234;
        }
        // 824B1648..166C: 720 reloads to 20 on718 and counts down to 0.
        let filtered = a.filtered_state == 7;
        if filtered {
            self.filtered_timer = 20;
        } else if self.filtered_timer > 0 {
            self.filtered_timer -= 1;
        }
        // 824B1590..15CC: hands over the deck, cleared for an off-board
        // biped (716) not holding its board (OffBoard311).
        let hands = if category(a.state_id) == 500 && !a.holding_board { [false; 2] }
            else { a.hands_over_deck };
        f.fields.extend([
            // Packet164 bit0 / packet160 bit0: OffBoard309/310.
            (309, Field::U8(a.unmounting as u8)),
            (310, Field::U8(drag as u8)),
            (320, Field::U8(a.kickout_dismount as u8)),
            (372, Field::U8(self.grind_scorable_234 as u8)),
            // Packet148 bit3: Skateboard200 (wheel hardness) below 0.5.
            (684, Field::U32(!(a.wheel_hardness >= 0.5) as u32)),
            (688, Field::U8(hands[0] as u8)),
            (689, Field::U8(hands[1] as u8)),
            (718, Field::U8(filtered as u8)),
            (720, Field::U32(self.filtered_timer as u32)),
        ]);
    }
}

fn classify(value: f32, thresholds: [f32; 2]) -> u32 {
    if !(value < thresholds[1]) { 2 } else if value >= thresholds[0] { 1 } else { 0 }
}

/// 82D2D908: a state ID's hundred (100..700); below 100 is 0.
fn category(state: u32) -> u32 {
    let s = state as i32;
    for c in [700, 600, 500, 400, 300, 200] {
        if s >= c { return c as u32; }
    }
    if s >= 100 { 100 } else { 0 }
}

/// 824B1850..1898: the low seven bits of a foot tag, 1..144 to the zero-based
/// table, anything else 143.
fn foot_surface_index(tag: u16) -> u32 {
    let v = u32::from(tag & 0x7F);
    if v == 0 || v - 1 > 143 { 143 } else { v - 1 }
}

//827A24B0..24D8: authored IDs1..144 map to the zero-based audio table.
fn surface_index(raw: u32) -> u32 {
    if (1..=144).contains(&raw) { raw - 1 } else { 143 }
}

/// PowerPC `fsel`: `b` when `a >= 0`, else `c` (NaN selects `c`).
fn fsel(a: f32, b: f32, c: f32) -> f32 {
    if a >= 0.0 { b } else { c }
}

/// `fsel` on `a - b`: `a` unless it is smaller.
fn larger(a: f32, b: f32) -> f32 {
    if a - b >= 0.0 { a } else { b }
}

fn from_input(a: &skate_host::bridge::audio::AudioInput) -> Frame {
    // Source offsets: examples/gv_skater.rs; no inferred physics or gains.
    // The remaining audio-owned latch/contact/trick fields need their own
    // producers before this partial publication can drive the full engine.
    Frame { fields: vec![
        //Host-only input extension; offset1000 is not a native Skater field.
        (1000, Field::U32(a.button_mask)),
        //827A1D28..3C packs Collision0;824B0F24 extracts the wheel count.
        (200, Field::U32(a.wheel_contact_count & 7)),
        (204, Field::F32(a.turn)),
        (208, Field::F32(a.board_ground_speed)),
        //SystemReckoning vector_16 (centre-of-mass velocity) -> Skater96..108.
        (96, Field::F32(a.com_velocity[0])),
        (100, Field::F32(a.com_velocity[1])),
        (104, Field::F32(a.com_velocity[2])),
        (108, Field::F32(a.com_velocity[3])),
        (212, Field::F32(a.com_velocity[..3].iter().map(|v| v * v).sum::<f32>().sqrt())),
        //Normal GTA free-skate runs at real time; no Skate replay time scale.
        (220, Field::F32(1.0)),
        (236, Field::F32(a.air_time)),
        (240, Field::F32(a.air_scalar_184)),
        (260, Field::F32(a.jump_height)),
        (308, Field::U8(a.offboard)),
        (328, Field::F32(a.root_angular_speed)),
        (332, Field::U8(a.air_no_wheel_contact as u8)),
        (336, Field::U8(a.foot_brake as u8)),
        (337, Field::U8(a.push_contact as u8)),
        (338, Field::U8(a.push_right_toe as u8)),
        (339, Field::U8(a.manual_brake as u8)),
        (340, Field::U8(a.manual as u8)),
        (343, Field::U8(a.trick.is_some() as u8)),
        (344, Field::U8(a.trick.is_some_and(|t|t.attribute) as u8)),
        (348, Field::U32(a.trick.map_or(-1,|t|t.ids[0]) as u32)),
        (352, Field::U32(a.trick.map_or(-1,|t|t.ids[1]) as u32)),
        //827A2588..2604 (packet288..308) and fill824B1784..17A0:
        //PhysOutSkeleton192/208 toe velocities, |y| and the larger |x|/|z|.
        (268, Field::F32(a.foot_local_velocity[1][1].abs())),
        (272, Field::F32(a.foot_local_velocity[0][1].abs())),
        (276, Field::F32(larger(a.foot_local_velocity[1][0].abs(), a.foot_local_velocity[1][2].abs()))),
        (280, Field::F32(larger(a.foot_local_velocity[0][0].abs(), a.foot_local_velocity[0][2].abs()))),
        (592, Field::U8(a.body_specific_contacts[0] as u8)),
        (593, Field::U8(a.body_specific_contacts[1] as u8)),
        //Packet148 bit8, Collision3475 (deck contact).
        (614, Field::U8((a.deck_contact != 0) as u8)),
        (615, Field::U8(a.left_foot_within_deck)),
        (616, Field::U8(a.right_foot_within_deck)),
        //827A1F20..1F58, literal820C6D98 is0.25; preserve addition order.
        (672, Field::F32(((a.limb_relative_speeds[3] + a.limb_relative_speeds[2])
            + a.limb_relative_speeds[1] + a.limb_relative_speeds[0]) * 0.25)),
        (676, Field::U8(a.wiping_out as u8)),
        (677, Field::U8(a.skeleton_over)),
        (690, Field::U8(a.revert as u8)),
        //Packet152 bit5, State80.
        (769, Field::U8(a.state_flag_80 as u8)),
        //Packet172 bit27: State16 is503.
        (814, Field::U8((a.state_id == 503) as u8)),
    ], ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skate_aems::live::frame::SkaterImage;
    use skate_host::bridge::audio::AudioInput;

    #[test]
    fn body_motion_and_score_metadata_keep_native_meanings() {
        let mut a = AudioInput::default();
        a.body_specific_contacts = [false, true];
        a.root_angular_speed = 3.5;
        a.limb_relative_speeds = [1.0, 2.0, 3.0, 6.0];
        let f = SkaterImage::from_frame(&from_input(&a));
        assert_eq!((f.r8(592), f.r8(593), f.rf(328), f.rf(672)), (0, 1, 3.5, 3.0));
        assert_eq!((f.r8(343), f.r32(348), f.r32(352)), (0, u32::MAX, u32::MAX));
        a.trick = Some(skate_host::bridge::audio::TrickAudio { ids: [12, -1], attribute: true });
        let f = SkaterImage::from_frame(&from_input(&a));
        assert_eq!((f.r8(343), f.r8(344), f.r32(348), f.r32(352)), (1, 1, 12, u32::MAX));
    }

    #[test]
    fn retained_grind_and_gated_landing_follow_native_lifetimes() {
        let mut state = State::default();
        let mut a = AudioInput::default();
        let f = SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0));
        assert_eq!(f.r32(192), u32::MAX);
        assert_eq!(f.r32(692), 143);
        a.grind_contact = 1; a.grind_family = 3; a.grind_surface = 8;
        a.grind_impact = 2.5; a.state_category = 400;
        a.landing_level_valid = true; a.landing_velocity_delta = [4.0, 0.0, 0.0, 0.0];
        let f = SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0));
        assert_eq!((f.r32(192), f.r32(692)), (3, 7));
        assert_eq!((f.r8(341), f.r8(342), f.rf(228), f.rf(468)), (1, 0, 2.5, 1.0));
        a.grind_contact = 0; a.grind_family = 5; a.grind_surface = 99;
        a.grind_impact = -1.0; a.state_category = 0;
        a.landing_level_valid = false; a.landing_velocity_delta = [0.0; 4];
        let f = SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0));
        assert_eq!((f.r32(192), f.r32(692)), (3, 7));
        assert_eq!((f.r8(341), f.r8(342), f.rf(228), f.rf(468)), (0, 1, 2.5, 1.0));
        a.state_id = 701; a.grind_flag_323 = 1;
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r8(341), 1);
    }

    #[test]
    fn wheel_landed_requires_six_complete_air_frames_and_holds_contact() {
        let mut state = State::default();
        let mut a = AudioInput::default();
        for _ in 0..6 { assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r8(464), 0); }
        a.wheel_contact[0] = 1;
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r8(464), 1);
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r8(464), 1);
        a.wheel_contact[0] = 0;
        for _ in 0..6 { assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r8(464), 1); }
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r8(464), 0);
    }

    #[test]
    fn footstep_heights_are_captured_on_rising_edges_and_surface_eight_changes_bank() {
        let mut state = State::default();
        let mut a = AudioInput { foot_flags: [1, 1], foot_height: [0.0, 0.1],
            foot_surface_tag: 8 << 7, ..Default::default() };
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r32(740), 4);
        a.foot_height = [0.0, -1.0]; //Held flag does not recapture height.
        a.foot_surface_tag = 0; //Zero preserves previous tag.
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r32(740), 4);
        a.foot_flags = [0, 0]; state.from_input(&a, 1.0 / 60.0);
        a.foot_flags = [1, 1]; a.foot_surface_tag = 3 << 7;
        // Both native event flags now hold; the first flag wins packet selection.
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r32(740), 2);
        a.foot_flags = [0, 0]; state.from_input(&a, 1.0 / 60.0);
        a.foot_flags = [1, 1]; a.foot_height = [0.0; 2];
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r32(740), 1);
    }

    #[test]
    fn board_foley_producers_follow_packet_and_fill() {
        let mut state = State::default();
        let mut a = AudioInput {
            foot_local_velocity: [[-3.0, -0.5, 2.0, 0.0], [1.0, 0.25, -4.0, 0.0]],
            deck_contact: 1, state_flag_80: true, state_id: 503, ..Default::default() };
        let f = SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0));
        assert_eq!((f.rf(268), f.rf(272), f.rf(276), f.rf(280)), (0.25, 0.5, 4.0, 3.0));
        assert_eq!((f.r8(614), f.r8(769), f.r8(776), f.r8(814)), (1, 1, 1, 1));
        a.state_flag_80 = false;
        // 824B17F0: five counted frames, cleared on the sixth.
        for _ in 0..5 {
            let f = SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0));
            assert_eq!((f.r8(769), f.r8(776)), (0, 1));
        }
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0)).r8(776), 0);
    }

    #[test]
    fn body_force_clears_but_material_holds_when_contact_ends() {
        let mut state = State::default();
        let mut a = AudioInput::default();
        a.body_contact_valid[2] = true;
        a.body_contact_tangent_speed[2] = 3.5;
        a.body_contact_impulse[2] = 0.75;
        a.body_contact_surface[2] = 19;
        a.body_skater_force = 2.0;
        a.body_other_skater = -1;
        a.board_impact_level = 0.5;
        a.board_slide_speed = 1.25;
        let f = SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0));
        assert_eq!((f.rf(504), f.rf(536), f.r32(568)), (0.75, 3.5, 19));
        assert_eq!((f.rf(664), f.rf(668)), (1.25, 0.5));
        assert_eq!((f.rf(600), f.r32(604)), (2.0, u32::MAX));
        a.body_contact_valid[2] = false;
        a.body_contact_tangent_speed[2] = 0.0;
        a.body_contact_impulse[2] = 0.0;
        a.body_contact_surface[2] = 0;
        a.board_impact_level = 0.0;
        a.board_slide_speed = 0.0;
        // 82773298: the impulse and board level are four-entry ring maxima.
        for _ in 0..3 {
            let f = SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0));
            assert_eq!((f.rf(504), f.rf(536), f.r32(568)), (0.75, 0.0, 19));
            assert_eq!((f.rf(664), f.rf(668)), (0.0, 0.5));
        }
        let f = SkaterImage::from_frame(&state.from_input(&a, 1.0 / 60.0));
        assert_eq!((f.rf(504), f.rf(668)), (0.0, 0.0));
    }

    #[test]
    fn native_push_latch_uses_previous_feet_and_asymmetric_contact() {
        let mut state = State::default();
        let mut a = AudioInput::default();
        let image = |f: Frame| SkaterImage::from_frame(&f);
        a.pushing = true;
        a.push_right_toe = true;
        let first = image(state.from_input(&a, 1.0 / 60.0));
        assert_eq!([first.r8(333), first.r8(334), first.r8(335)], [0, 1, 1]);
        let held = image(state.from_input(&a, 1.0 / 60.0));
        assert_eq!(held.r8(335), 0);
        a.push_right_toe = false;
        let switch = image(state.from_input(&a, 1.0 / 60.0));
        assert_eq!([switch.r8(333), switch.r8(334), switch.r8(335)], [0, 0, 0]);
        // Native start can repeat while left foot has no contact. Preserve it.
        let no_contact = image(state.from_input(&a, 1.0 / 60.0));
        assert_eq!(no_contact.r8(335), 1);
        a.push_contact = true;
        let left = image(state.from_input(&a, 1.0 / 60.0));
        assert_eq!([left.r8(333), left.r8(334), left.r8(335)], [1, 0, 1]);
        assert_eq!(image(state.from_input(&a, 1.0 / 60.0)).r8(335), 0);
    }

    #[test]
    fn foley_latches_follow_fill() {
        let mut state = State::default();
        let dt = 0.1;
        // Stationary unmount: the drag needs more than 0.4 s of OffBoard309.
        let mut a = AudioInput { unmounting: true, wheel_hardness: 0.75,
            grind_scorable_id: 234, filtered_state: 7, hands_over_deck: [true, false],
            state_id: 503, ..Default::default() };
        let f = SkaterImage::from_frame(&state.from_input(&a, dt));
        assert_eq!((f.r8(309), f.r8(310), f.r32(684), f.r8(372)), (1, 0, 0, 1));
        assert_eq!((f.r8(688), f.r8(689), f.r8(718), f.r32(720)), (0, 0, 1, 20));
        a.holding_board = true;
        a.grind_scorable_id = u32::MAX;
        a.filtered_state = 0;
        let mut drags = Vec::new();
        for _ in 0..5 {
            let f = SkaterImage::from_frame(&state.from_input(&a, dt));
            assert_eq!((f.r8(688), f.r8(372)), (1, 1));
            drags.push(f.r8(310));
        }
        // Clock 0.1..0.5 since arming at 0: strictly more than 0.4.
        assert_eq!(drags, [0, 0, 0, 0, 1]);
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, dt)).r32(720), 14);
        // Fast unmount (speed >= 1/0.12) drags as soon as the clock moves.
        let mut state = State::default();
        a.board_ground_speed = 10.0;
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, dt)).r8(310), 0);
        assert_eq!(SkaterImage::from_frame(&state.from_input(&a, dt)).r8(310), 1);
        a.unmounting = false;
        a.grind_scorable_id = 12;
        a.wheel_hardness = f32::NAN;
        let f = SkaterImage::from_frame(&state.from_input(&a, dt));
        assert_eq!((f.r8(310), f.r8(372), f.r32(684)), (0, 0, 1));
    }

    #[test]
    fn publication_preserves_typed_source_values_and_omits_missing_producers() {
        let a = AudioInput {
            board_ground_speed: 7.25,
            com_velocity: [3.0, 4.0, 0.0, 99.0],
            left_foot_within_deck: 1,
            right_foot_within_deck: 0,
            skeleton_over: 1,
            wiping_out: true,
            revert: true,
            ..Default::default()
        };
        let frame = from_input(&a);
        let image = SkaterImage::from_frame(&frame);
        assert_eq!(image.rf(208), 7.25);
        assert_eq!(image.rf(212), 5.0);
        assert_eq!((image.rf(96), image.rf(100), image.rf(108)), (3.0, 4.0, 99.0));
        assert_eq!(image.r8(615), 1);
        assert_eq!(image.r8(616), 0);
        assert_eq!(image.r8(677), 1);
        assert_eq!(image.r8(676), 1);
        assert_eq!(image.r8(690), 1);
        assert_eq!(image.r32(348), u32::MAX);
        assert_eq!(image.rf(220), 1.0);
        assert_eq!((image.r8(614), image.r8(769), image.r8(814)), (0, 0, 0));
        for missing in [192, 228, 232, 341, 384, 448, 464, 468,
                        480, 528, 560, 620, 636, 692, 740] {
            assert!(!frame.fields.iter().any(|(offset, _)| *offset == missing));
        }
    }
}

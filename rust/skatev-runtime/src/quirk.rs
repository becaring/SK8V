//! Retail Backwards Man stages the owner-recorded controller routine, including
//! Skate's marker return. Board retrieval, hold whip and remount supply the
//! speed through ordinary Skate inputs. See backwards-man-retail.md.
use bevy_math::Vec3;

mod retail_22;

pub const BUTTON_Y: u16 = 0x8000;
pub const BUTTON_X: u16 = 0x4000;
pub const BUTTON_LEFT_THUMB: u16 = 0x0040;
pub const BUTTON_RIGHT_THUMB: u16 = 0x0080;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetailQuirk {
    BackwardsMan,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackwardsManConfig {
    pub enabled: bool,
    /// XInput button mask that must be fully pressed (rising edge) to trigger.
    pub chord: u16,
}

impl Default for BackwardsManConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            chord: BUTTON_LEFT_THUMB | BUTTON_RIGHT_THUMB,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle = 0,
    /// Control is the player's again after the routine; reported only.
    Flight = 6,
    RetailSetup = 7,
    RetailReplay = 8,
}

/// What the assist sees of Skate each tick.
#[derive(Clone, Copy, Debug)]
pub struct Observation<'a> {
    pub state: &'a str,
    /// Skater root position and velocity, Skate world (Y up).
    pub position: Vec3,
    pub velocity: Vec3,
}

/// Controller frame the assist substitutes for the player's while active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Injected {
    pub buttons: u16,
    pub right_trigger: u8,
    pub left_trigger: u8,
    pub left_stick: [i16; 2],
    pub right_stick: [i16; 2],
}

/// Side effects for the worker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    None,
    /// Disarm Skate's off-board launch override (sequence aborted).
    Disarm,
}

pub struct BackwardsManAssist {
    pub config: BackwardsManConfig,
    phase: Phase,
    ticks: u32,
    chord_down: bool,
    /// Retail setup progress (event-driven): step, ticks in it, marker point.
    setup: SetupStep,
    setup_ticks: u32,
    ticks_standing: u32,
    marker: Vec3,
    pub log: Vec<String>,
}

/// Retail setup, each step waiting only for the Skate state it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupStep {
    /// Off the board and standing (BipedGround held `STAND_TICKS`).
    Dismount,
    /// Set Skate's session marker (LB + d-pad down, 11 frames as recorded).
    Marker,
    /// Walk until beyond the marker-return minimum distance.
    Walk,
    /// Come to rest before the recorded routine.
    Settle,
}

const FLIGHT_REPORT: u32 = 240;
/// BipedGround held this long before the marker is set (the dismount has
/// finished; a marker during the step-off is not placed).
const STAND_TICKS: u32 = 10;
/// Skate refuses a marker return within 0.5 m of the marker.
const MARKER_CLEARANCE: f32 = 0.6;
/// The skater counts as at rest below this horizontal speed (m/s).
const REST_SPEED: f32 = 0.2;
/// Any setup step taking longer than this aborts the sequence.
const SETUP_STEP_TIMEOUT: u32 = 240;

impl BackwardsManAssist {
    pub fn new(config: BackwardsManConfig) -> Self {
        Self {
            config,
            phase: Phase::Idle,
            ticks: 0,
            chord_down: false,
            setup: SetupStep::Dismount,
            setup_ticks: 0,
            ticks_standing: 0,
            marker: Vec3::ZERO,
            log: Vec::new(),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// True while the assist owns the controller.
    pub fn driving(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::Flight)
    }

    /// Feeds the player's raw buttons; returns true on an intentional trigger
    /// (every chord button newly held together, assist enabled and idle).
    pub fn check_trigger(&mut self, buttons: u16) -> bool {
        let held = self.config.chord != 0 && buttons & self.config.chord == self.config.chord;
        let rising = held && !self.chord_down;
        self.chord_down = held;
        if rising && self.config.enabled && matches!(self.phase, Phase::Idle | Phase::Flight) {
            self.start("chord");
            return true;
        }
        false
    }

    /// Host-requested trigger (keyboard shortcut).
    pub fn request(&mut self) -> bool {
        if self.config.enabled && matches!(self.phase, Phase::Idle | Phase::Flight) {
            self.start("host");
            return true;
        }
        false
    }

    fn start(&mut self, source: &str) {
        self.log.push(format!("BackwardsMan triggered ({source})"));
        self.enter(Phase::RetailSetup);
    }

    fn enter(&mut self, phase: Phase) {
        self.log.push(format!(
            "BackwardsMan phase {:?} -> {:?}",
            self.phase, phase
        ));
        self.phase = phase;
        self.ticks = 0;
        self.setup = SetupStep::Dismount;
        self.setup_ticks = 0;
        self.ticks_standing = 0;
    }

    fn abort(&mut self, why: &str) -> Event {
        self.log.push(format!("BackwardsMan aborted: {why}"));
        self.enter(Phase::Idle);
        Event::Disarm
    }

    /// One Skate tick: the injected controller frame (while driving) and any
    /// side effect for the worker.
    pub fn step(&mut self, obs: Observation<'_>) -> (Option<Injected>, Event) {
        self.ticks += 1;
        let t = self.ticks;
        match self.phase {
            Phase::Idle => (None, Event::None),
            Phase::RetailSetup => {
                // The recorded routine's preconditions: off the board and
                // standing, a marker set, > 0.5 m away from it (Skate's marker
                // return minimum), at rest. Each step waits only for its own
                // condition instead of the original fixed 260-tick schedule.
                self.setup_ticks += 1;
                let st = self.setup_ticks;
                if st > SETUP_STEP_TIMEOUT {
                    return (None, self.abort(&format!("retail setup step {:?} timed out", self.setup)));
                }
                let mut input = Injected::default();
                let mut next = None;
                match self.setup {
                    SetupStep::Dismount => {
                        if !obs.state.starts_with("Biped") {
                            // Y (dismount); re-press if a press was eaten.
                            if st % 40 == 1 {
                                input.buttons = BUTTON_Y;
                            }
                        } else if obs.state == "BipedGround" {
                            self.ticks_standing = self.ticks_standing.saturating_add(1);
                            if self.ticks_standing >= STAND_TICKS {
                                next = Some(SetupStep::Marker);
                            }
                        } else {
                            self.ticks_standing = 0;
                        }
                    }
                    SetupStep::Marker => {
                        input.buttons = 0x0100 | if st == 6 { 0x0002 } else { 0 };
                        if st >= 11 {
                            self.marker = obs.position;
                            next = Some(SetupStep::Walk);
                        }
                    }
                    SetupStep::Walk => {
                        let gone = Vec3::new(obs.position.x - self.marker.x, 0.0, obs.position.z - self.marker.z).length();
                        if gone >= MARKER_CLEARANCE {
                            next = Some(SetupStep::Settle);
                        } else {
                            input.left_stick = [0, 32000];
                        }
                    }
                    SetupStep::Settle => {
                        if Vec3::new(obs.velocity.x, 0.0, obs.velocity.z).length() < REST_SPEED {
                            self.log.push(format!("BackwardsMan retail setup done in {t} ticks"));
                            self.enter(Phase::RetailReplay);
                            return (Some(input), Event::None);
                        }
                    }
                }
                if let Some(n) = next {
                    self.setup = n;
                    self.setup_ticks = 0;
                    self.ticks_standing = 0;
                }
                (Some(input), Event::None)
            }
            Phase::RetailReplay => {
                let mut frame = t - 1;
                // Stop before the recording's NEXT attempt/marker return; give
                // control back after the successful remount has settled.
                for &(count, buttons, left_trigger, right_trigger, left_stick, right_stick) in retail_22::RETAIL_22 {
                    if frame < count {
                        return (Some(Injected { buttons, left_trigger, right_trigger, left_stick, right_stick }), Event::None);
                    }
                    frame -= count;
                }
                self.enter(Phase::Flight);
                (None, Event::None)
            }
            Phase::Flight => {
                // Control is the player's again; the phase is only reported.
                if t > FLIGHT_REPORT || obs.state == "PhysicsGround" && t > 10 {
                    self.enter(Phase::Idle);
                }
                (None, Event::None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retail_default_stages_marker_and_never_arms_velocity() {
        let mut a = BackwardsManAssist::new(BackwardsManConfig::default());
        a.request();
        let (mut marker, mut respawn, mut walk, mut jump, mut recall) =
            (false, false, false, false, false);
        // A skater that walks 3 m/s while the stick is forward.
        let (mut position, mut velocity) = (Vec3::ZERO, Vec3::ZERO);
        for _ in 0..500 {
            let mut o = obs("BipedGround");
            o.position = position;
            o.velocity = velocity;
            let (i, event) = a.step(o);
            velocity = if i.is_some_and(|i| i.left_stick[1] > 16000) { Vec3::Z * 3.0 } else { Vec3::ZERO };
            position += velocity / 60.0;
            assert_eq!(
                event,
                Event::None,
                "Retail must never arm a launch override"
            );
            if let Some(i) = i {
                marker |= i.buttons == 0x0102;
                respawn |= i.buttons == 0x0101;
                walk |= i.left_stick[1] == 32000;
                jump |= i.buttons == BUTTON_X;
                recall |= i.buttons == BUTTON_Y;
            }
        }
        assert!(marker && respawn && walk && jump && recall);
        assert!(!a.driving());
        assert!(a.log.iter().any(|l| l.starts_with("BackwardsMan retail setup done in")), "{:?}", a.log);
    }

    fn obs(state: &str) -> Observation<'_> {
        Observation {
            state,
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
        }
    }

    #[test]
    fn ordinary_play_never_triggers() {
        let mut a = BackwardsManAssist::new(BackwardsManConfig::default());
        // Every single button and common pairs, including each chord half.
        for b in 0..16u16 {
            assert!(!a.check_trigger(1 << b) || (1 << b) & a.config.chord == a.config.chord);
            assert!(!a.check_trigger(0));
        }
        for pair in [
            0x1000 | 0x4000,
            0x0100 | 0x0200,
            BUTTON_LEFT_THUMB,
            BUTTON_RIGHT_THUMB,
            0x8000 | 0x0040,
        ] {
            assert!(!a.check_trigger(pair), "{pair:#x} must not trigger");
            a.check_trigger(0);
        }
        assert_eq!(a.phase(), Phase::Idle);
    }

    #[test]
    fn chord_triggers_once_per_press_and_respects_disable() {
        let mut a = BackwardsManAssist::new(BackwardsManConfig::default());
        let chord = a.config.chord;
        assert!(a.check_trigger(chord));
        assert!(!a.check_trigger(chord), "held chord is not a new trigger");
        let mut off = BackwardsManAssist::new(BackwardsManConfig {
            enabled: false,
            ..Default::default()
        });
        assert!(!off.check_trigger(chord));
        assert!(!off.request());
    }
}

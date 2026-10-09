//! Seamless GTA <-> Skate lifecycle, Skate side.
//!
//! Entry: the session is prepared in the background while GTA runs; the host
//! enters Skate with the skater standing off the board, board in hand
//! (overlay patch 0007 `Session::activate_offboard`, Skate's own off-board
//! manual return). The board remains held until the player presses Y;
//! Skate's state graph owns mounting and dismounting.
//!
//! Exit: a voluntary dismount is the player's own Y while on the board. Skate
//! performs it (`PhysicsGround -> GroundAnimation -> BipedGround`, board held);
//! once the biped has settled on the ground with the board in hand the monitor
//! reports `Phase::Released` while Skate remains active. Only the host's
//! put-away action returns control to GTA, preserving the session. A bail
//! cancels a pending dismount: Skate keeps authority through its own
//! wipeout/recovery until the player remounts and dismounts again.
//!
//! The monitor only reads Skate's published state (`LifecycleView`) and the
//! player's raw buttons; it injects no input.

/// XInput Y: Skate 3's board button (mount / dismount).
pub const BUTTON_Y: u16 = 0x8000;

/// Skate `PhysicalStateId` values used here (skate-core `player::state`).
pub mod state {
    pub const PHYSICS_GROUND: u32 = 100;
    pub const GROUND_ANIMATION: u32 = 103;
    pub const PHYSICS_AIR: u32 = 200;
    pub const WIPEOUT_GROUND: u32 = 300;
    pub const BIPED_GROUND: u32 = 500;
    pub const BIPED_AIR: u32 = 501;
    pub const OFFBOARD_PUSHING: u32 = 502;
    pub const TELEPORTING: u32 = 702;
}

/// SkateboardController +448: the skater holds the board.
pub const BOARD_HELD: u32 = 1;

/// Riding states: on the board (ground, air, grinds, plants).
pub fn on_board(s: u32) -> bool {
    matches!(s / 100, 1 | 2 | 4 | 6)
}

/// Off the board on foot (Skate's biped).
pub fn off_board(s: u32) -> bool {
    matches!(
        s,
        state::BIPED_GROUND | state::BIPED_AIR | state::OFFBOARD_PUSHING
    )
}

/// Skate's state as the monitor sees it each tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub state: u32,
    pub board_possession: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum Phase {
    /// Skate not running (host owns the player).
    #[default]
    Inactive = 0,
    /// Skate owns the player (riding, or off-board after a bail).
    Riding = 2,
    /// The player pressed Y on the board; waiting for Skate's biped to settle.
    Dismounting = 3,
    /// Off the board with the board held; Skate remains active.
    Released = 4,
}

/// Ticks off the board, on the ground, board held, before release (~0.25 s).
pub const LEAVE_SETTLE: u32 = 15;
/// Give up on a dismount that never reaches the settled biped.
pub const DISMOUNT_TIMEOUT: u32 = 300;

#[derive(Default)]
pub struct Monitor {
    phase: Phase,
    ticks: u32,
    settled: u32,
    left_board: bool,
    y_down: bool,
    last: View,
    pub log: Vec<String>,
}

impl Monitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    fn enter(&mut self, phase: Phase, why: &str) {
        if phase != self.phase {
            self.log.push(format!(
                "lifecycle {:?} -> {:?} ({why}; Skate state {} board {})",
                self.phase, phase, self.last.state, self.last.board_possession
            ));
        }
        self.phase = phase;
        self.ticks = 0;
        self.settled = 0;
        self.left_board = false;
    }

    /// Take the board out; the player's Y controls mounting and dismounting.
    pub fn begin_held(&mut self) {
        self.y_down = false;
        self.enter(Phase::Released, "board taken out; waiting for player");
    }

    /// Legacy on-board entry (`Session::activate`).
    pub fn begin_onboard(&mut self) {
        self.enter(Phase::Riding, "on-board entry");
    }

    pub fn stop(&mut self) {
        self.enter(Phase::Inactive, "host deactivated");
    }

    /// The player's raw buttons for one host frame, as forwarded to Skate.
    /// A Y press while on the board asks for a voluntary dismount.
    pub fn player_buttons(&mut self, buttons: u16) {
        let down = buttons & BUTTON_Y != 0;
        let rising = down && !self.y_down;
        self.y_down = down;
        if rising && self.phase == Phase::Riding && on_board(self.last.state) {
            self.enter(Phase::Dismounting, "player pressed Y on the board");
        }
    }

    /// One Skate tick, before it advances: Skate's current state in.
    pub fn step(&mut self, view: View) {
        self.last = view;
        self.ticks += 1;
        let t = self.ticks;
        match self.phase {
            Phase::Inactive | Phase::Riding => {}
            Phase::Released => {
                // Off-board remains inside Skate until the host put-away
                // chord. A normal Y mount resumes riding without activation.
                if on_board(view.state) {
                    self.enter(Phase::Riding, "player remounted");
                }
            }
            Phase::Dismounting => {
                if view.state == state::WIPEOUT_GROUND || view.state == state::TELEPORTING {
                    self.enter(Phase::Riding, "bail during the dismount; Skate recovers");
                    return;
                }
                if off_board(view.state) {
                    self.left_board = true;
                } else if self.left_board && on_board(view.state) {
                    self.enter(Phase::Riding, "back on the board");
                    return;
                }
                if view.state == state::BIPED_GROUND && view.board_possession == BOARD_HELD {
                    self.settled += 1;
                    if self.settled >= LEAVE_SETTLE {
                        self.enter(Phase::Released, "off the board, board in hand");
                    }
                } else {
                    self.settled = 0;
                }
                if self.phase == Phase::Dismounting && t > DISMOUNT_TIMEOUT {
                    self.enter(
                        Phase::Riding,
                        "dismount did not settle with the board in hand",
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_out_waits_for_player_and_remount_keeps_session_active() {
        let mut m = Monitor::new();
        m.begin_held();
        for _ in 0..240 {
            m.step(v(state::BIPED_GROUND, BOARD_HELD));
            assert_eq!(m.phase(), Phase::Released);
        }
        m.player_buttons(BUTTON_Y);
        m.step(v(state::PHYSICS_GROUND, 2));
        assert_eq!(m.phase(), Phase::Riding);
    }

    fn v(state: u32, board: u32) -> View {
        View {
            state,
            board_possession: board,
        }
    }

    #[test]
    fn voluntary_dismount_releases_after_settling_with_board() {
        let mut m = Monitor::new();
        m.begin_onboard();
        m.step(v(state::PHYSICS_GROUND, 0));
        m.player_buttons(BUTTON_Y);
        assert_eq!(m.phase(), Phase::Dismounting);
        m.player_buttons(BUTTON_Y); // held: no new edge
        m.step(v(state::GROUND_ANIMATION, 0));
        for i in 0..LEAVE_SETTLE {
            assert_eq!(m.phase(), Phase::Dismounting, "tick {i}");
            m.step(v(state::BIPED_GROUND, BOARD_HELD));
        }
        assert_eq!(m.phase(), Phase::Released);
        m.player_buttons(0);
        m.player_buttons(BUTTON_Y);
        m.step(v(state::PHYSICS_GROUND, 0));
        assert_eq!(m.phase(), Phase::Riding, "normal remount reuses the same active monitor");
    }

    #[test]
    fn y_off_the_board_is_a_mount_not_a_dismount() {
        let mut m = Monitor::new();
        m.begin_onboard();
        m.step(v(state::BIPED_GROUND, BOARD_HELD)); // e.g. after a bail
        m.player_buttons(BUTTON_Y);
        assert_eq!(m.phase(), Phase::Riding);
        for _ in 0..100 {
            m.step(v(state::BIPED_GROUND, BOARD_HELD));
        }
        assert_eq!(
            m.phase(),
            Phase::Riding,
            "never released without a dismount"
        );
    }

    #[test]
    fn bail_cancels_dismount_and_skate_keeps_authority() {
        let mut m = Monitor::new();
        m.begin_onboard();
        m.step(v(state::PHYSICS_AIR, 0));
        m.player_buttons(BUTTON_Y);
        assert_eq!(m.phase(), Phase::Dismounting);
        m.step(v(state::WIPEOUT_GROUND, 0));
        assert_eq!(m.phase(), Phase::Riding);
        for _ in 0..100 {
            m.step(v(state::BIPED_GROUND, BOARD_HELD));
        }
        assert_eq!(m.phase(), Phase::Riding);
    }

    #[test]
    fn released_board_never_releases_the_player() {
        let mut m = Monitor::new();
        m.begin_onboard();
        m.step(v(state::PHYSICS_GROUND, 0));
        m.player_buttons(BUTTON_Y);
        for _ in 0..DISMOUNT_TIMEOUT + 5 {
            m.step(v(state::BIPED_GROUND, 2));
        }
        assert_eq!(
            m.phase(),
            Phase::Riding,
            "board let go: Skate keeps authority"
        );
    }

    #[test]
    fn remount_during_dismount_cancels() {
        let mut m = Monitor::new();
        m.begin_onboard();
        m.step(v(state::PHYSICS_GROUND, 0));
        m.player_buttons(BUTTON_Y);
        m.step(v(state::BIPED_GROUND, 2));
        m.step(v(state::PHYSICS_GROUND, 0));
        assert_eq!(m.phase(), Phase::Riding);
    }
}

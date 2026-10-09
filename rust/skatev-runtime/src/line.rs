//! Showcase lines: a line is a start pose plus
//! controller input authored trick by trick and checked offline against the
//! real Skate runtime on GTA collision (`examples/line_author.rs`). On request
//! the worker puts the skater at the start with Skate's own teleport and feeds
//! the input to Skate one frame per tick, exactly as a pad would; Skate still
//! does all the skating. Any input from the player stops playback.
//!
//! Fixed frames alone do not repeat: a centimetre of difference after one
//! grind decides whether the next catches. So a line can also react the way a
//! player does: steer toward a point (`aim`) and hold until the skater crosses
//! a mark before the next move (`wait`).
//!
//! File format (text; files stay local, they hold GTA map coordinates):
//!
//! ```text
//! SKATEV_LINE 1
//! name <name>
//! start <x> <y> <z> <heading degrees>      GTA space, GTA heading
//! offboard                                 optional: start on foot, board in hand
//! # comment
//! <ticks> <buttons> <lt> <rt> <lx> <ly> <rx> <ry>
//! aim <x> <y> <lx per degree> <max lx> [min lx]   steer toward (x, y) while rolling
//! follow <x> <y> <dx> <dy> <lookahead m> <lx per degree> <max lx> [min lx]
//! aim off                                  also ends follow
//! wait near <x> <y> <radius> <timeout ticks>
//! wait past <x> <y> <dir x> <dir y> <timeout ticks>
//! ```
//!
//! A frame row holds one controller state for `ticks` Skate ticks. Buttons
//! are XInput bits (decimal or 0x hex); sticks are XInput i16 (+y up). An
//! `offboard` line starts on foot, so it can run in (A held sprints) and jump
//! on with Y for more speed than pushing gives.
//!
//! `aim` sets the left stick's X from the heading error to the point, only
//! while the skater rolls on the ground (in the air it would spin the body,
//! on a rail it would shift the balance) and only on frames whose own left
//! stick X is zero. Skate barely turns below about 0.6 deflection, so a
//! heading error over half a degree commands at least `min lx` (default 0).
//! `follow` steers onto the line through (x, y) along (dx, dy): it aims at
//! the point `lookahead` metres down the line from the skater's projection,
//! so the skater converges onto the line instead of a single point.
//! A `wait` emits neutral frames (plus aim) until the skater
//! is within the radius of the point, or has crossed the line through it
//! facing `dir`; a wait that times out stops the line (the take missed).
use bevy_math::{Vec2, Vec3};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub buttons: u16,
    pub triggers: [u8; 2],
    pub left: [i16; 2],
    pub right: [i16; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    /// GTA space.
    pub target: Vec2,
    /// `follow`: the line's unit direction and the lookahead (metres); the
    /// target is then the line's point that far past the skater.
    pub along: Option<(Vec2, f32)>,
    /// Left stick X per degree of heading error.
    pub gain: f32,
    pub max: i16,
    /// Least deflection for an error over half a degree (Skate's dead zone).
    pub min: i16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cond {
    Near { at: Vec2, radius: f32 },
    Past { at: Vec2, dir: Vec2 },
}

impl Cond {
    pub fn met(&self, pos: Vec2) -> bool {
        match *self {
            Cond::Near { at, radius } => pos.distance(at) <= radius,
            Cond::Past { at, dir } => (pos - at).dot(dir) >= 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Frame(Frame),
    Aim(Option<Aim>),
    Wait { cond: Cond, timeout: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub name: String,
    /// GTA space.
    pub start: Vec3,
    /// GTA heading, degrees.
    pub heading: f32,
    /// Start on foot with the board in hand (Skate's off-board entry).
    pub offboard: bool,
    /// Frames are one per Skate tick.
    pub steps: Vec<Step>,
}

/// Lines longer than this are refused (two minutes at 60 Hz, waits counted
/// at their timeout).
pub const MAX_TICKS: usize = 7200;

fn int<T: TryFrom<i64>>(token: &str, what: &str, line: usize) -> Result<T, String> {
    let value = if let Some(hex) = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16)
    } else {
        token.parse::<i64>()
    }
    .map_err(|_| format!("line {line}: {what} '{token}' is not a number"))?;
    T::try_from(value).map_err(|_| format!("line {line}: {what} {value} out of range"))
}

fn floats(tokens: &[&str], n: usize, line: usize) -> Result<Vec<f32>, String> {
    let v: Vec<f32> = tokens
        .iter()
        .map(|s| s.parse::<f32>().map_err(|_| format!("line {line}: bad number '{s}'")))
        .collect::<Result<_, _>>()?;
    if v.len() != n || !v.iter().all(|x| x.is_finite()) {
        return Err(format!("line {line}: expected {n} numbers"));
    }
    Ok(v)
}

impl Line {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut rows = text.lines().enumerate().map(|(i, l)| (i + 1, l.trim()));
        match rows.next() {
            Some((_, "SKATEV_LINE 1")) => {}
            _ => return Err("not a SKATEV_LINE 1 file".into()),
        }
        let (mut name, mut start, mut steps, mut offboard) = (String::new(), None, Vec::new(), false);
        let mut ticks = 0usize;
        for (n, row) in rows {
            if row.is_empty() || row.starts_with('#') {
                continue;
            }
            let t: Vec<&str> = row.split_whitespace().collect();
            match t[0] {
                "name" => name = t[1..].join(" "),
                "offboard" => offboard = true,
                "start" => {
                    let v = floats(&t[1..], 4, n)?;
                    start = Some((Vec3::new(v[0], v[1], v[2]), v[3]));
                }
                "aim" if t.get(1) == Some(&"off") => steps.push(Step::Aim(None)),
                "aim" => {
                    if t.len() != 5 && t.len() != 6 {
                        return Err(format!("line {n}: aim x y gain max [min]"));
                    }
                    let v = floats(&t[1..4], 3, n)?;
                    let max: i16 = int(t[4], "max", n)?;
                    let min: i16 = t.get(5).map_or(Ok(0), |m| int(m, "min", n))?;
                    steps.push(Step::Aim(Some(Aim {
                        target: Vec2::new(v[0], v[1]),
                        along: None,
                        gain: v[2],
                        max: max.saturating_abs(),
                        min: min.saturating_abs().min(max.saturating_abs()),
                    })));
                }
                "follow" => {
                    if t.len() != 8 && t.len() != 9 {
                        return Err(format!("line {n}: follow x y dx dy lookahead gain max [min]"));
                    }
                    let v = floats(&t[1..7], 6, n)?;
                    let dir = Vec2::new(v[2], v[3]).normalize_or_zero();
                    if dir == Vec2::ZERO {
                        return Err(format!("line {n}: follow needs a direction"));
                    }
                    let max: i16 = int(t[7], "max", n)?;
                    let min: i16 = t.get(8).map_or(Ok(0), |m| int(m, "min", n))?;
                    steps.push(Step::Aim(Some(Aim {
                        target: Vec2::new(v[0], v[1]),
                        along: Some((dir, v[4])),
                        gain: v[5],
                        max: max.saturating_abs(),
                        min: min.saturating_abs().min(max.saturating_abs()),
                    })));
                }
                "wait" => {
                    let (cond, rest) = match (t.get(1).copied(), t.len()) {
                        (Some("near"), 6) => {
                            let v = floats(&t[2..5], 3, n)?;
                            (Cond::Near { at: Vec2::new(v[0], v[1]), radius: v[2] }, t[5])
                        }
                        (Some("past"), 7) => {
                            let v = floats(&t[2..6], 4, n)?;
                            let dir = Vec2::new(v[2], v[3]).normalize_or_zero();
                            if dir == Vec2::ZERO {
                                return Err(format!("line {n}: wait past needs a direction"));
                            }
                            (Cond::Past { at: Vec2::new(v[0], v[1]), dir }, t[6])
                        }
                        _ => {
                            return Err(format!(
                                "line {n}: wait near x y radius timeout | wait past x y dx dy timeout"
                            ));
                        }
                    };
                    let timeout: u32 = int(rest, "timeout", n)?;
                    ticks += timeout as usize;
                    steps.push(Step::Wait { cond, timeout });
                }
                _ => {
                    if t.len() != 8 {
                        return Err(format!("line {n}: frame rows have 8 fields"));
                    }
                    let count: usize = int(t[0], "ticks", n)?;
                    let frame = Frame {
                        buttons: int(t[1], "buttons", n)?,
                        triggers: [int(t[2], "lt", n)?, int(t[3], "rt", n)?],
                        left: [int(t[4], "lx", n)?, int(t[5], "ly", n)?],
                        right: [int(t[6], "rx", n)?, int(t[7], "ry", n)?],
                    };
                    ticks += count;
                    if ticks > MAX_TICKS {
                        return Err(format!("line {n}: longer than {MAX_TICKS} ticks"));
                    }
                    steps.extend(std::iter::repeat_n(Step::Frame(frame), count));
                }
            }
            if ticks > MAX_TICKS {
                return Err(format!("line {n}: longer than {MAX_TICKS} ticks"));
            }
        }
        let (start, heading) = start.ok_or("no start pose")?;
        if !steps.iter().any(|s| matches!(s, Step::Frame(_))) {
            return Err("no frames".into());
        }
        Ok(Self { name, start, heading, offboard, steps })
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Frames plus wait timeouts: the longest the line can play.
    pub fn max_ticks(&self) -> usize {
        self.steps
            .iter()
            .map(|s| match s {
                Step::Frame(_) => 1,
                Step::Aim(_) => 0,
                Step::Wait { timeout, .. } => *timeout as usize,
            })
            .sum()
    }
}

/// What the player sees of the skater each tick (GTA space).
#[derive(Clone, Copy, Debug)]
pub struct Obs {
    pub pos: Vec3,
    /// Direction of travel (or facing when slow); only XY is used.
    pub forward: Vec3,
    /// Rolling on the ground: steering is the left stick's job here.
    pub rolling: bool,
}

impl Obs {
    /// From Skate's pose: travel direction above walking pace, else facing
    /// (Skate's root -Z, checked against travel on flat ground).
    pub fn from_pose(pose: &skate_host::bridge::Pose) -> Self {
        let root = pose.root.w_axis.truncate();
        let velocity = crate::coords::from_skate(pose.velocity);
        let facing = crate::coords::from_skate(pose.root.transform_vector3(-Vec3::Z));
        let flat = Vec3::new(velocity.x, velocity.y, 0.);
        Self {
            pos: crate::coords::from_skate(root),
            forward: if flat.length() > 1.0 { flat } else { facing },
            rolling: pose.state == "PhysicsGround",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Next {
    Frame(Frame),
    /// The line played out.
    Done,
    /// A wait timed out; the take missed its mark.
    Missed(String),
}

/// Plays one line, a frame per Skate tick.
#[derive(Default)]
pub struct Player {
    line: Option<Line>,
    step: usize,
    aim: Option<Aim>,
    waited: u32,
    tick: u32,
    /// Marks reached, for the log; drained by the caller.
    pub log: Vec<String>,
}

/// Left stick X steering toward `aim.target` (positive steers right).
pub fn steer(aim: &Aim, obs: &Obs) -> i16 {
    let pos = obs.pos.truncate();
    let target = match aim.along {
        Some((dir, ahead)) => aim.target + dir * ((pos - aim.target).dot(dir) + ahead),
        None => aim.target,
    };
    let (f, d) = (obs.forward.truncate(), target - pos);
    if f.length_squared() < 1e-6 || d.length_squared() < 1e-4 {
        return 0;
    }
    // GTA: right of forward (x, y) is (y, -x).
    let right = Vec2::new(f.y, -f.x);
    let error = d.dot(right).atan2(d.dot(f)).to_degrees();
    let lx = (aim.gain * error.abs()).clamp(0.0, f32::from(aim.max));
    let lx = if error.abs() > 0.5 { lx.max(f32::from(aim.min)) } else { lx };
    (lx * error.signum()) as i16
}

impl Player {
    pub fn start(&mut self, line: Line) {
        *self = Self { line: Some(line), ..Self::default() };
    }
    pub fn stop(&mut self) -> Option<Line> {
        let line = self.line.take();
        *self = Self::default();
        line
    }
    pub fn driving(&self) -> bool {
        self.line.is_some()
    }

    /// The input for this tick.
    pub fn next(&mut self, obs: &Obs) -> Next {
        let Some(line) = self.line.as_ref() else {
            return Next::Done;
        };
        self.tick += 1;
        let pos = obs.pos.truncate();
        loop {
            let Some(&step) = line.steps.get(self.step) else {
                return Next::Done;
            };
            match step {
                Step::Aim(aim) => {
                    self.aim = aim;
                    self.step += 1;
                }
                Step::Wait { cond, timeout } => {
                    if cond.met(pos) {
                        self.log.push(format!(
                            "line mark {cond:?} reached at line tick {} ({:.2}, {:.2}, {:.2})",
                            self.tick, obs.pos.x, obs.pos.y, obs.pos.z
                        ));
                        self.step += 1;
                        self.waited = 0;
                        continue;
                    }
                    if self.waited >= timeout {
                        return Next::Missed(format!(
                            "mark {cond:?} not reached in {timeout} ticks (skater at {:.2}, {:.2}, {:.2})",
                            obs.pos.x, obs.pos.y, obs.pos.z
                        ));
                    }
                    self.waited += 1;
                    return Next::Frame(self.steered(Frame::default(), obs));
                }
                Step::Frame(frame) => {
                    self.step += 1;
                    return Next::Frame(self.steered(frame, obs));
                }
            }
        }
    }

    fn steered(&self, mut frame: Frame, obs: &Obs) -> Frame {
        if let Some(aim) = self.aim.as_ref().filter(|_| obs.rolling && frame.left[0] == 0) {
            frame.left[0] = steer(aim, obs);
        }
        frame
    }
}

/// The player touched the controller: any button, trigger or stick past
/// a small dead zone.
pub fn player_input(buttons: u16, triggers: [u8; 2], left: [i16; 2], right: [i16; 2]) -> bool {
    const DEAD: i32 = 8000;
    buttons != 0
        || triggers.iter().any(|t| *t > 30)
        || left.iter().chain(right.iter()).any(|v| i32::from(*v).abs() > DEAD)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "SKATEV_LINE 1\nname test line\nstart -1.5 2 3.25 90\n# push\n6 0x1000 0 0 0 0 0 0\n\n3 0 0 255 0 -32768 0 32767\n";

    fn frames(l: &Line) -> Vec<Frame> {
        l.steps.iter().filter_map(|s| if let Step::Frame(f) = s { Some(*f) } else { None }).collect()
    }

    fn obs(x: f32, y: f32, fx: f32, fy: f32) -> Obs {
        Obs { pos: Vec3::new(x, y, 0.), forward: Vec3::new(fx, fy, 0.), rolling: true }
    }

    #[test]
    fn parses_and_expands_rows_per_tick() {
        let l = Line::parse(TEXT).unwrap();
        assert_eq!(l.name, "test line");
        assert_eq!((l.start, l.heading), (Vec3::new(-1.5, 2.0, 3.25), 90.0));
        let f = frames(&l);
        assert_eq!(f.len(), 9);
        assert_eq!(l.max_ticks(), 9);
        assert!(!l.offboard);
        assert!(Line::parse(&TEXT.replace("# push", "offboard")).unwrap().offboard);
        assert_eq!(f[0].buttons, 0x1000);
        assert_eq!(f[8], Frame { buttons: 0, triggers: [0, 255], left: [0, -32768], right: [0, 32767] });
    }

    #[test]
    fn rejects_bad_files() {
        let ok = "SKATEV_LINE 1\nstart 0 0 0 0\n1 0 0 0 0 0 0 0\n";
        assert!(Line::parse(ok).is_ok());
        assert!(Line::parse("hello").is_err());
        assert!(Line::parse("SKATEV_LINE 1\nstart 0 0 0 0\n").is_err(), "no frames");
        assert!(Line::parse("SKATEV_LINE 1\n6 0 0 0 0 0 0 0\n").is_err(), "no start");
        assert!(Line::parse("SKATEV_LINE 1\nstart 0 0 0 0\n6 0 0 0 0 0 0 40000\n").is_err(), "stick range");
        assert!(Line::parse("SKATEV_LINE 1\nstart 0 0 0 0\n6 0 0 0\n").is_err(), "short row");
        assert!(Line::parse("SKATEV_LINE 1\nstart 0 0 0 0\n9000 0 0 0 0 0 0 0\n").is_err(), "too long");
        assert!(Line::parse(&format!("{ok}wait near 1 2 3\n")).is_err(), "no timeout");
        assert!(Line::parse(&format!("{ok}wait past 1 2 0 0 9\n")).is_err(), "no direction");
        assert!(Line::parse(&format!("{ok}wait near 0 0 1 9000\n")).is_err(), "waits count");
        assert!(Line::parse(&format!("{ok}aim 1 2 3\n")).is_err(), "aim needs a max");
    }

    #[test]
    fn player_feeds_each_tick_then_finishes() {
        let mut p = Player::default();
        p.start(Line::parse(TEXT).unwrap());
        assert!(p.driving());
        let o = obs(0., 0., 0., 1.);
        let fed: Vec<Next> = std::iter::from_fn(|| match p.next(&o) {
            Next::Done => None,
            n => Some(n),
        })
        .collect();
        assert_eq!(fed.len(), 9);
        assert!(p.stop().is_some() && !p.driving());
    }

    #[test]
    fn wait_holds_until_the_mark_then_moves_on() {
        let text = "SKATEV_LINE 1\nstart 0 0 0 0\nwait past 0 10 0 1 100\n2 0x1000 0 0 0 0 0 0\n";
        let mut p = Player::default();
        p.start(Line::parse(text).unwrap());
        // Rolling north toward y = 10: neutral frames until it is crossed.
        for y in [0., 5., 9.9] {
            assert_eq!(p.next(&obs(0., y, 0., 1.)), Next::Frame(Frame::default()));
        }
        assert_eq!(p.next(&obs(0., 10.1, 0., 1.)), Next::Frame(Frame { buttons: 0x1000, ..Default::default() }));
        assert_eq!(p.log.len(), 1);
        // A mark never reached misses the take.
        let mut p = Player::default();
        p.start(Line::parse("SKATEV_LINE 1\nstart 0 0 0 0\nwait near 50 50 1 3\n1 0 0 0 0 0 0 0\n").unwrap());
        let o = obs(0., 0., 0., 1.);
        for _ in 0..3 {
            assert!(matches!(p.next(&o), Next::Frame(_)));
        }
        assert!(matches!(p.next(&o), Next::Missed(_)));
    }

    #[test]
    fn aim_steers_toward_the_point_only_while_rolling() {
        let aim = Aim { target: Vec2::new(10., 10.), along: None, gain: 1000., max: 20000, min: 0 };
        // Facing north, the point is 45 degrees to the right: full right (+).
        assert_eq!(steer(&aim, &obs(0., 0., 0., 1.)), 20000);
        // Facing east, it is 45 degrees to the left.
        assert_eq!(steer(&aim, &obs(0., 0., 1., 0.)), -20000);
        assert_eq!(steer(&aim, &obs(0., 0., 1., 1.)), 0);
        // A small error still clears Skate's dead zone with a minimum.
        let firm = Aim { target: Vec2::new(1., 100.), along: None, gain: 10., max: 32000, min: 20000 };
        assert_eq!(steer(&firm, &obs(0., 0., 0., 1.)), 20000);
        assert_eq!(steer(&Aim { target: Vec2::new(0.001, 100.), ..firm }, &obs(0., 0., 0., 1.)), 0);
        // Following the line x = 2 northward from x = 0 heading north: the
        // point 2 m ahead on it is 45 degrees right; on the line, straight on.
        let follow = Line::parse("SKATEV_LINE 1\nstart 0 0 0 0\nfollow 2 50 0 1 2 1000 20000\n1 0 0 0 0 0 0 0\n").unwrap();
        let Step::Aim(Some(line_aim)) = follow.steps[0] else { panic!("follow parses to an aim") };
        assert_eq!(steer(&line_aim, &obs(0., 0., 0., 1.)), 20000);
        assert_eq!(steer(&line_aim, &obs(2., 7., 0., 1.)), 0);
        let text = "SKATEV_LINE 1\nstart 0 0 0 0\naim 10 10 100 20000\n1 0 0 0 0 0 0 0\n1 0 0 0 -5 0 0 0\naim off\n1 0 0 0 0 0 0 0\n";
        let mut p = Player::default();
        p.start(Line::parse(text).unwrap());
        let mut o = obs(0., 0., 0., 1.);
        assert_eq!(p.next(&o), Next::Frame(Frame { left: [4500, 0], ..Default::default() }));
        // A frame with its own left stick X keeps it.
        assert_eq!(p.next(&o), Next::Frame(Frame { left: [-5, 0], ..Default::default() }));
        o.rolling = false;
        assert_eq!(p.next(&o), Next::Frame(Frame::default()));
        assert_eq!(p.next(&o), Next::Done);
    }

    #[test]
    fn player_input_ignores_stick_noise() {
        assert!(!player_input(0, [0, 0], [3000, -2000], [0, 7000]));
        assert!(player_input(0x1000, [0, 0], [0, 0], [0, 0]));
        assert!(player_input(0, [0, 200], [0, 0], [0, 0]));
        assert!(player_input(0, [0, 0], [0, 0], [-20000, 0]));
    }
}

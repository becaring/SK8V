//! The cinematic camera for skating, as GTA's cinematic camera in a car
//! (R): it cuts between shots every few seconds. A low chase behind the
//! board, a tracking shot from the side, a roadside camera that waits ahead
//! and zooms as the skater comes and goes, a low shot from in front looking
//! back, a helicopter shot from above, a long lens from far behind, and one
//! down at the wheels. As GTA's, the moving cameras trail the skater softly
//! and shake a little as if held, and the roadside one pans on its tripod
//! Each shot is worked out from where the skater is and goes; the host
//! keeps it out of walls. Contributed by GTAxSkate (Apache-2.0).

use bevy_math::Vec3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shot {
    Chase,
    Side,
    Roadside,
    Front,
    Overhead,
    Telephoto,
    Wheel,
}

const ORDER: [Shot; 9] = [
    Shot::Chase,
    Shot::Roadside,
    Shot::Side,
    Shot::Overhead,
    Shot::Wheel,
    Shot::Roadside,
    Shot::Telephoto,
    Shot::Front,
    Shot::Side,
];

/// Seconds a moving shot lasts.
const SHOT_SECONDS: f32 = 5.;
/// How quickly a moving camera follows (per second; higher is stiffer).
const TRAIL: f32 = 4.;
/// How quickly the camera's aim follows.
const AIM: f32 = 7.;
/// The hand-held shake: metres of the eye, metres of the aim point.
const SHAKE_EYE: f32 = 0.035;
const SHAKE_LOOK: f32 = 0.025;

pub struct ActionCam {
    shot: usize,
    /// Seconds into the shot.
    time: f32,
    /// Seconds since the camera came on, for the shake.
    clock: f32,
    /// Which side a side shot is on (+1 right of travel, -1 left).
    side: f32,
    /// Where a roadside camera stands, fixed for its shot.
    post: Option<Vec3>,
    /// The way the skater went, kept while they stand.
    heading: Vec3,
    /// Where the camera is and aims now (none right after a cut: it starts
    /// in place).
    eye: Option<Vec3>,
    look: Option<Vec3>,
}

impl Default for ActionCam {
    fn default() -> Self {
        Self { shot: 0, time: 0., clock: 0., side: 1., post: None, heading: Vec3::Y, eye: None, look: None }
    }
}

/// A camera placement: where it is and the point it looks at, and its
/// field of view (degrees).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub eye: Vec3,
    pub look: Vec3,
    pub fov: f32,
}

/// `current` moved toward `target` as a damped follow over `dt` seconds.
fn follow(current: Vec3, target: Vec3, rate: f32, dt: f32) -> Vec3 {
    current + (target - current) * (1. - (-rate * dt.max(0.)).exp())
}

/// A slow, uneven wobble about zero (each axis about -1 to 1).
fn handheld(t: f32) -> Vec3 {
    Vec3::new(
        (1.3 * t).sin() * 0.65 + (3.1 * t + 1.).sin() * 0.35,
        (1.7 * t + 2.).sin() * 0.65 + (2.9 * t).sin() * 0.35,
        (1.1 * t + 4.).sin() * 0.6 + (2.3 * t + 0.5).sin() * 0.4,
    )
}

impl ActionCam {
    pub fn shot(&self) -> Shot {
        ORDER[self.shot % ORDER.len()]
    }

    /// The camera for this frame: the skater's pelvis `at`, their velocity
    /// (m/s), the frame's seconds.
    pub fn update(&mut self, at: Vec3, velocity: Vec3, dt: f32) -> Placed {
        let flat = Vec3::new(velocity.x, velocity.y, 0.);
        if flat.length() > 0.8 {
            self.heading = flat.normalize();
        }
        let ahead = self.heading;
        let right = Vec3::new(ahead.y, -ahead.x, 0.);
        self.time += dt;
        self.clock += dt;
        let speed = flat.length();
        // A roadside camera cuts once the skater is well past it (or never
        // reached it); the others after a few seconds.
        let done = match self.shot() {
            Shot::Roadside => self.post.is_some_and(|p| (at - p).dot(ahead) > 6. || self.time > 9.),
            _ => self.time > SHOT_SECONDS,
        };
        if done {
            self.cut();
        }
        let aim = at + Vec3::Z * 0.2;
        let (eye, look, fov) = match self.shot() {
            Shot::Chase => (at - ahead * 3.4 - Vec3::Z * 0.45, aim + ahead * 2., 55.),
            Shot::Side => (at + right * self.side * (3. + speed * 0.1) - Vec3::Z * 0.55, aim, 50.),
            Shot::Front => (at + ahead * (3.2 + speed * 0.15) - Vec3::Z * 0.6, aim, 55.),
            // From a helicopter: high, behind and off to a side.
            Shot::Overhead => (at - ahead * 4. + right * self.side * 3. + Vec3::Z * 9., aim, 40.),
            // A long lens from far behind: the street squeezed up.
            Shot::Telephoto => (at - ahead * (14. + speed * 0.4) + Vec3::Z * 1.2, aim + ahead * 1.5, 22.),
            // Down by the wheels, alongside.
            Shot::Wheel => (at + right * self.side * 1.1 + ahead * 0.6 - Vec3::Z * 0.85, at - Vec3::Z * 0.75 + ahead * 0.3, 65.),
            Shot::Roadside => {
                let post = *self.post.get_or_insert(at + ahead * (6. + speed * 1.6) + right * self.side * 3.5 - Vec3::Z * 0.4);
                // Zoomed to keep the skater about the same size.
                let distance = post.distance(aim).max(0.5);
                (post, aim, (2. * (1.6 / distance).atan()).to_degrees().clamp(18., 50.))
            }
        };
        let roadside = self.shot() == Shot::Roadside;
        // Moving cameras trail; the roadside one stands still on its post.
        let eye = match (self.eye, roadside) {
            (Some(now), false) => follow(now, eye, TRAIL, dt),
            _ => eye,
        };
        let look = match self.look {
            Some(now) => follow(now, look, AIM, dt),
            None => look,
        };
        self.eye = Some(eye);
        self.look = Some(look);
        let shake = handheld(self.clock);
        let eye = if roadside { eye } else { eye + shake * SHAKE_EYE };
        let look = look + handheld(self.clock * 0.8 + 7.) * SHAKE_LOOK;
        Placed { eye, look, fov }
    }

    fn cut(&mut self) {
        self.shot = (self.shot + 1) % ORDER.len();
        self.time = 0.;
        self.post = None;
        self.side = -self.side;
        self.eye = None;
        self.look = None;
    }

    /// Starts over at the first shot.
    pub fn reset(&mut self) {
        *self = Self { heading: self.heading, ..Self::default() };
    }
}

/// The skate-video camera (R, after GTA's cinematic one): a filmer on a
/// board behind and beside the skater, the camera held low with a fisheye
/// lens, as skate videos are shot. The filmer stays on the
/// ground when the skater pops, so the lens looks up at the trick.
pub struct FilmerCam {
    eye: Option<Vec3>,
    look: Option<Vec3>,
    /// The filmer's ground under them: follows the skater's height slowly.
    ground: Option<f32>,
    heading: Vec3,
    clock: f32,
}

impl Default for FilmerCam {
    fn default() -> Self {
        Self { eye: None, look: None, ground: None, heading: Vec3::Y, clock: 0. }
    }
}

/// The fisheye's field of view (degrees): as wide as GTA draws well.
pub const FISHEYE_FOV: f32 = 100.;

impl FilmerCam {
    /// The camera for this frame: the skater's pelvis `at`, their velocity
    /// (m/s), the frame's seconds.
    pub fn update(&mut self, at: Vec3, velocity: Vec3, dt: f32) -> Placed {
        let flat = Vec3::new(velocity.x, velocity.y, 0.);
        if flat.length() > 0.8 {
            self.heading = flat.normalize();
        }
        self.clock += dt;
        let ahead = self.heading;
        let right = Vec3::new(ahead.y, -ahead.x, 0.);
        // The skater's pelvis about a metre over the ground; the filmer's
        // ground follows it slowly (an ollie does not lift the filmer).
        let pelvis_ground = at.z - 1.;
        let ground = match self.ground {
            Some(g) if pelvis_ground > g => g + (pelvis_ground - g) * (1. - (-1.5 * dt).exp()),
            _ => pelvis_ground,
        };
        self.ground = Some(ground);
        let target = Vec3::new(at.x, at.y, ground + 0.55) - ahead * 1.7 + right * 0.8;
        let eye = match self.eye {
            Some(now) => follow(now, target, 5., dt),
            None => target,
        };
        let aim = at - Vec3::Z * 0.35 + ahead * 0.4;
        let look = match self.look {
            Some(now) => follow(now, aim, 9., dt),
            None => aim,
        };
        self.eye = Some(eye);
        self.look = Some(look);
        // Pushing along: a bit more bounce than a tripod or a car mount.
        Placed { eye: eye + handheld(self.clock * 1.4) * 0.045, look, fov: FISHEYE_FOV }
    }

    pub fn reset(&mut self) {
        *self = Self { heading: self.heading, ..Self::default() };
    }
}

use std::sync::Mutex;

static CAMS: Mutex<Option<(ActionCam, FilmerCam)>> = Mutex::new(None);

/// The cinematic cameras for the host: `mode` 1 the cinematic camera, 2 the
/// skate filmer, 0 resets both. `at`/`velocity`: the skater (GTA world, m
/// and m/s). Writes eye xyz, look xyz and fov (degrees) to `out[0..7]`.
/// Returns 1 when written.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sv_action_cam(_rt: *mut std::ffi::c_void, mode: u32, at: *const f32, velocity: *const f32, dt: f32,
                                       out: *mut f32) -> u32 {
    crate::guard(0, || {
        let mut cams = CAMS.lock().unwrap_or_else(|e| e.into_inner());
        let (cinema, filmer) = cams.get_or_insert_with(|| (ActionCam::default(), FilmerCam::default()));
        if mode == 0 || at.is_null() || velocity.is_null() || out.is_null() {
            cinema.reset();
            filmer.reset();
            return 0;
        }
        let read = |p: *const f32| unsafe { Vec3::new(*p, *p.add(1), *p.add(2)) };
        let (at, velocity) = (read(at), read(velocity));
        if !at.is_finite() || !velocity.is_finite() || !dt.is_finite() {
            return 0;
        }
        let placed = if mode == 2 { filmer.update(at, velocity, dt) } else { cinema.update(at, velocity, dt) };
        let values = [placed.eye.x, placed.eye.y, placed.eye.z, placed.look.x, placed.look.y, placed.look.z, placed.fov];
        unsafe { std::ptr::copy_nonoverlapping(values.as_ptr(), out, 7) };
        1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filmer_rides_low_behind_and_stays_down_when_the_skater_pops() {
        let mut cam = FilmerCam::default();
        let v = Vec3::new(0., 6., 0.);
        let first = cam.update(Vec3::new(0., 0., 1.), v, 0.016);
        assert!(first.eye.y < -1. && first.eye.z < 0.8 && first.fov >= 90., "{first:?}");
        // An ollie: the pelvis 0.6 m higher for a moment.
        let up = cam.update(Vec3::new(0., 0.1, 1.6), v, 0.016);
        assert!(up.eye.z < first.eye.z + 0.1, "the filmer stays down: {} then {}", first.eye.z, up.eye.z);
        assert!(up.look.z > first.look.z, "and looks up at the trick");
    }

    #[test]
    fn the_shots_cut_and_the_roadside_camera_waits_ahead() {
        let mut cam = ActionCam::default();
        let v = Vec3::new(0., 8., 0.);
        let first = cam.update(Vec3::ZERO, v, 0.016);
        assert_eq!(cam.shot(), Shot::Chase);
        assert!(first.eye.y < -3. && first.eye.z < 0., "behind and low: {:?}", first.eye);
        // Ride on until the chase ends: a roadside camera ahead.
        let mut at = Vec3::ZERO;
        let mut placed = first;
        for _ in 0..330 {
            at += v * 0.016;
            placed = cam.update(at, v, 0.016);
        }
        assert_eq!(cam.shot(), Shot::Roadside);
        let post = placed.eye;
        assert!(post.y > at.y, "ahead of the skater");
        // It stays put while they ride past, then cuts.
        for _ in 0..60 {
            at += v * 0.016;
            assert!(cam.update(at, v, 0.016).eye == post || cam.shot() != Shot::Roadside);
        }
        for _ in 0..400 {
            at += v * 0.016;
            cam.update(at, v, 0.016);
        }
        assert_ne!(cam.shot(), Shot::Roadside, "cut once passed");
    }

    #[test]
    fn a_moving_camera_trails_softly_instead_of_jumping() {
        let mut cam = ActionCam::default();
        let v = Vec3::new(0., 5., 0.);
        let before = cam.update(Vec3::ZERO, v, 0.016);
        // The skater lurches 1 m sideways in one frame: the chase camera
        // takes only a little of it at once.
        let after = cam.update(Vec3::new(1., 0.08, 0.), v, 0.016);
        let moved = (after.eye.x - before.eye.x).abs();
        assert!(moved > 0.01 && moved < 0.2, "moved {moved}");
    }

    #[test]
    fn the_roadside_camera_zooms_in_as_the_skater_goes_away() {
        let mut cam = ActionCam::default();
        cam.shot = 1;
        let v = Vec3::new(0., 1., 0.);
        let near = cam.update(Vec3::ZERO, v, 0.016);
        assert_eq!(cam.shot(), Shot::Roadside);
        let far = cam.update(Vec3::new(0., -30., 0.), v, 0.016);
        assert!(far.fov < near.fov, "{} then {}", near.fov, far.fov);
    }
}

//! Skate 3 on keyboard and mouse.
//!
//! Skate's tricks are right-stick motions, which read like a fighting
//! game's: an ollie is 2 then 8, a kickflip 2 then 7, a shove-it 2-1-4.
//! So the keyboard gets both of SF6's schemes. Classic: direction keys
//! (arrows or the numpad) hold the trick stick and motions are typed as
//! motions, with SF6's leniency for diagonals. Modern: one key plays a
//! whole trick's motion. The mouse can flick the stick too. All of it
//! becomes a controller, merged with a real one, for Skate's own input.

pub mod controls;
pub mod hints;
pub mod keys;
pub mod translate;

pub use controls::{ACTIONS, Action, Controls, Gesture, MouseTrick};
pub use translate::{Pad, Translator};

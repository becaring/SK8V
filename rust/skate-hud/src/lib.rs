//! Skate 3's original scoring HUD (the APT `trickdisplay2` movie) for SkateV.
//!
//! The HUD runtime itself is the donor engine's
//! (`SK8-ENGINE/skate-3-rust-engine` @ cb7968930, `crates/skate-game/src`),
//! compiled **in place** from `upstream/skate-3-rust-engine-donor` through
//! `#[path]` modules: no donor text lives in this repository (the donor has
//! no license file at that commit; chasmlol granted Apache-2.0 for these
//! modules, see NOTICE). Those modules are
//! Bevy-free: the ActionScript VM (`apt_vm`), display list (`apt_display`),
//! movie timeline (`apt_movie`), text layout (`apt_text`), scene traversal
//! (`apt_scene`) and the native `Tricks`/`HUDComponents` bindings
//! (`hud_runtime`).
//!
//! Project-owned glue:
//! - [`player`]: asset loading, the scorer -> HUD input mapping, edge offset,
//!   flat draw list in output pixels;
//! - [`hom`]: the Hall of Meat movie (`homscoring`) and its natives;
//! - [`layout`]: aspect ratio / safe area mapping of the 1280x720 movie;
//! - [`raster`]: a CPU reference of the donor's GPU path (tests, previews).

#[allow(dead_code, clippy::all)]
#[path = "../../../upstream/skate-3-rust-engine-donor/crates/skate-game/src/apt_vm.rs"]
pub mod apt_vm;

#[allow(dead_code, clippy::all)]
#[path = "../../../upstream/skate-3-rust-engine-donor/crates/skate-game/src/apt_display.rs"]
pub mod apt_display;

#[allow(dead_code, clippy::all)]
#[path = "../../../upstream/skate-3-rust-engine-donor/crates/skate-game/src/apt_text.rs"]
pub mod apt_text;

#[allow(dead_code, clippy::all)]
#[path = "../../../upstream/skate-3-rust-engine-donor/crates/skate-game/src/apt_movie.rs"]
pub mod apt_movie;

#[allow(dead_code, clippy::all)]
#[path = "../../../upstream/skate-3-rust-engine-donor/crates/skate-game/src/apt_scene.rs"]
pub mod apt_scene;

#[allow(dead_code, clippy::all)]
#[path = "../../../upstream/skate-3-rust-engine-donor/crates/skate-game/src/hud_runtime.rs"]
pub mod hud_runtime;

pub mod chyron;
pub mod hom;
pub mod layout;
mod movie;
pub mod player;
mod presentation;
pub mod raster;

pub use chyron::ChyronPlayer;
pub use hom::{HomData, HomPlayer};
pub use layout::Layout;
pub use player::{Assets, DrawCmd, Frame, Player, ScoreInput, Vertex};

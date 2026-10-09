//! SkateV's live Skate 3 audio: the AEMS world driven by the game-side class
//! handlers from live skater state, every voice rendered through EA Audio
//! Core's dry chain (SndPlayer1 → Rechannel → Resample → HighPassIir2 →
//! LowPassIir2 → Gain) and summed per emitter. No pan, sends, buses or
//! reverb: the host's audio engine places the result (docs/AEMS.md, "Live
//! engine").

pub mod cache;
pub mod components;
pub mod component_life;
pub mod creation;
pub mod driver;
pub mod engine;
pub mod frame;
pub mod grains;
pub mod grain_inputs;
pub mod grain_tuning;
pub mod splices;
pub mod ui_sounds;
pub mod board_foley;
pub mod body_foley;
pub mod footsteps;
pub mod body_impacts;
pub mod collisions;
pub mod impacts;
pub mod mxb;
pub mod evaluator;
pub mod tuning;
pub mod voices;
pub mod seam_voices;

pub use engine::{BlockInfo, Engine};
pub use frame::Frame;

/// Emitters (match `SvAudioEmitterId` in host/include/skatev_runtime.h).
pub const EMITTER_BOARD: usize = 0;
pub const EMITTER_BODY: usize = 1;
pub const EMITTER_SPEED: usize = 2;
pub const EMITTERS: usize = 3;
pub mod inputs;
pub mod mix_inputs;
pub mod evaluator_controls;
pub mod foley_life;

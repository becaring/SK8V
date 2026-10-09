//! Skate 3's AEMS (EA audio scripting) runtime, ported from the TU3 image:
//! `.abk` patch banks, sound objects, parameters and messages, the 40-module
//! patch VM with its control tick, and the sound-player glue down to EA Audio
//! Core's dry voices. See `docs/AEMS.md`.
//!
//! The port keeps the game's float branches and PowerPC rounding. Clippy lints
//! that would rewrite them are allowed here with the reason:
//! - `neg_cmp_op_on_partial_ord`: `!(a < b)` is true for NaN, `a >= b` is not.
//! - `excessive_precision`: constants are the game's literal values.
//! - `needless_range_loop`: lane loops mirror the vector code.
//! - `should_implement_trait`: `next` is the game's PRNG step, not an iterator.
//! - `chunks_exact_to_as_chunks`: block loops keep `chunks_exact`.
#![allow(
    clippy::neg_cmp_op_on_partial_ord,
    clippy::excessive_precision,
    clippy::needless_range_loop,
    clippy::should_implement_trait,
    clippy::chunks_exact_to_as_chunks
)]

pub mod be;
pub mod eac;
pub mod glue;
pub mod libm;
pub mod live;
pub mod mem;
#[cfg(feature = "translated")]
pub mod translated;
pub mod ops;
pub mod ppc;
pub mod rng;
pub mod sine;
pub mod splice;
pub mod voice;
pub mod world;

pub use world::World;

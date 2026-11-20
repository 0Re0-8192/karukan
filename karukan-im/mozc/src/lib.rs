//! Mozc `Command` protobuf <-> karukan engine, for a Mozc-based Android IME.
//!
//! See [`bridge::Bridge`] for the ownership rules and [`proto`] for the wire codec.

pub mod bridge;
pub mod input;
pub mod output;
pub mod proto;

pub use bridge::{Backend, Bridge, ImBackend};

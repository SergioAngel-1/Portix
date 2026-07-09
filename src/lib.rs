//! Portix: the local development service explorer.
//!
//! Dependencies flow one way and never invert:
//!
//! ```text
//! ui ──▶ core ──▶ { detect, cache, platform } ──▶ model ──▶ util
//! ```
//!
//! `unsafe` is denied crate-wide. The only places that opt back in are the
//! syscall wrappers in `platform::linux`, each with a SAFETY comment: parsing
//! attacker-controlled `/proc` data must never happen next to raw pointers.
#![deny(unsafe_code)]

pub mod cache;
pub mod core;
pub mod detect;
pub mod model;
pub mod platform;
pub mod ui;
pub mod util;

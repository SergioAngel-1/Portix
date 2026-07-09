#![forbid(unsafe_code)]

pub mod app;
pub mod format;
// pub mod input; // Reserved for structured filtering (PRD §11.5)
// pub mod widgets; // Reserved for reusable widgets (future)
pub mod views;

pub use app::*;

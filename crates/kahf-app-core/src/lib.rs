//! KAHF app core: everything the desktop app does besides drawing.
//!
//! The window (any toolkit) owns a [`Shared`], calls the functions in [`backend`] and receives the
//! engine's events as JSON through its [`Emitter`]. Nothing here knows about windows.

pub mod applog;
pub mod backend;
pub mod convert;
pub mod diag;
pub mod directory;
pub mod live;
pub mod report;
pub mod rig;
pub mod settings;
pub mod update;
pub mod voice;

pub use backend::*;

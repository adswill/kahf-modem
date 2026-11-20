//! Client for the kahf.dev relay server (see `server/README.md`).
//!
//! A relay station registers its callsign, one-degree area and public key
//! (`POST /register`), then asks for transmit and receive slots
//! (`POST /schedule`). Both requests are signed with the station key over
//! canonical JSON, as the server checks them.
//!
//! - [`Client`] sends one request and maps the answer to Rust types.
//! - [`RelaySync`] decides when to register, when to ask for the next
//!   schedule and how long to back off while the server cannot be reached.
//!   It takes the time as an argument, so it is tested without a clock.
//! - [`HttpTransport`] is the real HTTPS transport. Tests use their own
//!   [`Transport`].

mod client;
mod http;
mod sync;
mod wire;

pub use client::{Client, Error, Registration, Response, Schedule, SlotRequest, Transport, Unplaced};
pub use http::HttpTransport;
pub use sync::{RelaySync, Step, SyncConfig};
pub use wire::{canonical_json, Area, ChannelGrid};

/// The public server.
pub const DEFAULT_BASE_URL: &str = "https://kahf.dev";

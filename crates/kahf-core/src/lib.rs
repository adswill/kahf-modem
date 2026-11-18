//! KAHF core: own LDPC code, OFDM-style PHY, frame layer, link and session layers, I/O traits.
//! Audio sample rate is 8000 Hz (real, mono, f32 in [-1, 1]).
pub mod access;
pub mod auth;
pub mod bandplan;
pub mod blank;
pub mod bundle;
pub mod diversity;
pub mod extras;
pub mod frame;
pub mod harq;
pub mod hop;
pub mod image;
pub mod interf;
pub mod io;
pub mod kami;
pub mod ldpc;
pub mod lanes;
pub mod lanesense;
pub mod ldpc_std;
pub mod link;
pub mod mac;
pub mod narrow;
#[cfg(test)]
pub mod sim;
pub mod marker;
pub mod outbox;
pub mod payload_position;
pub mod payload_presence;
pub mod payload_reception;
pub mod payload_telemetry;
pub mod payload_time;
pub mod payload_update;
pub mod payload_urgent;
pub mod payload_weather;
pub mod phy;
pub mod planner;
pub mod referee;
pub mod relay;
pub mod select;
pub mod sense;
pub mod session;
pub mod snr_est;
pub mod spectrum;

pub mod callsign;
pub mod update_prompt;

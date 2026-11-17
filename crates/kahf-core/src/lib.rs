//! KAHF core: own LDPC code, OFDM-style PHY, frame layer, link and session layers, I/O traits.
//! Audio sample rate is 8000 Hz (real, mono, f32 in [-1, 1]).
pub mod ldpc;
pub mod phy;
pub mod frame;
pub mod io;
pub mod link;
pub mod session;

//! SDR adapters (IqSource / IqSink from kahf-core::io). Reference: OnAir's
//! core/src/source/hackrf_usb.cpp and source_rtlsdr.cpp.
pub mod hackrf; // owner: HackRF agent (RX + TX)
pub mod rtlsdr; // owner: RTL-SDR agent (RX)

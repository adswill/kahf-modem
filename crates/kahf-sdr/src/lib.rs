//! SDR adapters (IqSource / IqSink from kahf-core::io).
//! core/src/source/hackrf_usb.cpp and source_rtlsdr.cpp.
pub(crate) mod dl;
pub mod hackrf; // HackRF receive and transmit
pub mod rtlsdr; // RTL-SDR receive

/// Whether the HackRF and RTL-SDR libraries can be loaded on this computer.
pub fn libraries_present() -> (bool, bool) {
    (dl::hackrf().is_some(), dl::rtlsdr().is_some())
}

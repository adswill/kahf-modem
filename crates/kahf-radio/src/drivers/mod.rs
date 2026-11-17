//! One module per radio. Each module provides `probe()` and `open(id)`.
//! A module that is not written yet returns an empty list and an error that says so.

pub mod common;

pub mod airspy;
pub mod airspyhf;
pub mod bladerf;
pub mod fobos;
pub mod hackrf;
pub mod hermeslite2;
pub mod icom;
pub mod kiwisdr;
pub mod limesdr;
pub mod nrspst;
pub mod plutosdr;
pub mod rfnm;
pub mod rtlsdr;
pub mod rtltcp;
pub mod rx888;
pub mod sdrplay;
pub mod sdrpp;
pub mod soapy;
pub mod spyserver;
pub mod tci;
pub mod usrp;

use crate::{DeviceInfo, RxRadio};

pub const NAMES: &[&str] = &[
    "airspy",
    "airspyhf",
    "bladerf",
    "fobos",
    "hackrf",
    "hermeslite2",
    "icom",
    "kiwisdr",
    "limesdr",
    "nrspst",
    "plutosdr",
    "rfnm",
    "rtlsdr",
    "rtltcp",
    "rx888",
    "sdrpp",
    "sdrplay",
    "soapy",
    "spyserver",
    "tci",
    "usrp",
];

pub fn probe_all() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    out.extend(airspy::probe());
    out.extend(airspyhf::probe());
    out.extend(bladerf::probe());
    out.extend(fobos::probe());
    out.extend(hackrf::probe());
    out.extend(hermeslite2::probe());
    out.extend(icom::probe());
    out.extend(kiwisdr::probe());
    out.extend(limesdr::probe());
    out.extend(nrspst::probe());
    out.extend(plutosdr::probe());
    out.extend(rfnm::probe());
    out.extend(rtlsdr::probe());
    out.extend(rtltcp::probe());
    out.extend(rx888::probe());
    out.extend(sdrpp::probe());
    out.extend(sdrplay::probe());
    out.extend(soapy::probe());
    out.extend(spyserver::probe());
    out.extend(tci::probe());
    out.extend(usrp::probe());
    out
}

pub fn open(driver: &str, id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    match driver {
        "airspy" => airspy::open(id),
        "airspyhf" => airspyhf::open(id),
        "bladerf" => bladerf::open(id),
        "fobos" => fobos::open(id),
        "hackrf" => hackrf::open(id),
        "hermeslite2" => hermeslite2::open(id),
        "icom" => icom::open(id),
        "kiwisdr" => kiwisdr::open(id),
        "limesdr" => limesdr::open(id),
        "nrspst" => nrspst::open(id),
        "plutosdr" => plutosdr::open(id),
        "rfnm" => rfnm::open(id),
        "rtlsdr" => rtlsdr::open(id),
        "rtltcp" => rtltcp::open(id),
        "rx888" => rx888::open(id),
        "sdrpp" => sdrpp::open(id),
        "sdrplay" => sdrplay::open(id),
        "soapy" => soapy::open(id),
        "spyserver" => spyserver::open(id),
        "tci" => tci::open(id),
        "usrp" => usrp::open(id),
        other => Err(format!("unknown radio {other:?}")),
    }
}

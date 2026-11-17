//! Driver for the RFNM boards (rfnm.com): a motherboard with up to two daughterboards,
//! each with one or more receive channels, over USB or Ethernet.
//!
//! Status: partial. The vendor library (librfnm) is found at run time, but its
//! interface is C++ only, so this program cannot call it through the run-time loader.
//! Until a small C wrapper library exists, `probe` lists nothing and `open` reports
//! why. The parts that do not touch the library are written and tested here: the
//! sample-rate choice, gain clamping, and sample conversion.

use super::common::load_library;
use crate::{DeviceInfo, RxRadio};

/// The top sample rate of the boards, in Hz. Every rate is this value divided by a power of two.
const TOP_RATE_HZ: u32 = 122_880_000;
/// Number of rates: 122.88 MHz down to 7.68 MHz.
const RATE_STEPS: u32 = 5;

/// Library names to try, in order, for this operating system.
#[cfg(target_os = "macos")]
pub const LIBRARY_NAMES: &[&str] = &["librfnm.dylib"];
#[cfg(all(unix, not(target_os = "macos")))]
pub const LIBRARY_NAMES: &[&str] = &["librfnm.so", "librfnm.so.1"];
#[cfg(windows)]
pub const LIBRARY_NAMES: &[&str] = &["rfnm.dll"];
#[cfg(not(any(unix, windows)))]
pub const LIBRARY_NAMES: &[&str] = &[];

/// The sample rates the boards offer, in ascending order (7.68 to 122.88 Msps).
pub fn sample_rates() -> Vec<u32> {
    (0..RATE_STEPS).rev().map(|n| TOP_RATE_HZ >> n).collect()
}

/// Picks the smallest rate that is at least `want_hz` (within 0.1 %). If none is fast enough,
/// the largest rate is used. `want_hz` of 0 gives the smallest rate.
/// `rates` must be in ascending order and not empty.
pub fn choose_rate(want_hz: u32, rates: &[u32]) -> u32 {
    let want = want_hz as f64 * 0.999;
    for &r in rates {
        if r as f64 >= want {
            return r;
        }
    }
    *rates.last().expect("rate list is never empty")
}

/// Clamps a requested gain to the stage range and rounds it to a whole dB.
/// Returns `None` when the stage has no range (`max <= min`), which means no gain stage.
/// A NaN request is treated as the minimum.
pub fn clamp_gain_db(requested: f32, min: i32, max: i32) -> Option<i32> {
    if max <= min {
        return None;
    }
    let value = if requested.is_nan() { min as f32 } else { requested };
    Some(value.clamp(min as f32, max as f32).round() as i32)
}

/// Converts interleaved float I/Q (I, Q, I, Q, ...) into complex pairs.
/// Values are limited to -1.0 to 1.0. Returns the number of pairs written,
/// which is the smaller of the pairs available and the space in `out`.
/// A trailing odd value is ignored. NaN becomes 0.0.
pub fn convert_iq(src: &[f32], out: &mut [(f32, f32)]) -> usize {
    let n = (src.len() / 2).min(out.len());
    for i in 0..n {
        out[i] = (clean(src[2 * i]), clean(src[2 * i + 1]));
    }
    n
}

fn clean(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-1.0, 1.0)
    }
}

/// The error text when the vendor library cannot be loaded. `detail` is the loader's message.
pub fn missing_library_text(detail: &str) -> String {
    format!(
        "RFNM: the vendor library is missing or cannot be loaded ({detail}). \
         Install the RFNM library (librfnm) next to the program or in the system library path, then try again."
    )
}

/// Lists RFNM radios. Returns an empty list: enumeration needs the C wrapper described above.
pub fn probe() -> Vec<DeviceInfo> {
    Vec::new()
}

/// Opens an RFNM radio. Always fails for now; the message says what is missing.
pub fn open(_id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let lib = load_library(LIBRARY_NAMES).map_err(|e| missing_library_text(&e))?;
    drop(lib);
    Err("RFNM: the library was found, but its interface is C++ only and cannot be called from this program yet. \
         A small C wrapper library is needed."
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_the_power_of_two_steps() {
        assert_eq!(
            sample_rates(),
            vec![7_680_000, 15_360_000, 30_720_000, 61_440_000, 122_880_000]
        );
    }

    #[test]
    fn rate_choice_takes_the_smallest_fast_enough() {
        let r = sample_rates();
        assert_eq!(choose_rate(2_000_000, &r), 7_680_000);
        assert_eq!(choose_rate(7_680_000, &r), 7_680_000);
        assert_eq!(choose_rate(7_690_000, &r), 15_360_000);
        assert_eq!(choose_rate(20_000_000, &r), 30_720_000);
        assert_eq!(choose_rate(0, &r), 7_680_000);
    }

    #[test]
    fn rate_choice_falls_back_to_the_largest() {
        let r = sample_rates();
        assert_eq!(choose_rate(500_000_000, &r), 122_880_000);
    }

    #[test]
    fn gain_is_clamped_and_rounded() {
        assert_eq!(clamp_gain_db(10.4, 0, 60), Some(10));
        assert_eq!(clamp_gain_db(10.6, 0, 60), Some(11));
        assert_eq!(clamp_gain_db(-5.0, -63, 26), Some(-5));
        assert_eq!(clamp_gain_db(-100.0, -63, 26), Some(-63));
        assert_eq!(clamp_gain_db(100.0, -63, 26), Some(26));
        assert_eq!(clamp_gain_db(f32::NAN, 0, 60), Some(0));
    }

    #[test]
    fn no_gain_range_means_no_stage() {
        assert_eq!(clamp_gain_db(10.0, 0, 0), None);
        assert_eq!(clamp_gain_db(10.0, 5, 3), None);
    }

    #[test]
    fn conversion_pairs_samples_and_limits_range() {
        let src = [0.5, -0.5, 2.0, -3.0, f32::NAN, 0.25, 0.9];
        let mut out = [(0.0f32, 0.0f32); 8];
        let n = convert_iq(&src, &mut out);
        assert_eq!(n, 3);
        assert_eq!(out[0], (0.5, -0.5));
        assert_eq!(out[1], (1.0, -1.0));
        assert_eq!(out[2], (0.0, 0.25));
    }

    #[test]
    fn conversion_stops_at_output_size() {
        let src = [0.1, 0.2, 0.3, 0.4];
        let mut out = [(0.0f32, 0.0f32); 1];
        assert_eq!(convert_iq(&src, &mut out), 1);
        assert_eq!(out[0], (0.1, 0.2));
    }

    #[test]
    fn missing_library_error_says_what_to_do() {
        let err = load_library(&["kahf-no-such-library-for-test.so"]).unwrap_err();
        let text = missing_library_text(&err);
        assert!(text.starts_with("RFNM:"));
        assert!(text.contains("librfnm"));
        assert!(text.contains("kahf-no-such-library-for-test.so"));
    }

    #[test]
    fn probe_lists_nothing_yet() {
        assert!(probe().is_empty());
    }
}

//! HackRF One / Pro over libhackrf (Homebrew on macOS, system library on Linux).
//!
//! Hand-written FFI: only the calls this module uses are declared. The radio sends and takes
//! signed 8-bit I/Q pairs. They map to f32 in -1..1 by dividing by 128 (and back on transmit).
//! Call order follows the usual HackRF library sequence.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use kahf_core::io::{IqSink, IqSource};

const HACKRF_SUCCESS: c_int = 0;
/// Receive backlog cap in I/Q samples (about 0.4 s at 10 Msps). Oldest samples go first on overflow.
const RX_LIMIT: usize = 1 << 22;
/// Transmit backlog cap in bytes (two per sample).
const TX_LIMIT: usize = 1 << 22;
/// How long a read or write waits for the radio before it reports a stall.
const STALL: Duration = Duration::from_secs(5);

/// libhackrf's `hackrf_transfer`, passed to the sample callbacks.
#[repr(C)]
pub struct HackrfTransfer {
    device: *mut c_void,
    buffer: *mut u8,
    buffer_length: c_int,
    valid_length: c_int,
    rx_ctx: *mut c_void,
    tx_ctx: *mut c_void,
}

/// libhackrf's `hackrf_device_list_t` (only the serial numbers are read here).
#[repr(C)]
pub struct HackrfDeviceList {
    serial_numbers: *mut *mut c_char,
    usb_board_ids: *mut c_void,
    usb_device_index: *mut c_int,
    devicecount: c_int,
    usb_devices: *mut *mut c_void,
    usb_devicecount: c_int,
}

/// `int (*)(hackrf_transfer*)`: return 0 to keep streaming, non-zero to stop.
pub type SampleBlockCb = unsafe extern "C" fn(*mut HackrfTransfer) -> c_int;

crate::dl::foreign_fns! {
hackrf;
fn hackrf_init() -> c_int;
fn hackrf_exit() -> c_int;
fn hackrf_open(device: *mut *mut c_void) -> c_int;
fn hackrf_open_by_serial(serial: *const c_char, device: *mut *mut c_void) -> c_int;
fn hackrf_close(device: *mut c_void) -> c_int;
fn hackrf_set_freq(device: *mut c_void, freq_hz: u64) -> c_int;
fn hackrf_set_sample_rate(device: *mut c_void, freq_hz: f64) -> c_int;
fn hackrf_set_baseband_filter_bandwidth(device: *mut c_void, bandwidth_hz: u32) -> c_int;
fn hackrf_board_id_read(device: *mut c_void, value: *mut u8) -> c_int;
fn hackrf_set_lna_gain(device: *mut c_void, value: u32) -> c_int;
fn hackrf_set_vga_gain(device: *mut c_void, value: u32) -> c_int;
fn hackrf_set_txvga_gain(device: *mut c_void, value: u32) -> c_int;
fn hackrf_set_amp_enable(device: *mut c_void, value: u8) -> c_int;
fn hackrf_start_rx(device: *mut c_void, cb: SampleBlockCb, rx_ctx: *mut c_void) -> c_int;
fn hackrf_start_tx(device: *mut c_void, cb: SampleBlockCb, tx_ctx: *mut c_void) -> c_int;
fn hackrf_stop_rx(device: *mut c_void) -> c_int;
fn hackrf_stop_tx(device: *mut c_void) -> c_int;
fn hackrf_is_streaming(device: *mut c_void) -> c_int;
fn hackrf_device_list() -> *mut HackrfDeviceList;
fn hackrf_device_list_free(list: *mut HackrfDeviceList);
fn hackrf_error_name(errcode: c_int) -> *const c_char;}

/// Baseband filter widths of the MAX2837 (the hackrf_usb.cpp, kFilterHz).
const FILTER_HZ: [u32; 16] = [
    1_750_000, 2_500_000, 3_500_000, 5_000_000, 5_500_000, 6_000_000, 7_000_000, 8_000_000, 9_000_000,
    10_000_000, 12_000_000, 14_000_000, 15_000_000, 20_000_000, 24_000_000, 28_000_000,
];

/// Largest filter width at or below the wish (the filterBelow, not the strict "below" variant).
/// At 10 Msps the wish is 5 MHz and that is the width kept, as the does.
fn filter_below(wish_hz: u32) -> u32 {
    let mut i = 0;
    while i < FILTER_HZ.len() && FILTER_HZ[i] < wish_hz {
        i += 1;
    }
    if i == FILTER_HZ.len() || (i > 0 && FILTER_HZ[i] > wish_hz) {
        i -= 1;
    }
    FILTER_HZ[i]
}

/// Tuning range per board id (the hackrfBoardRange). None when the board is not known.
fn board_range_hz(id: u8) -> Option<(f64, f64)> {
    match id {
        1 => Some((10e6, 6e9)),    // Jawbreaker
        2 | 4 => Some((1e6, 6e9)), // HackRF One (r8 and r9+)
        3 => Some((50e6, 4e9)),    // rad1o
        5 => Some((0.1e6, 6e9)),   // HackRF Pro
        _ => None,
    }
}

/// int8 I/Q sample to f32 in -1..1. Receive scale: divide by 128 (the hackrf_usb.cpp, onTransfer).
pub fn i8_to_f32(v: i8) -> f32 {
    f32::from(v) / 128.0
}

/// f32 in -1..1 to int8 for transmit. the hackrfTxPack: scale by 127, round half to even, clamp to -127..127,
/// so full scale is +127 and -127 (never -128). NaN becomes 0.
pub fn f32_to_i8(v: f32) -> i8 {
    if v.is_nan() {
        return 0;
    }
    (v * 127.0).round_ties_even().clamp(-127.0, 127.0) as i8
}

fn error_text(code: c_int) -> String {
    let p = unsafe { hackrf_error_name(code) };
    if p.is_null() {
        return format!("error {code}");
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn check(what: &str, code: c_int) -> Result<(), String> {
    if code == HACKRF_SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: {}", error_text(code)))
    }
}

/// Reference count for hackrf_init/hackrf_exit, so several devices can share the library.
static INIT_COUNT: Mutex<usize> = Mutex::new(0);

/// What to tell the operator when the HackRF library cannot be found.
pub(crate) const HACKRF_MISSING: &str = "The HackRF library is not installed. Install libhackrf: Homebrew \"brew install hackrf\" on macOS, the libhackrf package on Linux, or the HackRF driver package on Windows.";

fn acquire() -> Result<(), String> {
    if crate::dl::hackrf().is_none() {
        return Err(HACKRF_MISSING.to_string());
    }
    let mut n = INIT_COUNT.lock().unwrap_or_else(|e| e.into_inner());
    if *n == 0 {
        check("hackrf_init", unsafe { hackrf_init() })?;
    }
    *n += 1;
    Ok(())
}

fn release() {
    let mut n = INIT_COUNT.lock().unwrap_or_else(|e| e.into_inner());
    if *n == 0 {
        return;
    }
    *n -= 1;
    if *n == 0 {
        unsafe {
            hackrf_exit();
        }
    }
}

/// An open device. Closing it (and releasing the library) happens on drop.
struct Session {
    dev: *mut c_void,
}

// The handle is only used through libhackrf, which allows calls from any thread.
unsafe impl Send for Session {}

impl Session {
    fn open(serial: Option<&str>) -> Result<Self, String> {
        acquire()?;
        let mut dev: *mut c_void = std::ptr::null_mut();
        let result = match serial {
            Some(s) => match CString::new(s) {
                Ok(c) => check("hackrf_open_by_serial", unsafe {
                    hackrf_open_by_serial(c.as_ptr(), &mut dev)
                }),
                Err(_) => Err("HackRF serial contains a NUL byte".to_string()),
            },
            None => check("hackrf_open", unsafe { hackrf_open(&mut dev) }),
        };
        match result {
            Ok(()) if !dev.is_null() => Ok(Self { dev }),
            Ok(()) => {
                release();
                Err("hackrf_open returned no device".to_string())
            }
            Err(e) => {
                release();
                Err(e)
            }
        }
    }

    /// Order as the driver: sample rate, then baseband filter, then centre frequency.
    fn tune(&self, freq_hz: u64, rate_hz: f64) -> Result<(), String> {
        if !(rate_hz.is_finite() && rate_hz > 0.0) {
            return Err(format!("invalid HackRF sample rate {rate_hz}"));
        }
        // the advertised HackRF sample-rate range (2 to 20 Msps).
        if !(2e6..=20e6).contains(&rate_hz) {
            return Err(format!(
                "HackRF sample rate {} Msps is outside 2 to 20 Msps",
                rate_hz / 1e6
            ));
        }
        // Frequency range by board. Checked before anything is sent so a refused tune leaves the radio as it was.
        let mut board: u8 = 0xff;
        let board_known = unsafe { hackrf_board_id_read(self.dev, &mut board) } == HACKRF_SUCCESS;
        if let Some((lo, hi)) = board_known.then(|| board_range_hz(board)).flatten() {
            let f = freq_hz as f64;
            if f < lo || f > hi {
                return Err(format!(
                    "frequency {} MHz is outside this HackRF's range {} to {} MHz",
                    f / 1e6,
                    lo / 1e6,
                    hi / 1e6
                ));
            }
        }
        check("hackrf_set_sample_rate", unsafe {
            hackrf_set_sample_rate(self.dev, rate_hz)
        })?;
        // libhackrf's own sample-rate call picks a wider filter (about 0.75 of the rate). the sets the filter
        // explicitly: the largest width at or below half the sample rate (5 MHz at 10 Msps).
        let filter = filter_below((rate_hz * 0.5) as u32);
        check("hackrf_set_baseband_filter_bandwidth", unsafe {
            hackrf_set_baseband_filter_bandwidth(self.dev, filter)
        })?;
        check("hackrf_set_freq", unsafe { hackrf_set_freq(self.dev, freq_hz) })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            hackrf_close(self.dev);
        }
        release();
    }
}

/// Bounded queue shared between a libhackrf callback thread and the caller.
struct Queue<T> {
    state: Mutex<QueueState<T>>,
    cv: Condvar,
}

struct QueueState<T> {
    items: VecDeque<T>,
    closed: bool,
    /// Receive: samples dropped on overflow. Transmit: items padded with silence on underrun.
    lost: u64,
}

impl<T> Queue<T> {
    fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                items: VecDeque::new(),
                closed: false,
                lost: 0,
            }),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, QueueState<T>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn close(&self) {
        self.lock().closed = true;
        self.cv.notify_all();
    }
}

unsafe extern "C" fn rx_callback(t: *mut HackrfTransfer) -> c_int {
    let t = unsafe { &*t };
    if t.buffer.is_null() || t.valid_length <= 0 {
        return 0;
    }
    let q = unsafe { &*(t.rx_ctx as *const Queue<(f32, f32)>) };
    let bytes = unsafe { std::slice::from_raw_parts(t.buffer as *const i8, t.valid_length as usize) };
    let mut st = q.lock();
    if st.closed {
        return -1;
    }
    for pair in bytes.as_chunks::<2>().0 {
        if st.items.len() >= RX_LIMIT {
            st.items.pop_front();
            st.lost += 1;
        }
        st.items.push_back((i8_to_f32(pair[0]), i8_to_f32(pair[1])));
    }
    drop(st);
    q.cv.notify_all();
    0
}

unsafe extern "C" fn tx_callback(t: *mut HackrfTransfer) -> c_int {
    let t = unsafe { &mut *t };
    if t.buffer.is_null() || t.buffer_length <= 0 {
        return 0;
    }
    let len = t.buffer_length as usize;
    let out = unsafe { std::slice::from_raw_parts_mut(t.buffer, len) };
    let q = unsafe { &*(t.tx_ctx as *const Queue<u8>) };
    let mut st = q.lock();
    if st.closed {
        return -1;
    }
    let n = len.min(st.items.len());
    for (slot, b) in out.iter_mut().zip(st.items.drain(..n)) {
        *slot = b;
    }
    // Underrun: fill the rest with silence so the radio keeps its clock.
    for slot in &mut out[n..] {
        *slot = 0;
    }
    st.lost += (len - n) as u64;
    t.valid_length = t.buffer_length;
    drop(st);
    q.cv.notify_all();
    0
}

/// HackRF receiver. Implements `IqSource`.
pub struct HackrfRx {
    q: Arc<Queue<(f32, f32)>>,
    rate_hz: f64,
    session: Session,
}

impl HackrfRx {
    /// Opens the radio (first one if `serial` is None), tunes, sets gains and starts receiving.
    /// `lna_db` is clamped to 0..40 in 8 dB steps, `vga_db` to 0..62 in 2 dB steps.
    pub fn open(
        serial: Option<&str>,
        freq_hz: u64,
        sample_rate_hz: f64,
        lna_db: u32,
        vga_db: u32,
        amp_on: bool,
    ) -> Result<Self, String> {
        let session = Session::open(serial)?;
        session.tune(freq_hz, sample_rate_hz)?;
        check("hackrf_set_lna_gain", unsafe {
            hackrf_set_lna_gain(session.dev, lna_db.min(40) & !7)
        })?;
        check("hackrf_set_vga_gain", unsafe {
            hackrf_set_vga_gain(session.dev, vga_db.min(62) & !1)
        })?;
        check("hackrf_set_amp_enable", unsafe {
            hackrf_set_amp_enable(session.dev, amp_on as u8)
        })?;
        let q = Arc::new(Queue::new());
        let ctx = Arc::as_ptr(&q) as *mut c_void;
        check("hackrf_start_rx", unsafe {
            hackrf_start_rx(session.dev, rx_callback, ctx)
        })?;
        Ok(Self {
            q,
            rate_hz: sample_rate_hz,
            session,
        })
    }

    /// Samples dropped because the caller fell behind.
    pub fn dropped_samples(&self) -> u64 {
        self.q.lock().lost
    }

    /// True while libhackrf is still delivering samples.
    pub fn is_streaming(&self) -> bool {
        unsafe { hackrf_is_streaming(self.session.dev) == 1 }
    }
}

impl Drop for HackrfRx {
    fn drop(&mut self) {
        // Close the queue first so the callback returns, then stop; libhackrf waits for its thread.
        self.q.close();
        unsafe {
            hackrf_stop_rx(self.session.dev);
        }
    }
}

impl IqSource for HackrfRx {
    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut st = self.q.lock();
        loop {
            if !st.items.is_empty() {
                let n = out.len().min(st.items.len());
                for (slot, s) in out.iter_mut().zip(st.items.drain(..n)) {
                    *slot = s;
                }
                return Ok(n);
            }
            if st.closed {
                return Err("HackRF receive stopped".to_string());
            }
            let (g, timeout) = self
                .q
                .cv
                .wait_timeout(st, STALL)
                .unwrap_or_else(|e| e.into_inner());
            st = g;
            if timeout.timed_out() && st.items.is_empty() {
                return Err(format!("no samples from the HackRF for {} s", STALL.as_secs()));
            }
        }
    }

    fn sample_rate(&self) -> f64 {
        self.rate_hz
    }
}

/// HackRF transmitter. Implements `IqSink`.
pub struct HackrfTx {
    q: Arc<Queue<u8>>,
    rate_hz: f64,
    session: Session,
}

impl HackrfTx {
    /// Opens the radio, tunes, sets the TX VGA gain (0..47 dB) and the amplifier, and starts transmitting.
    /// Transmit is silent until samples are written; the radio gets zeros while the queue is empty.
    pub fn open(
        serial: Option<&str>,
        freq_hz: u64,
        sample_rate_hz: f64,
        txvga_db: u32,
        amp_on: bool,
    ) -> Result<Self, String> {
        let session = Session::open(serial)?;
        session.tune(freq_hz, sample_rate_hz)?;
        check("hackrf_set_txvga_gain", unsafe {
            hackrf_set_txvga_gain(session.dev, txvga_db.min(47))
        })?;
        check("hackrf_set_amp_enable", unsafe {
            hackrf_set_amp_enable(session.dev, amp_on as u8)
        })?;
        let q = Arc::new(Queue::new());
        let ctx = Arc::as_ptr(&q) as *mut c_void;
        // the shut() turns the amplifier back off when transmit does not start; do the same here.
        if let Err(e) = check("hackrf_start_tx", unsafe {
            hackrf_start_tx(session.dev, tx_callback, ctx)
        }) {
            unsafe {
                hackrf_set_amp_enable(session.dev, 0);
            }
            return Err(e);
        }
        Ok(Self {
            q,
            rate_hz: sample_rate_hz,
            session,
        })
    }

    /// Samples padded with silence because the radio ran out of data.
    pub fn underrun_samples(&self) -> u64 {
        self.q.lock().lost / 2
    }
}

impl Drop for HackrfTx {
    fn drop(&mut self) {
        self.q.close();
        unsafe {
            hackrf_stop_tx(self.session.dev);
            // the shut(): the amplifier goes back off when transmit stops (hackrf_stop_tx does not do it).
            hackrf_set_amp_enable(self.session.dev, 0);
        }
    }
}

impl IqSink for HackrfTx {
    fn write_iq(&mut self, iq: &[(f32, f32)]) -> Result<(), String> {
        let mut st = self.q.lock();
        for &(i, q) in iq {
            for b in [f32_to_i8(i) as u8, f32_to_i8(q) as u8] {
                while st.items.len() >= TX_LIMIT {
                    if st.closed {
                        return Err("HackRF transmit stopped".to_string());
                    }
                    let (g, timeout) = self
                        .q
                        .cv
                        .wait_timeout(st, STALL)
                        .unwrap_or_else(|e| e.into_inner());
                    st = g;
                    if timeout.timed_out() && st.items.len() >= TX_LIMIT {
                        return Err("the HackRF is not taking samples".to_string());
                    }
                }
                if st.closed {
                    return Err("HackRF transmit stopped".to_string());
                }
                st.items.push_back(b);
            }
        }
        Ok(())
    }

    fn sample_rate(&self) -> f64 {
        self.rate_hz
    }
}

/// Serial numbers of the HackRF devices that are connected. Empty if none or the library fails.
pub fn list_devices() -> Vec<String> {
    if acquire().is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let list = unsafe { hackrf_device_list() };
    if !list.is_null() {
        unsafe {
            let l = &*list;
            for i in 0..l.devicecount.max(0) as usize {
                let p = *l.serial_numbers.add(i);
                if !p.is_null() {
                    out.push(CStr::from_ptr(p).to_string_lossy().into_owned());
                }
            }
            hackrf_device_list_free(list);
        }
    }
    release();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hackrf_i8_to_f32_scale() {
        assert_eq!(i8_to_f32(0), 0.0);
        assert_eq!(i8_to_f32(64), 0.5);
        assert_eq!(i8_to_f32(-128), -1.0);
        assert_eq!(i8_to_f32(127), 127.0 / 128.0);
    }

    #[test]
    fn hackrf_f32_to_i8_rounds_and_clamps() {
        // the hackrfTxPack: scale 127, symmetric clamp at -127..127
        assert_eq!(f32_to_i8(0.0), 0);
        assert_eq!(f32_to_i8(0.5), 64); // 63.5 rounds to even 64
        assert_eq!(f32_to_i8(-0.5), -64);
        assert_eq!(f32_to_i8(1.0), 127);
        assert_eq!(f32_to_i8(-1.0), -127);
        assert_eq!(f32_to_i8(2.0), 127);
        assert_eq!(f32_to_i8(-2.0), -127);
        assert_eq!(f32_to_i8(f32::INFINITY), 127);
        assert_eq!(f32_to_i8(f32::NEG_INFINITY), -127);
        assert_eq!(f32_to_i8(f32::NAN), 0);
    }

    #[test]
    fn hackrf_filter_below_rule() {
        // 10 Msps: half the rate is exactly 5 MHz, kept as 5 MHz (not stepped down to 3.5)
        assert_eq!(filter_below(5_000_000), 5_000_000);
        assert_eq!(filter_below(4_000_000), 3_500_000);
        assert_eq!(filter_below(2_000_000), 1_750_000);
        assert_eq!(filter_below(40_000_000), 28_000_000);
    }

    #[test]
    fn hackrf_board_ranges_table() {
        assert_eq!(board_range_hz(2), Some((1e6, 6e9)));
        assert_eq!(board_range_hz(5), Some((0.1e6, 6e9)));
        assert_eq!(board_range_hz(0), None);
    }
}

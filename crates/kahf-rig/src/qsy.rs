//! Moving the radio to KAHF channels on 17 m. KAHF listens to audio from 0 to 4 kHz above
//! the dial in upper sideband, so a channel's dial is its centre minus the audio centre.
use crate::{Mode, Rig};
use kahf_core::bandplan::{self, KAHF_CHANNEL_WIDTH_HZ, RELAY_SEGMENT};

/// Audio frequency where KAHF puts a channel's centre when the dial is placed for it.
pub const AUDIO_CENTRE_HZ: u64 = 1_500;

/// The 17 m amateur band (IARU Region 1). The QSY helpers never leave it.
pub const BAND_17M: (u64, u64) = (18_068_000, 18_168_000);

pub fn relay_slot_count() -> usize {
    ((RELAY_SEGMENT.upper_hz - RELAY_SEGMENT.lower_hz) / KAHF_CHANNEL_WIDTH_HZ) as usize
}

/// Centre frequency of relay slot `slot` (0 based), or None past the last slot.
pub fn relay_slot_centre_hz(slot: usize) -> Option<u64> {
    (slot < relay_slot_count())
        .then(|| RELAY_SEGMENT.lower_hz + KAHF_CHANNEL_WIDTH_HZ / 2 + slot as u64 * KAHF_CHANNEL_WIDTH_HZ)
}

/// Dial frequency that puts `channel_hz` at the audio centre. None outside 17 m.
pub fn dial_for_channel(channel_hz: u64) -> Option<u64> {
    let half = KAHF_CHANNEL_WIDTH_HZ / 2;
    if channel_hz < BAND_17M.0 + half || channel_hz + half > BAND_17M.1 {
        return None;
    }
    Some(channel_hz - AUDIO_CENTRE_HZ)
}

/// Tunes the radio to a channel: dial, then upper-sideband data mode, then a read back.
/// Returns the dial frequency. Never keys the transmitter.
pub fn qsy_to_channel(rig: &mut dyn Rig, channel_hz: u64) -> Result<u64, String> {
    let dial = dial_for_channel(channel_hz)
        .ok_or_else(|| format!("{channel_hz} Hz is outside 17 m; KAHF stays on 17 m"))?;
    rig.set_freq_hz(dial)?;
    rig.set_mode(Mode::UsbData)?;
    let got = rig.freq_hz()?;
    if got.abs_diff(dial) > 10 {
        return Err(format!("the radio is at {got} Hz, not {dial} Hz"));
    }
    Ok(dial)
}

/// Tunes to relay slot `slot` (0 based) inside the relay segment.
pub fn qsy_to_relay_slot(rig: &mut dyn Rig, slot: usize) -> Result<u64, String> {
    let c = relay_slot_centre_hz(slot)
        .ok_or_else(|| format!("relay slot {slot} does not exist ({} slots)", relay_slot_count()))?;
    debug_assert!(bandplan::allowed_segment(c, KAHF_CHANNEL_WIDTH_HZ).is_some());
    qsy_to_channel(rig, c)
}

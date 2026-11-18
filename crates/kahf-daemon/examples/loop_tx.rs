//! Transmits a KAHF test signal on a HackRF in a loop, for checking the
//! transmitter and receiving it with another SDR (for example in SDR++).
//!
//! Run from the repository root:
//!   cargo run --release -p kahf-daemon --example loop_tx -- \
//!       [--freq-hz 145500000] [--txvga-db 30] [--seconds 60] [--text "KAHF TEST"] [--serial SN]
//!
//! The amplifier stays off. Use a dummy load or a suitable antenna.

use kahf_core::frame;
use kahf_core::io::IqSink;
use kahf_core::ldpc;
use kahf_core::phy::{self, Modulation};
use kahf_daemon::audio_to_iq;
use kahf_sdr::hackrf::HackrfTx;
use std::time::{Duration, Instant};

/// Audio rate of the modem.
const AUDIO_RATE_HZ: usize = 8_000;
/// HackRF sample rate used by the daemon.
const SAMPLE_RATE_HZ: f64 = 2_400_000.0;
/// Payload bytes per frame (same as the `kahf tx` command).
const CHUNK_BYTES: usize = 36;
/// Silence between frames: 0.2 s of audio.
const GAP_SAMPLES: usize = AUDIO_RATE_HZ / 5;

struct Options {
    freq_hz: u64,
    txvga_db: u32,
    seconds: u64,
    text: String,
    serial: Option<String>,
    channels: usize,
}

fn parse_args() -> Result<Options, String> {
    let mut o = Options {
        freq_hz: 145_500_000,
        txvga_db: 30,
        seconds: 60,
        text: "KAHF TEST".to_string(),
        serial: None,
        channels: 1,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let val = args.get(i + 1).ok_or_else(|| format!("{flag} needs a value"))?;
        match flag {
            "--freq-hz" => o.freq_hz = val.parse().map_err(|_| format!("bad --freq-hz: {val}"))?,
            "--txvga-db" => o.txvga_db = val.parse().map_err(|_| format!("bad --txvga-db: {val}"))?,
            "--seconds" => o.seconds = val.parse().map_err(|_| format!("bad --seconds: {val}"))?,
            "--text" => o.text = val.clone(),
            "--serial" => o.serial = Some(val.clone()),
            "--channels" => o.channels = val.parse().map_err(|_| format!("bad --channels: {val}"))?,
            other => return Err(format!("unknown option {other}")),
        }
        i += 2;
    }
    if o.channels == 0 || o.channels > 4 {
        return Err("--channels must be 1 to 4".into());
    }
    if o.text.is_empty() {
        return Err("--text must not be empty".into());
    }
    Ok(o)
}

/// Encodes the text as KAHF frames at audio rate, with a gap between frames.
fn encode_audio(data: &[u8]) -> Result<Vec<f32>, String> {
    let mut out = Vec::new();
    for (n, chunk) in data.chunks(CHUNK_BYTES).enumerate() {
        if n > 0 {
            out.extend(std::iter::repeat_n(0.0f32, GAP_SAMPLES));
        }
        let bits = frame::build_payload(chunk);
        if bits.len() != ldpc::K {
            return Err(format!(
                "build_payload gave {} bits, expected {}",
                bits.len(),
                ldpc::K
            ));
        }
        let coded = ldpc::encode(&bits);
        out.extend(phy::modulate_frame(&coded, Modulation::Qpsk));
    }
    Ok(out)
}

/// Spacing of the channel grid in Hz.
const CHANNEL_HZ: f64 = 2_700.0;

/// Shifts each channel's IQ to its offset from the centre and adds them.
/// The sum is divided by the number of channels to keep the peak in range.
fn combine_channels(parts: &[(Vec<(f32, f32)>, f64)]) -> Vec<(f32, f32)> {
    let len = parts.iter().map(|(iq, _)| iq.len()).max().unwrap_or(0);
    let scale = 1.0 / parts.len() as f64;
    let fs = SAMPLE_RATE_HZ;
    let mut out = vec![(0f32, 0f32); len];
    for (iq, offset) in parts {
        let w = 2.0 * std::f64::consts::PI * offset / fs;
        for (n, &(i, q)) in iq.iter().enumerate() {
            let (s, c) = (w * n as f64).sin_cos();
            // (i + jq) * (c + js)
            let re = i as f64 * c - q as f64 * s;
            let im = i as f64 * s + q as f64 * c;
            out[n].0 += (re * scale) as f32;
            out[n].1 += (im * scale) as f32;
        }
    }
    out
}

fn run() -> Result<(), String> {
    let o = parse_args()?;
    let (iq, centre_hz) = if o.channels == 1 {
        let audio = encode_audio(o.text.as_bytes())?;
        (audio_to_iq(&audio), o.freq_hz as f64)
    } else {
        // Channel centres 2.7 kHz apart, starting at the first 20 m channel.
        let mut parts = Vec::new();
        for k in 0..o.channels {
            let audio = encode_audio(format!("{} CH{}", o.text, k + 1).as_bytes())?;
            let offset = (k as f64 - (o.channels as f64 - 1.0) / 2.0) * CHANNEL_HZ;
            parts.push((audio_to_iq(&audio), offset));
        }

        (combine_channels(&parts), o.freq_hz as f64)
    };
    eprintln!(
        "loop_tx: {} Hz, TX VGA {} dB, amplifier off, {:.2} s per loop, running {} s",
        o.freq_hz,
        o.txvga_db,
        iq.len() as f64 / SAMPLE_RATE_HZ,
        o.seconds
    );

    let mut tx = HackrfTx::open(
        o.serial.as_deref(),
        centre_hz as u64,
        SAMPLE_RATE_HZ,
        o.txvga_db,
        false,
    )?;
    let end = Instant::now() + Duration::from_secs(o.seconds);
    let mut loops = 0u64;
    while Instant::now() < end {
        tx.write_iq(&iq)?;
        loops += 1;
    }
    drop(tx);
    eprintln!("loop_tx: done, {loops} loops sent");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

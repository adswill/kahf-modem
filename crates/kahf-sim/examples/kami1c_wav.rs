//! Writes a KAMI-1c test signal as a stereo I/Q wav (48 kHz, 16 bit) for SDR++ (File source), and
//! the same as a mono audio wav. Several frames with gaps, so the burst shape is easy to see.
//! Run: cargo run --release -p kahf-sim --example kami1c_wav -- /path/to/folder [1c|8]
//! Frames are sent back to back (0.1 s apart) for about 15 s, like a real burst.
use kahf_core::phy::{modulate_frame, Modulation};

const RATE: u32 = 48_000;
/// KAMI's own sample rate; the wav is converted up to RATE so SDR++ shows the true width (2.4 kHz).
const FS: usize = 8_000;
const UP: usize = 6;

fn wav(path: &str, channels: u16, data: &[i16]) {
    let bytes = (data.len() * 2) as u32;
    let mut f = Vec::new();
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&(36 + bytes).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    f.extend_from_slice(&16u32.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&channels.to_le_bytes());
    f.extend_from_slice(&RATE.to_le_bytes());
    f.extend_from_slice(&(RATE * 2 * channels as u32).to_le_bytes());
    f.extend_from_slice(&(2 * channels).to_le_bytes());
    f.extend_from_slice(&16u16.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&bytes.to_le_bytes());
    for v in data {
        f.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, f).unwrap();
}

fn main() {
    let dir = std::env::args().nth(1).expect("folder");
    let (m, name) = match std::env::args().nth(2).as_deref() {
        Some("8") => (Modulation::Kami8, "kami8"),
        _ => (Modulation::Kami1c, "kami1c"),
    };
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = 4242u64;
    let mut audio: Vec<f32> = vec![0.0; FS]; // 1 s of silence first
    while audio.len() < FS * 15 {
        let bits: Vec<u8> = (0..648)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) & 1) as u8
            })
            .collect();
        audio.extend(modulate_frame(&bits, m));
        audio.extend(std::iter::repeat(0.0).take(FS / 10));
    }
    audio.extend(std::iter::repeat(0.0).take(FS));
    // 8 kHz -> 48 kHz: windowed-sinc interpolation.
    let half = 12i64;
    let low = audio;
    let mut audio = vec![0.0f32; low.len() * UP];
    for (t, out) in audio.iter_mut().enumerate() {
        let c = t as f64 / UP as f64;
        let k0 = c.floor() as i64;
        let mut acc = 0.0f64;
        for k in (k0 - half + 1)..=(k0 + half) {
            if k < 0 || k as usize >= low.len() {
                continue;
            }
            let x = c - k as f64;
            let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
            let w = 0.5 + 0.5 * (std::f64::consts::PI * x / half as f64).cos();
            acc += low[k as usize] as f64 * sinc * w;
        }
        *out = acc as f32;
    }
    let peak = audio.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let g = 0.7 / peak;

    // Analytic signal (Hilbert FIR, 255 taps) so the spectrum shows one side only, as a radio would.
    let n = 255usize;
    let mid = n / 2;
    let h: Vec<f32> = (0..n)
        .map(|i| {
            let k = i as i32 - mid as i32;
            let w = 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
            if k % 2 == 0 { 0.0 } else { w * 2.0 / (std::f32::consts::PI * k as f32) }
        })
        .collect();
    let mut iq = Vec::with_capacity(audio.len() * 2);
    for t in 0..audio.len() {
        let mut q = 0.0f32;
        for (i, c) in h.iter().enumerate() {
            if *c != 0.0 {
                let idx = t as i64 + mid as i64 - i as i64;
                if idx >= 0 && (idx as usize) < audio.len() {
                    q += c * audio[idx as usize];
                }
            }
        }
        iq.push((audio[t] * g * 32767.0) as i16);
        iq.push((q * g * 32767.0) as i16);
    }
    let mono: Vec<i16> = audio.iter().map(|v| (v * g * 32767.0) as i16).collect();
    wav(&format!("{dir}/{name}-iq-48k.wav"), 2, &iq);
    wav(&format!("{dir}/{name}-audio-48k.wav"), 1, &mono);
    eprintln!("wrote {dir}/{name}-iq-48k.wav and {name}-audio-48k.wav ({:.1} s)", audio.len() as f32 / RATE as f32);
}

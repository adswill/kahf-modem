//! `kahf tx <in.bin> -o out.wav`, `kahf rx <in.wav> -o out.bin`,
//! `kahf sim-tx <in.bin> -o out.wav --snr <dB>`,
//! `kahf bler --from <dB> --to <dB> --step <dB> --frames <n>`,
//! `kahf doctor`, `kahf e2e --profile <name> --snr <dB> --bytes <n> [--seed <n>]`.
use kahf_core::frame;
use kahf_core::ldpc;
use kahf_core::phy::{self, Modulation};
use kahf_sim::Profile;
use std::env;
use std::fs;
use std::process;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const SAMPLE_RATE: u32 = 8000;
const SILENCE_SAMPLES: usize = SAMPLE_RATE as usize / 5; // 0.2 s
const CHUNK_BYTES: usize = 36;
const RX_SCAN_STEP: usize = 576; // one symbol, N + CP = 512 + 64 (phy.rs SYM)
const MAX_LDPC_ITER: usize = 50;
const SIM_SEED: u64 = 0x4B41_4846; // "KAHF"
const MOD: Modulation = Modulation::Qpsk;

const USAGE: &str = "usage:
  kahf tx     <input_file> -o <out.wav>
  kahf rx     <in.wav> -o <out_file>
  kahf sim-tx <input_file> -o <out.wav> --snr <dB>
  kahf bler   --from <dB> --to <dB> --step <dB> --frames <n>
              (Monte-Carlo frame error rate, QPSK + LDPC, awgn SNR per real sample)
  kahf doctor (list audio, HackRF and RTL-SDR devices and check the SDR libraries)
  kahf audio-test [--device NAME] (read the input for 3 s and print its level)
  kahf e2e --profile <awgn|two-path|freq-offset|clock-offset|fade> --snr <dB> --bytes <n> [--seed <n>]
              (random file -> tx WAV -> channel -> rx WAV -> compare; exit 0 on PASS, 1 on FAIL)";

enum Fail {
    Usage(String),
    Run(String),
}

struct Opts {
    input: String,
    output: String,
    snr: Option<f32>,
}

fn parse_opts(args: &[String], want_snr: bool) -> Result<Opts, String> {
    let mut input = None;
    let mut output = None;
    let mut snr = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                output = Some(args.get(i).ok_or("-o needs a value")?.clone());
            }
            "--snr" if want_snr => {
                i += 1;
                let v = args.get(i).ok_or("--snr needs a value")?;
                snr = Some(v.parse::<f32>().map_err(|_| format!("bad --snr value: {v}"))?);
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => {
                if input.is_some() {
                    return Err(format!("unexpected extra argument {s}"));
                }
                input = Some(s.to_string());
            }
        }
        i += 1;
    }
    let input = input.ok_or("missing input file")?;
    let output = output.ok_or("missing -o <output>")?;
    if want_snr && snr.is_none() {
        return Err("sim-tx needs --snr <dB>".into());
    }
    Ok(Opts { input, output, snr })
}

/// Split data into 36-byte chunks, frame, LDPC-encode, modulate, and join with silence.
/// Returns the samples and the number of frames.
fn encode_stream(data: &[u8]) -> Result<(Vec<f32>, usize), String> {
    if data.is_empty() {
        return Err("input file is empty".into());
    }
    let silence = vec![0.0f32; SILENCE_SAMPLES];
    let mut out = Vec::new();
    let mut frames = 0usize;
    for chunk in data.chunks(CHUNK_BYTES) {
        let bits = frame::build_payload(chunk);
        if bits.len() != ldpc::K {
            return Err(format!(
                "build_payload gave {} bits, expected {}",
                bits.len(),
                ldpc::K
            ));
        }
        let coded = ldpc::encode(&bits);
        if coded.len() != ldpc::N {
            return Err(format!(
                "ldpc::encode gave {} bits, expected {}",
                coded.len(),
                ldpc::N
            ));
        }
        if frames > 0 {
            out.extend_from_slice(&silence);
        }
        out.extend(phy::modulate_frame(&coded, MOD));
        frames += 1;
    }
    Ok((out, frames))
}

fn write_wav(path: &str, samples: &[f32]) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| format!("cannot create {path}: {e}"))?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        w.write_sample(v).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

fn read_wav(path: &str) -> Result<Vec<f32>, String> {
    let mut r = hound::WavReader::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let spec = r.spec();
    if spec.channels != 1 || spec.sample_rate != SAMPLE_RATE {
        return Err(format!(
            "need mono {SAMPLE_RATE} Hz WAV, got {} ch at {} Hz",
            spec.channels, spec.sample_rate
        ));
    }
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => r
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0))
            .collect::<Result<Vec<f32>, hound::Error>>()
            .map_err(|e| e.to_string())?,
        (hound::SampleFormat::Float, 32) => r
            .samples::<f32>()
            .collect::<Result<Vec<f32>, hound::Error>>()
            .map_err(|e| e.to_string())?,
        _ => return Err("need 16-bit PCM or 32-bit float WAV".into()),
    };
    Ok(samples)
}

/// Scan the stream for frames. After a success, skip exactly one frame length;
/// on failure, advance by one symbol and retry, so a missed frame cannot hide
/// the frame behind it. Returns (bytes, frames passing CRC).
fn decode_stream(samples: &[f32]) -> (Vec<u8>, usize) {
    let frame_len = phy::modulate_frame(&vec![0u8; ldpc::N], MOD).len();
    let mut out = Vec::new();
    let mut frames = 0usize;
    let mut pos = 0usize;
    while pos < samples.len() {
        let decoded = phy::demodulate_frame(&samples[pos..], MOD, ldpc::N)
            .and_then(|llrs| ldpc::decode(&llrs, MAX_LDPC_ITER))
            .and_then(|info| frame::parse_payload(&info));
        match decoded {
            Some(bytes) => {
                out.extend_from_slice(&bytes);
                frames += 1;
                pos += frame_len + SILENCE_SAMPLES;
            }
            None => pos += RX_SCAN_STEP,
        }
    }
    (out, frames)
}

fn cmd_tx(o: &Opts) -> Result<(), String> {
    let data = fs::read(&o.input).map_err(|e| format!("cannot read {}: {e}", o.input))?;
    let (samples, frames) = encode_stream(&data)?;
    write_wav(&o.output, &samples)?;
    eprintln!(
        "tx: {} bytes in {} frames -> {} ({:.2} s)",
        data.len(),
        frames,
        o.output,
        samples.len() as f64 / SAMPLE_RATE as f64
    );
    Ok(())
}

fn cmd_sim_tx(o: &Opts) -> Result<(), String> {
    let snr = o.snr.ok_or("missing --snr")?;
    let data = fs::read(&o.input).map_err(|e| format!("cannot read {}: {e}", o.input))?;
    let (clean, frames) = encode_stream(&data)?;
    let noisy = kahf_sim::awgn(&clean, snr, SIM_SEED);
    write_wav(&o.output, &noisy)?;
    eprintln!("sim-tx: {frames} frames at {snr} dB SNR -> {}", o.output);
    Ok(())
}

fn cmd_rx(o: &Opts) -> Result<(), String> {
    let samples = read_wav(&o.input)?;
    let (bytes, frames) = decode_stream(&samples);
    fs::write(&o.output, &bytes).map_err(|e| format!("cannot write {}: {e}", o.output))?;
    eprintln!(
        "rx: {frames} frames passed CRC, {} bytes -> {}",
        bytes.len(),
        o.output
    );
    Ok(())
}

struct BlerOpts {
    from: f32,
    to: f32,
    step: f32,
    frames: usize,
}

fn parse_bler(args: &[String]) -> Result<BlerOpts, String> {
    let (mut from, mut to, mut step, mut frames) = (None, None, None, None);
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let val = args.get(i + 1).ok_or_else(|| format!("{flag} needs a value"))?;
        match flag {
            "--from" => {
                from = Some(
                    val.parse::<f32>()
                        .map_err(|_| format!("bad --from value: {val}"))?,
                )
            }
            "--to" => to = Some(val.parse::<f32>().map_err(|_| format!("bad --to value: {val}"))?),
            "--step" => {
                step = Some(
                    val.parse::<f32>()
                        .map_err(|_| format!("bad --step value: {val}"))?,
                )
            }
            "--frames" => {
                frames = Some(
                    val.parse::<usize>()
                        .map_err(|_| format!("bad --frames value: {val}"))?,
                )
            }
            s => return Err(format!("unknown option {s}")),
        }
        i += 2;
    }
    let o = BlerOpts {
        from: from.ok_or("missing --from")?,
        to: to.ok_or("missing --to")?,
        step: step.ok_or("missing --step")?,
        frames: frames.ok_or("missing --frames")?,
    };
    if o.step.is_nan() || o.step <= 0.0 || !o.step.is_finite() {
        return Err("--step must be a positive number".into());
    }
    if o.to < o.from {
        return Err("--to must be >= --from".into());
    }
    if o.frames == 0 {
        return Err("--frames must be at least 1".into());
    }
    Ok(o)
}

fn cmd_bler(o: &BlerOpts) -> Result<(), String> {
    // Samples per frame, for the Eb/N0 column (K info bits per frame).
    let frame_samples = phy::modulate_frame(&vec![0u8; ldpc::N], MOD).len();
    println!(
        "{:>8} {:>9} {:>12} {:>10} {:>10}",
        "SNR_dB", "EbN0_dB", "FER", "errors", "wall_s"
    );
    // Integer step count avoids float drift in the loop bound.
    let steps = ((o.to - o.from) / o.step + 1e-4).floor() as usize;
    for k in 0..=steps {
        let snr = o.from + k as f32 * o.step;
        let t0 = Instant::now();
        let (errors, frames) = kahf_sim::run_point(snr, o.frames, SIM_SEED);
        let wall = t0.elapsed().as_secs_f64();
        let eb = kahf_sim::eb_n0_db(snr, frame_samples, ldpc::K);
        println!(
            "{:>8.2} {:>9.2} {:>12.6} {:>10} {:>10.2}",
            snr,
            eb,
            errors as f64 / frames as f64,
            errors,
            wall
        );
    }
    Ok(())
}

/// Print a list section: one entry per line, or "none found" when empty.
fn print_section(title: &str, items: &[String]) {
    println!("{title}");
    if items.is_empty() {
        println!("  none found");
    } else {
        for item in items {
            println!("  {item}");
        }
    }
    println!();
}

/// Report the devices and libraries this machine can use. Always succeeds.
fn cmd_doctor() {
    print_section("Audio devices (input or output)", &kahf_audio::list_devices());
    print_section("HackRF devices", &kahf_sdr::hackrf::list_devices());
    print_section("RTL-SDR devices", &kahf_sdr::rtlsdr::list_devices());
    let (hackrf, rtlsdr) = kahf_sdr::libraries_present();
    println!("SDR libraries");
    println!("  libhackrf: {}", if hackrf { "found" } else { "not installed" });
    println!("  librtlsdr: {}", if rtlsdr { "found" } else { "not installed" });
}

/// `kahf audio-test [--device NAME]`: read the input for 3 s, print its level once a second.
fn cmd_audio_test(args: &[String]) -> Result<(), Fail> {
    let mut device: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--device" => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| Fail::Usage("--device needs a value".into()))?;
                device = Some(v.clone());
            }
            s => return Err(Fail::Usage(format!("unknown option {s}"))),
        }
        i += 1;
    }
    let mut input = match kahf_audio::AudioIn::open(device.as_deref()) {
        Ok(a) => a,
        Err(e) => {
            println!("No audio input device found.");
            eprintln!("{e}");
            process::exit(1);
        }
    };
    println!("Input device: {}", input.name());
    let mut tmp = vec![0.0f32; kahf_audio::RATE as usize];
    for _ in 0..3 {
        let mut window: Vec<f32> = Vec::new();
        let t0 = Instant::now();
        while t0.elapsed() < std::time::Duration::from_secs(1) {
            let n = input.read(&mut tmp);
            window.extend_from_slice(&tmp[..n]);
            if let Some(e) = input.error() {
                return Err(Fail::Run(e));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        println!("Input level: {:.1} dBFS", kahf_audio::level_db(&window));
    }
    Ok(())
}

struct E2eOpts {
    profile: Profile,
    profile_name: String,
    snr: f32,
    bytes: usize,
    seed: u64,
}

fn parse_profile(name: &str) -> Result<Profile, String> {
    match name {
        "awgn" => Ok(Profile::Awgn),
        "two-path" => Ok(Profile::TwoPath),
        "freq-offset" => Ok(Profile::FreqOffset),
        "clock-offset" => Ok(Profile::ClockOffset),
        "fade" => Ok(Profile::Fade),
        _ => Err(format!(
            "unknown profile {name} (awgn, two-path, freq-offset, clock-offset, fade)"
        )),
    }
}

fn parse_e2e(args: &[String]) -> Result<E2eOpts, String> {
    let (mut profile, mut snr, mut bytes, mut seed) = (None, None, None, None);
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let val = args.get(i + 1).ok_or_else(|| format!("{flag} needs a value"))?;
        match flag {
            "--profile" => profile = Some(val.clone()),
            "--snr" => {
                snr = Some(
                    val.parse::<f32>()
                        .map_err(|_| format!("bad --snr value: {val}"))?,
                )
            }
            "--bytes" => {
                bytes = Some(
                    val.parse::<usize>()
                        .map_err(|_| format!("bad --bytes value: {val}"))?,
                )
            }
            "--seed" => {
                seed = Some(
                    val.parse::<u64>()
                        .map_err(|_| format!("bad --seed value: {val}"))?,
                )
            }
            s => return Err(format!("unknown option {s}")),
        }
        i += 2;
    }
    let profile_name = profile.ok_or("missing --profile")?;
    let profile_v = parse_profile(&profile_name)?;
    let snr = snr.ok_or("missing --snr")?;
    let bytes = bytes.ok_or("missing --bytes")?;
    if bytes == 0 {
        return Err("--bytes must be at least 1".into());
    }
    // Default seed: wall-clock nanoseconds. It is printed, so a failing run can be repeated with --seed.
    let seed = seed.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
    });
    Ok(E2eOpts {
        profile: profile_v,
        profile_name,
        snr,
        bytes,
        seed,
    })
}

/// Deterministic pseudo-random bytes (xorshift64) for the test file.
fn random_bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Index of the first byte that differs, or the shorter length if one is a prefix of the other.
fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or_else(|| (a.len() != b.len()).then(|| a.len().min(b.len())))
}

/// Run one end-to-end case. Returns true on PASS. Prints one result line either way.
fn cmd_e2e(o: &E2eOpts) -> Result<bool, String> {
    let data = random_bytes(o.bytes, o.seed);
    let (clean, tx_frames) = encode_stream(&data)?;
    let impaired = kahf_sim::apply(&clean, o.profile, o.snr, SIM_SEED);

    // The WAV round trip is part of the chain, so it goes through a real file.
    let wav = env::temp_dir().join(format!("kahf-e2e-{}-{}.wav", process::id(), o.seed));
    let wav_s = wav.to_string_lossy().into_owned();
    write_wav(&wav_s, &impaired)?;
    let rx_samples = read_wav(&wav_s);
    let _ = fs::remove_file(&wav);
    let rx_samples = rx_samples?;

    let (rx_bytes, rx_frames) = decode_stream(&rx_samples);
    let diff = first_diff(&data, &rx_bytes);
    let verdict = if diff.is_none() { "PASS" } else { "FAIL" };
    let diff_s = match diff {
        Some(off) => format!("{off}"),
        None => "none".to_string(),
    };
    println!(
        "{verdict} profile={} snr={} dB bytes={} frames={}/{} rx_bytes={} first_diff={} seed={}",
        o.profile_name,
        o.snr,
        o.bytes,
        rx_frames,
        tx_frames,
        rx_bytes.len(),
        diff_s,
        o.seed
    );
    Ok(diff.is_none())
}

fn run(args: &[String]) -> Result<(), Fail> {
    let (cmd, rest) = args
        .split_first()
        .ok_or_else(|| Fail::Usage("missing command".into()))?;
    match cmd.as_str() {
        "tx" => {
            let o = parse_opts(rest, false).map_err(Fail::Usage)?;
            cmd_tx(&o).map_err(Fail::Run)
        }
        "rx" => {
            let o = parse_opts(rest, false).map_err(Fail::Usage)?;
            cmd_rx(&o).map_err(Fail::Run)
        }
        "sim-tx" => {
            let o = parse_opts(rest, true).map_err(Fail::Usage)?;
            cmd_sim_tx(&o).map_err(Fail::Run)
        }
        "bler" => {
            let o = parse_bler(rest).map_err(Fail::Usage)?;
            cmd_bler(&o).map_err(Fail::Run)
        }
        "audio-test" => cmd_audio_test(rest),
        "e2e" => {
            let o = parse_e2e(rest).map_err(Fail::Usage)?;
            match cmd_e2e(&o).map_err(Fail::Run)? {
                true => Ok(()),
                false => process::exit(1),
            }
        }
        "doctor" => {
            if !rest.is_empty() {
                return Err(Fail::Usage("doctor takes no arguments".into()));
            }
            cmd_doctor();
            Ok(())
        }
        other => Err(Fail::Usage(format!("unknown command {other}"))),
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => {}
        Err(Fail::Usage(m)) => {
            eprintln!("error: {m}\n{USAGE}");
            process::exit(2);
        }
        Err(Fail::Run(m)) => {
            eprintln!("error: {m}");
            process::exit(1);
        }
    }
}

//! `kahf tx <in.bin> -o out.wav`, `kahf rx <in.wav> -o out.bin`,
//! `kahf sim-tx <in.bin> -o out.wav --snr <dB>`,
//! `kahf bler --from <dB> --to <dB> --step <dB> --frames <n>`.
use kahf_core::frame;
use kahf_core::ldpc;
use kahf_core::phy::{self, Modulation};
use std::env;
use std::fs;
use std::process;
use std::time::Instant;

const SAMPLE_RATE: u32 = 8000;
const SILENCE_SAMPLES: usize = SAMPLE_RATE as usize / 5; // 0.2 s
const CHUNK_BYTES: usize = 36;
const RX_SCAN_STEP: usize = SAMPLE_RATE as usize; // 1 s
const MAX_LDPC_ITER: usize = 50;
const SIM_SEED: u64 = 0x4B41_4846; // "KAHF"
const MOD: Modulation = Modulation::Qpsk;

const USAGE: &str = "usage:
  kahf tx     <input_file> -o <out.wav>
  kahf rx     <in.wav> -o <out_file>
  kahf sim-tx <input_file> -o <out.wav> --snr <dB>
  kahf bler   --from <dB> --to <dB> --step <dB> --frames <n>
              (Monte-Carlo frame error rate, QPSK + LDPC, awgn SNR per real sample)";

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
            return Err(format!("build_payload gave {} bits, expected {}", bits.len(), ldpc::K));
        }
        let coded = ldpc::encode(&bits);
        if coded.len() != ldpc::N {
            return Err(format!("ldpc::encode gave {} bits, expected {}", coded.len(), ldpc::N));
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

/// Scan the stream for frames. After a success, skip the frame and the silence
/// after it; on failure, advance by 1 s and retry. Returns (bytes, frames passing CRC).
fn decode_stream(samples: &[f32]) -> (Vec<u8>, usize) {
    let frame_len = phy::modulate_frame(&vec![0u8; ldpc::N], MOD).len();
    let stride = frame_len + SILENCE_SAMPLES;
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
                pos += stride;
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
    eprintln!("rx: {frames} frames passed CRC, {} bytes -> {}", bytes.len(), o.output);
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
            "--from" => from = Some(val.parse::<f32>().map_err(|_| format!("bad --from value: {val}"))?),
            "--to" => to = Some(val.parse::<f32>().map_err(|_| format!("bad --to value: {val}"))?),
            "--step" => step = Some(val.parse::<f32>().map_err(|_| format!("bad --step value: {val}"))?),
            "--frames" => frames = Some(val.parse::<usize>().map_err(|_| format!("bad --frames value: {val}"))?),
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
    if !(o.step > 0.0) || !o.step.is_finite() {
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
    println!("{:>8} {:>9} {:>12} {:>10} {:>10}", "SNR_dB", "EbN0_dB", "FER", "errors", "wall_s");
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

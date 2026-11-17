//! Release-mode timing report for the core encode and decode paths.
//!
//! Ignored by default. Run with:
//!   cargo test --release -p kahf-core --test perf_report -- --ignored --nocapture
//!
//! Each timed run makes BATCH calls and divides the elapsed time by BATCH. The
//! figure in the table is the median of TIMED_RUNS such runs, after WARMUP_RUNS
//! untimed runs. Inputs come from a fixed seed, so repeated runs use the same data.
//! Every operation is checked for correct output once, before timing starts.

use kahf_core::frame;
use kahf_core::ldpc::{self, K, N};
use kahf_core::phy::{self, Modulation};
use kahf_core::session::{self, Message};
use std::hint::black_box;
use std::time::Instant;

const WARMUP_RUNS: usize = 5;
const TIMED_RUNS: usize = 50;
const BATCH: usize = 20;
const SEED: u64 = 0x5EED_2026_0001_CAFE;
const MAX_ITER: usize = 50;
const FRAME_BYTES: usize = 36;
const MESSAGE_BYTES: usize = 1024;

/// xorshift64. Seeded with a nonzero value.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        assert_ne!(seed, 0);
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn bit(&mut self) -> u8 {
        (self.next_u64() >> 63) as u8
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() >> 56) as u8
    }

    /// Uniform in (0, 1).
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal, Box-Muller.
    fn gaussian(&mut self) -> f64 {
        let u1 = self.uniform();
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// Rate 1/2 BPSK over AWGN at the given Eb/N0 in dB. Returns LLRs (positive = bit 0).
fn awgn_llr(rng: &mut Rng, coded: &[u8], ebn0_db: f64) -> Vec<f32> {
    let ebn0 = 10f64.powf(ebn0_db / 10.0);
    // Rate 1/2 with unit symbol energy: sigma^2 = 1 / (2 * R * Eb/N0) = 1 / Eb/N0.
    let sigma2 = 1.0 / ebn0;
    let sigma = sigma2.sqrt();
    coded
        .iter()
        .map(|&b| {
            let x = 1.0 - 2.0 * b as f64;
            let y = x + sigma * rng.gaussian();
            (2.0 * y / sigma2) as f32
        })
        .collect()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Median time per call, in microseconds.
fn bench<F: FnMut()>(mut f: F) -> f64 {
    for _ in 0..WARMUP_RUNS {
        for _ in 0..BATCH {
            f();
        }
    }
    let mut per_call_us = Vec::with_capacity(TIMED_RUNS);
    for _ in 0..TIMED_RUNS {
        let start = Instant::now();
        for _ in 0..BATCH {
            f();
        }
        let elapsed_ns = start.elapsed().as_nanos() as f64;
        per_call_us.push(elapsed_ns / BATCH as f64 / 1000.0);
    }
    median(per_call_us)
}

#[test]
#[ignore]
fn perf_report() {
    let mut rng = Rng::new(SEED);
    let mut rows: Vec<(String, String, f64)> = Vec::new();

    // LDPC. Fixed info word and fixed noise realisations.
    let info: Vec<u8> = (0..K).map(|_| rng.bit()).collect();
    let coded = ldpc::encode(&info);
    assert_eq!(coded.len(), N);
    let llr_3db = awgn_llr(&mut rng, &coded, 3.0);
    let llr_0db = awgn_llr(&mut rng, &coded, 0.0);

    let t = bench(|| {
        black_box(ldpc::encode(black_box(&info)));
    });
    rows.push((
        "LDPC encode".into(),
        format!("K={K} info bits to N={N} coded bits"),
        t,
    ));

    // The 3 dB frame should decode. The 0 dB frame is timed whether or not it does.
    let decoded = ldpc::decode(&llr_3db, MAX_ITER).expect("3 dB frame should decode");
    assert_eq!(decoded, info, "3 dB frame decoded to the wrong bits");

    let t = bench(|| {
        black_box(ldpc::decode(black_box(&llr_3db), MAX_ITER));
    });
    rows.push((
        "LDPC decode".into(),
        format!("Eb/N0 3 dB, max {MAX_ITER} iterations"),
        t,
    ));

    let t = bench(|| {
        black_box(ldpc::decode(black_box(&llr_0db), MAX_ITER));
    });
    rows.push((
        "LDPC decode".into(),
        format!("Eb/N0 0 dB, max {MAX_ITER} iterations"),
        t,
    ));

    // PHY. One frame of N coded bits per modulation, clean channel.
    let coded_phy: Vec<u8> = (0..N).map(|_| rng.bit()).collect();
    for (name, m) in [
        ("QPSK", Modulation::Qpsk),
        ("16QAM", Modulation::Qam16),
        ("64QAM", Modulation::Qam64),
    ] {
        let samples = phy::modulate_frame(&coded_phy, m);
        let llr = phy::demodulate_frame(&samples, m, N).expect("clean frame should be found");
        assert_eq!(llr.len(), N, "{name}: wrong LLR count");
        for (i, (&b, &l)) in coded_phy.iter().zip(llr.iter()).enumerate() {
            assert_eq!((l < 0.0) as u8, b, "{name}: bit {i} demodulated wrong");
        }

        let t = bench(|| {
            black_box(phy::modulate_frame(black_box(&coded_phy), m));
        });
        rows.push((
            format!("PHY modulate {name}"),
            format!("{N} coded bits, one frame"),
            t,
        ));

        let t = bench(|| {
            black_box(phy::demodulate_frame(black_box(&samples), m, N));
        });
        rows.push((
            format!("PHY demodulate {name}"),
            format!("{N} coded bits, one frame"),
            t,
        ));
    }

    // Frame payload: one frame of up to 36 bytes.
    let payload: Vec<u8> = (0..FRAME_BYTES).map(|_| rng.byte()).collect();
    let bits = frame::build_payload(&payload);
    assert_eq!(frame::parse_payload(&bits).as_deref(), Some(payload.as_slice()));

    let t = bench(|| {
        black_box(frame::build_payload(black_box(&payload)));
    });
    rows.push(("frame::build_payload".into(), format!("{FRAME_BYTES} bytes"), t));

    let t = bench(|| {
        black_box(frame::parse_payload(black_box(&bits)));
    });
    rows.push(("frame::parse_payload".into(), format!("{FRAME_BYTES} bytes"), t));

    // Session: one 1 KB file message, split into link chunks.
    let data: Vec<u8> = (0..MESSAGE_BYTES).map(|_| rng.byte()).collect();
    let msg = Message::File {
        from: "ALPHA".into(),
        to: "BRAVO".into(),
        name: "blob.bin".into(),
        data,
    };
    let chunks = session::encode(&msg);
    assert_eq!(session::decode(&chunks).as_ref(), Some(&msg));

    let t = bench(|| {
        black_box(session::encode(black_box(&msg)));
    });
    rows.push((
        "session::encode".into(),
        format!("{MESSAGE_BYTES} byte file message"),
        t,
    ));

    let t = bench(|| {
        black_box(session::decode(black_box(&chunks)));
    });
    rows.push((
        "session::decode".into(),
        format!("{MESSAGE_BYTES} byte file message"),
        t,
    ));

    println!();
    println!("| Operation | Setting | Median per call (µs) |");
    println!("|---|---|---:|");
    for (op, setting, us) in &rows {
        println!("| {op} | {setting} | {us:.2} |");
    }
    println!();
}

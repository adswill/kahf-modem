# Performance

Timings for the core encode and decode paths of `kahf-core`, measured in release mode, and frame error rate from the `kahf-cli` BLER sweep.

- Machine: Apple M2 Pro
- macOS: 27.2
- Date: 2026-11-17
- Build: `cargo build --release` (default release profile)

## Commands

```
cargo test --release -p kahf-core --test perf_report -- --ignored --nocapture
cargo build --release -p kahf-cli
./target/release/kahf-cli bler --from 0 --to 6 --step 2 --frames 500
```

## Core timings

Median time per call over 50 timed runs, after 5 warm-up runs. Each run makes 20 calls and divides the elapsed time by 20. One run of the test on the machine above.

| Operation | Setting | Median per call (µs) |
|---|---|---:|
| LDPC encode | K=324 info bits to N=648 coded bits | 0.74 |
| LDPC decode | Eb/N0 3 dB, max 50 iterations | 24.32 |
| LDPC decode | Eb/N0 0 dB, max 50 iterations | 710.34 |
| PHY modulate QPSK | 648 coded bits, one frame | 13.82 |
| PHY demodulate QPSK | 648 coded bits, one frame | 375.10 |
| PHY modulate 16QAM | 648 coded bits, one frame | 10.70 |
| PHY demodulate 16QAM | 648 coded bits, one frame | 389.40 |
| PHY modulate 64QAM | 648 coded bits, one frame | 6.27 |
| PHY demodulate 64QAM | 648 coded bits, one frame | 394.51 |
| frame::build_payload | 36 bytes | 0.54 |
| frame::parse_payload | 36 bytes | 0.61 |
| session::encode | 1024 byte file message | 14.79 |
| session::decode | 1024 byte file message | 3.34 |

## Frame error rate (BLER sweep)

`kahf-cli bler --from 0 --to 6 --step 2 --frames 500`. Columns as printed by the tool.

| SNR (dB) | Eb/N0 (dB) | FER | Errors | Wall time (s) |
|---:|---:|---:|---:|---:|
| 0.00 | 6.48 | 0.072000 | 36 | 0.05 |
| 2.00 | 8.48 | 0.000000 | 0 | 0.04 |
| 4.00 | 10.48 | 0.000000 | 0 | 0.04 |
| 6.00 | 12.48 | 0.000000 | 0 | 0.04 |

Each row is a single run of 500 frames at the stated SNR.

## Notes on the measurements

- LDPC inputs are one fixed information word, encoded, then passed through a BPSK AWGN channel at the stated Eb/N0 with a fixed seed. Eb/N0 is the energy per information bit over noise power spectral density, for rate 1/2.
- The 3 dB frame is checked to decode to the original bits before timing starts. The 0 dB frame is timed whether or not it decodes.
- PHY timings use a clean channel (no added noise). Demodulation output is checked against the transmitted bits before timing starts.
- Session timings use a 1 KB file message with random contents, so compression has little to remove.
- All inputs come from a fixed seed. Results depend on the machine, the power state and the other load on it. Only one run of the timing test is recorded here, so treat the figures as approximate.
- The BLER table is one sweep of 500 frames per point. At 0 dB SNR the 36 errors give a FER estimate with wide uncertainty; the points at 2 dB and above show no frame errors in this sweep and do not establish an error floor.

## Open items

- Two-path echo performance: not measured here. This figure stays open while the modem is still being changed, so no number is recorded in this file.

## How to reproduce

Run the commands above on a quiet machine with the power connected. The perf test is marked `#[ignore]`, so a plain `cargo test` skips it. The source is `crates/kahf-core/tests/perf_report.rs`. The BLER sweep is run by the `bler` subcommand of `crates/kahf-cli`.

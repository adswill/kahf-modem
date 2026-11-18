# KAHF Modem

KAHF (pronounced "kaf", rhymes with "calf") is an HF data modem written in Rust. It has so far been built and run only on macOS (Linux and Windows are not yet tested) and is built for amateur radio data links: station connections, messages, mail and file transfer.

Like KAHF? You can support its development on Ko-fi: <https://ko-fi.com/adswill>

## Status

Early development. KAHF is not yet on air. The modem, frame, link and session layers, the logbook and the stress tests are implemented and tested. The sound-card and SDR adapters and the desktop interface are in progress. Measured numbers are in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

## What works now

- The modem over audio: encode a message to a WAV file and decode it back, including through the channel simulator.
- Messages, mail and file transfer over the link and session layers.
- A contact logbook with CSV storage and ADIF 3 export.
- Unit and simulation tests for the station server.

## Build and test

Requires a recent stable Rust toolchain.

```bash
cargo build --workspace
cargo test --workspace
```

The `kahf-sdr` crate loads the HackRF and RTL-SDR libraries at run time; they are not needed to build. To use those radios on macOS, install them with Homebrew:

```bash
brew install hackrf librtlsdr
```

Example: encode a file to audio and decode it again with the command-line tool.

```bash
cargo run -p kahf-cli -- tx message.bin -o message.wav
cargo run -p kahf-cli -- rx message.wav -o decoded.bin
```

## Crates

| Crate | Purpose |
|---|---|
| `kahf-core` | LDPC code, OFDM-style modem, frame, link and session layers, I/O traits |
| `kahf-sim` | Channel simulator and error-rate measurement |
| `kahf-log` | Contact logbook: CSV storage and ADIF export |
| `kahf-prop` | First-pass HF propagation estimator (estimate only) |
| `kahf-audio` | Sound-card input and output |
| `kahf-sdr` | SDR adapters (HackRF, RTL-SDR) |
| `kahf-daemon` | Engine that connects the modem to audio, SDR and rig; the API used by the interfaces |
| `kahf-cli` | Command-line tool: `tx`, `rx`, `sim-tx`, `bler`, `e2e` |
| `kahf-gui` | Desktop interface |
| `kahf-ui` | Alternative desktop interface (shows sample data; not for real use) |

## Licence

Copyright (c) 2026 adswill. All rights reserved. This software is source-available. Licence terms will be published with the first release. See [LICENSE](LICENSE).

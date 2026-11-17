# KAHF Modem

KAHF is an HF data modem written in Rust. It runs on macOS, Linux and Windows and is built for amateur radio data links: station connections, messages, mail, and file transfer.

## Status

Early development. The coding, modulation, framing, link and session layers are implemented and tested. The sound-card, SDR and rig interfaces and the desktop interface are in progress. The modem is not yet ready for on-air use.

## Crates

| Crate | Purpose |
|---|---|
| `kahf-core` | LDPC code, OFDM modem, frame, link and session layers, hardware traits |
| `kahf-sim` | Channel simulator and error-rate measurement |
| `kahf-cli` | Command-line tool: `tx`, `rx`, `sim-tx`, `bler` |
| `kahf-audio` | Sound-card input and output |
| `kahf-sdr` | SDR adapters (HackRF, RTL-SDR) |
| `kahf-daemon` | Engine that connects the modem to audio, SDR and rig |
| `kahf-gui` | Desktop interface |

## Build

```bash
cargo build --workspace
cargo test --workspace
```

The `kahf-sdr` crate links the system libraries for HackRF and RTL-SDR. On macOS, install them with Homebrew:

```bash
brew install hackrf librtlsdr
```

## Licence

See [LICENSE](LICENSE).

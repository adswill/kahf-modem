# KAHF Modem

KAHF (pronounced "kaf", rhymes with "calf") is an HF data modem written in Rust. It has so far been built and run only on macOS (Linux and Windows are not yet tested) and is built for amateur radio data links: station connections, messages, mail and file transfer.

Like KAHF? You can support its development on Ko-fi: <https://ko-fi.com/adswill>

## Status

Early development. KAHF is not yet on air: nothing has been tried on a real radio. The modem, frame, link and session layers, the logbook, the relay layer and the stress tests are implemented and tested with simulated stations. The sound-card and SDR adapters are in progress. The app refuses to transmit until the 17 m band table is verified. Measured numbers are in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

## What works now

- The modem over audio: encode a message to a WAV file and decode it back, including through the channel simulator.
- Messages, mail and file transfer over the link and session layers.
- A contact logbook with CSV storage and ADIF 3 export.
- Server stress tests for the station server.
- Radio relay on 17 m: store-and-forward over one or more relays, with a propagation-aware route planner and a relay referee. It is tested with simulated stations, not yet on air.
- 17 m lanes: KAHF uses 12 narrow lanes of 480 Hz at 700 Hz pitch between 18.111 and 18.120 MHz, with lane sensing and a transmit gate. This is a design checked in simulation only.
- A native desktop app (`app/`, built with Alloy UI) with Radio, Chats, Mail, Stations, Relays and Logbook screens. See [app/README.md](app/README.md).

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
| `kahf-core` | LDPC code, the KAMI modem (plain and 480 Hz lane mode), 17 m band table and lanes, frame, bundle, link and session layers, relay referee, route planner |
| `kahf-sim` | Channel simulator and error-rate measurement |
| `kahf-log` | Contact logbook: CSV storage and ADIF export |
| `kahf-prop` | First-pass HF propagation estimator (estimate only) |
| `kahf-audio` | Sound-card input and output |
| `kahf-sdr` | SDR adapters (HackRF, RTL-SDR) |
| `kahf-daemon` | The engine: connects the modem to audio, SDR and rig; the API used by the interfaces |
| `kahf-app-core` | Everything the desktop app does besides drawing: engine, radio control, voice notes, logbook, relay status |
| `kahf-net` | Client for the kahf.dev relay server: relay registration and slot schedules |
| `kahf-node` | Headless station and relay node for a Raspberry Pi or any always-on computer |
| `kahf-winlink` | Winlink bridge: B2F protocol, LZHUF, message format and mail mapping |
| `kahf-cli` | Command-line tool: `tx`, `rx`, `sim-tx`, `bler`, `e2e` |
| `kahf-gui` | Older desktop interface, no longer built (replaced by the Alloy UI app) |
| `kahf-ui` | Older egui interface, no longer built (replaced by the Alloy UI app) |

The native desktop app lives in `app/`, which is its own Cargo workspace.

## Licence

Copyright (c) 2026 Kristijonas Adomaitis (adswill). KAHF is free software under the GNU General Public License, version 3 or (at your option) any later version. You may use, study, change and share it, and if you share a changed version, or build it into another program, that program must be offered under the same licence with its source. If you copy any part of KAHF, or build a changed version, you must keep the copyright notices and credit the project in your about screen or docs, for example "Based on KAHF by adswill (https://kahf.dev)". This is an additional term in [LICENSE](LICENSE).

A request, not a legal requirement: if KAHF only inspired your project, or you used it as a reference without copying code, please mention it too. Copyright cannot cover ideas, but it would mean a lot.

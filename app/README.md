# KAHF desktop app

The window for KAHF, drawn natively in Rust with [Alloy UI](https://github.com/adswill/alloy-ui):
no web view, no JavaScript. It runs on macOS, Linux and Windows.

Screens: Radio, Chats, Mail, Stations (with a map that works offline), Relays, Logbook, and
Settings, Help and a short tour. It uses the 17 m band only: 12 narrow lanes of 480 Hz at 700 Hz pitch between 18.111 and 18.120 MHz. Nothing has been tried on a real radio yet, and the app refuses to transmit until the 17 m band table is verified.

## Layout

- `../crates/kahf-app-core`: the engine glue without any window. It starts the modem, talks to the
  radio, keeps the logbook and settings, and reports events as JSON.
- `src/model.rs`: the state of the window as plain data, with unit tests.
- `src/app.rs`: the model plus the form state, and the bridge to the engine. Work that takes time
  runs on a thread and comes back through `pump()`, which the window calls every 40 ms.
- `src/ui/`: one file per screen.
- `assets/land.txt`: coastline outlines (Natural Earth, public domain) for the map.

This crate is its own Cargo workspace, so the normal KAHF build does not need the window libraries.

## Build and test

```sh
cd app
cargo run --release     # the app
cargo test              # includes headless drawing of every screen
KAHF_SHOTS=/tmp/shots cargo test   # also writes a PNG of each screen
```

On Linux you need the usual window and sound development packages, for example
`libasound2-dev libudev-dev libxkbcommon-dev libwayland-dev libxcb1-dev`.

Settings and messages are stored where the earlier app kept them (`dev.kahf.app` in the system's
data folder), so an existing station keeps its settings. Set `KAHF_DATA_DIR` to use another folder.

## Bundle

`alloy bundle` builds a `.app`, `.msi` or AppImage from `[package.metadata.alloy]` in `Cargo.toml`.
On macOS the bundle needs a microphone permission entry (`NSMicrophoneUsageDescription`) for voice
notes and the audio input entitlement; see `docs/BUILD.md`.

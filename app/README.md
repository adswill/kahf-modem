# KAHF app

The desktop app: a window built with Svelte and TypeScript (`src/`), on top of the Rust engine
(`src-tauri/`, using Tauri 2). This is the interface new users should see. It runs on the same engine
(`kahf-daemon`) as `kahf-ui` (iced) and `kahf-gui` (egui), which stay for now as the older windows.

This engine works on 17 m in three channels (18.111 to 18.120 MHz) and relays through kahf.dev. It has
no speed table, no transmit-level control and no channel-use report, so the window does not show them.

## Run it in a browser, no radio needed

```bash
cd app
npm install
npm run dev        # opens http://127.0.0.1:5173 with a built-in demo radio
```

The demo engine (`src/lib/mock.ts`) pretends to be a radio: stations appear, calls connect or
fail, messages go out in steps. Every screen can be tried and checked this way.

## Run the real app

```bash
cd app
npm install && npm run build           # builds the window into dist/
cd src-tauri
cargo run                              # the window with the real engine
KAHF_DATA_DIR=/tmp/kahf-test cargo run # start fresh, as a brand-new user, leaving your settings alone
```

## Checks

```bash
npm run check          # types
npm test               # the logic behind the screens
cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings
```

## Layout

| Path | What it holds |
|---|---|
| `src/pages/` | Welcome and Setup (first run) |
| `src/views/` | The main screens: Chats, Mail, Stations, Radio, Relay, Logbook |
| `src/components/` | Top strip, nav, settings sheet, help panel, tour, map, radio and sound pickers, spectrum |
| `src/lib/state.svelte.ts` | Everything the window shows, and the only place that talks to the engine |
| `src/lib/types.ts` | The shapes shared with the Rust side |
| `src-tauri/src/lib.rs` | The commands the window calls, and the event loop |
| `src-tauri/src/convert.rs` | Engine events to the JSON the window understands |
| `src-tauri/src/voice.rs` | Voice notes: microphone, Codec2 at 1200 bit/s, `.kvm` files (Codec2 is LGPL, patent-free) |
| `src-tauri/src/directory.rs` | Relays this station knows: server list, saved copy, typed-in callsigns. Never required |
| `src-tauri/src/rig.rs` | Radio control through Hamlib |
| `src-tauri/src/live.rs` | The address book, the logbook (ADIF export), relay numbers and locators, read from the engine and the data folder |
| `src-tauri/src/diag.rs` | The Test buttons for the rig and the sound devices. They never transmit |
| `src-tauri/src/update.rs` | The update check, run only when the person presses the button |

## Design rules

- Written for someone who has never heard of KAHF: plain words, one idea per screen, no jargon
  without an explanation next to it.
- Automatic by default: the app connects, picks the speed and retries by itself. Manual control
  lives under "More options" or in Settings.
- Privacy first: location sharing is off until the person turns it on.

# KAHF app

The desktop app: a window built with Svelte and TypeScript (`src/`), on top of the Rust engine
(`src-tauri/`, using Tauri 2). This is the interface new users should see. The older `kahf-ui`
(iced) and `kahf-gui` (egui) programs are kept for reference and tests only.

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
| `src/pages/` | One file per screen: Welcome, Setup, Home, Messages, Network, Radio, Settings, Help |
| `src/components/` | Spectrum and waterfall, the guided tour, sidebar, toasts, the location editor |
| `src/lib/state.svelte.ts` | Everything the window shows, and the only place that talks to the engine |
| `src/lib/types.ts` | The shapes shared with the Rust side |
| `src-tauri/src/lib.rs` | The commands the window calls, and the event loop |
| `src-tauri/src/convert.rs` | Engine events to the JSON the window understands |

## Design rules

- Written for someone who has never heard of KAHF: plain words, one idea per screen, no jargon
  without an explanation next to it.
- Automatic by default: the app connects, picks the speed and retries by itself. Manual control
  lives under "More options" or in Settings.
- Privacy first: location sharing is off until the person turns it on.

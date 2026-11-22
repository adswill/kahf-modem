# Handover B (2026-11-18, end of a long session): read this whole file first

Work only inside this KAHF worktree (`/Users/kristijonasadomaitis/kahf-work/wt-rig`, branch
`app-rig-relay`). Never touch any dvb-t2 or OnAir folder. This file adds to
`docs/HANDOVER-2026-11-18.md` (the owner rules there still hold; they are repeated below).

Then read `docs/MESH-SPEC.md` (the design the owner handed over to you) and `docs/NETWORK-STATUS.md`.

## 1. Owner rules (do not break)

1. Only edit KAHF folders.
2. **Never run tests or audio while the owner's BlackHole instances run.** Check first, every time:
   `pgrep -fl 'kahf-alpha|kahf-bravo'`. Empty output means safe. Daemon tests use in-memory pipes and are safe when
   those are stopped. Do not start `/tmp/kahf-ab/start.sh` unless the owner asks.
3. Commit with the repo's configured `adswill` noreply identity (already set: `git config user.name` is `adswill`,
   email `326940294+adswill@users.noreply.github.com`). End every commit message with
   `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Push **only** branch `app-rig-relay`. Merges into
   `release-0.1` only on request.
4. Server deploys (`cd server && npx wrangler deploy`, needs `dangerouslyDisableSandbox`) only when the owner asks.
   This Mac's web filter blocks kahf.dev; curl needs `--resolve kahf.dev:443:104.21.3.235 -k`.
5. Replies to the owner: plain, short, honest about what is **not** verified. No jargon. The owner dislikes long waits:
   do not run a 2-minute test, then another, then another. Get tests fast first (task 1 below).
6. The owner interrupts when something is slow or unwanted. Take it as steering, not as an error.
7. The owner said **no subagents** earlier in this session. Do not spawn any unless asked.

## 2. What the owner wants now (in order)

1. A relay mesh that is autonomous, fast and does not clash on the air. The design is in `docs/MESH-SPEC.md`
   (17 m only, about 12 narrow lanes, server-planned slots, no stored messages, both-offline paging).
2. The app, tests, scenarios, server (Cloudflare) all finished and reliable under real conditions: both sides sending at
   once, fast typing, random timing, a link closing mid-conversation, a station hearing its own transmissions
   (two app instances over BlackHole).
3. The website redesigned, "way way better" (paused; see section 7).
4. The owner said they will come back to "a fully done app/mesh/system". So: build as much of `MESH-SPEC.md`
   section 14 as you can, in order, with each step's acceptance test passing.

## 3. State of the working tree (nothing is committed)

Last commit: `bd5fdca wip: echo discard, address filtering, voice playback fixes, tracking switched off; handover`.

Modified (all uncommitted):

- **UI rework** (about 20 files under `app/src/`, plus `app/src-tauri/tauri.conf.json`): dark slate default, light
  option, Windows 11 plus XP style, three main tabs (Messages, Stations, Radio) with sub-tabs, a tuning dial in the top
  strip, status bar, wizard-style Welcome and Setup windows, group-box captions in Radio, a `modal` focus action
  (`app/src/lib/modal.ts`), a shared band list (`app/src/lib/bands.ts`). `svelte-check` 0 errors, `vitest` 21 pass.
  The window opens at 1360x800, minimum 1100x620. Audit scored 19/20 (`.impeccable/audit/`).
- **`crates/kahf-daemon/src/lib.rs`**: echo fixes (section 4) **plus debug leftovers that must be removed**.
- **New files:** `crates/kahf-sim/examples/kami1c_wav.rs` (SDR++ wav generator, keep), `docs/MESH-SPEC.md`,
  `docs/MESH-DESIGN.md` (superseded draft, kept), `PRODUCT.md` and `.impeccable/` (Impeccable design tool context:
  commit these), `.github/agents|hooks|skills` (installed for Copilot by the design tool installer: **do not commit**,
  delete or ignore).

Suggested commits, in this order, only after the checks pass:

1. `feat(app): dark slate UI with three tabs, tuning dial, focus handling` (everything under `app/`, `PRODUCT.md`,
   `.impeccable/`).
2. `fix(daemon): echo window shorter than the answer gap; real-time test bus` (the daemon change, **after** removing the
   debug lines and passing the tests).
3. `docs: mesh specification (17 m, lanes, planner, paging)` (`docs/MESH-SPEC.md`, `docs/MESH-DESIGN.md`, this file,
   `docs/NEXT-SESSION-PROMPT.md`).
4. `example: SDR++ wav generator` (`crates/kahf-sim/examples/kami1c_wav.rs`).

Do not commit `.wav` files. The generated wavs are in the owner's `~/Downloads` only.

## 4. The echo bug: root cause found, fix applied, **not yet verified**

Original symptom (`docs/HANDOVER-2026-11-18.md`): two engines on one shared echoing channel never deliver a message. Tests:
`one_message_over_an_echoing_channel_arrives_once` and
`several_messages_each_way_all_arrive_once_in_order_and_none_from_self` in `crates/kahf-daemon/src/lib.rs`.

Two causes, both found by tracing (`KAHF_TRACE=1`):

1. **Engine bug (real).** After every transmission the station is deaf for `ECHO_TAIL` to ignore its own echo. It was
   **400 ms**, while the MAC makes the station a burst was *for* answer after `SIFS_MS` = **300 ms**
   (`crates/kahf-core/src/mac.rs`). So the first 100 ms of every answer fell inside the deaf window and was thrown away; a
   short answer could be lost entirely. Fix applied: `ECHO_TAIL` = **200 ms**, plus a compile-time guard
   `const _: () = assert!(ECHO_TAIL + 50 ms <= SIFS_MS)` (lib.rs lines near 1347).
2. **Test bus bug (mine).** The `Bus`/`BusRadio` test helper handed over queued audio ahead of real time and padded
   "silence" for the same period, putting gaps inside bursts. Fix applied: `BusRadio` now has `start` and `delivered`
   and returns exactly one second of audio per real second (queue content in order, silence only where nothing was
   sent). Lines about 3660 to 3695.

Also: the bus first returned **no audio at all when nobody transmitted**, so a short KAMI-1c burst waited forever for
audio that never came (`finish_kami` only gives up after `longest` frame). A real sound card always delivers samples;
the new `BusRadio` does too.

**Verify** (about 60 to 120 s per test today; run in the background, do not poll):

```bash
cd /Users/kristijonasadomaitis/kahf-work/wt-rig
pgrep -fl 'kahf-alpha|kahf-bravo'     # must print nothing
KAHF_TRACE=1 timeout 130 cargo test -q -p kahf-daemon one_message_over_an_echoing -- --nocapture > /tmp/echo.txt 2>&1; grep -v "Spectrum\|Metrics" /tmp/echo.txt | head -60
```

Expected after the fix: station 1 gets `Connected`, answers, station 0 gets `Connected` (a `Link(Connected)` event for st0, which
never appeared before), then `Event::Message` arrives at st1 once. If st0 still never decodes st1's answer, look next at:
(a) the very first call at t=0 was also lost in traces (a preamble was found, but nothing decoded) which may be the bus
start-up; (b) `finish_kami` retries a mode only when `fl > kami_tried`, so a burst whose first attempt ran on partial audio is
never retried for the same mode; (c) the MAC `Answer` class and `rx_locked` handling.

**Remove these debug lines before committing** (all added this session in `crates/kahf-daemon/src/lib.rs`; the
`KAHF_TRACE` check inside `run_bus` near line 3726 is original, keep it):

- `if std::env::var_os("KAHF_TRACE").is_some() { eprintln!("TRACE preamble at ... ") }` in `decode_frames` (about line 3355).
- `... eprintln!("TRACE gave up, avail ...") ...` in `finish_kami` (about line 3455).
- The 3-line `if std::env::var_os("KAHF_TRACE")... eprintln!("TRACE-RX ...")` block at the top of `on_frame` (about line 3483).
- The `if std::env::var_os("KAHF_NOFIX").is_none() { e.set_fixed_mode("KAMI-1c").unwrap(); }` wrapper in `bus_engines`
  (about line 3710): restore the plain `e.set_fixed_mode("KAMI-1c").unwrap();`.

Then run `cargo clippy -q -p kahf-core -p kahf-daemon -p kahf-sim --all-targets -- -D warnings` and
`cargo test -q -p kahf-daemon` (about 45 s today).

## 5. Make tests fast first (the owner's explicit request)

The daemon engines run on the **wall clock**: `kahf-daemon/src/lib.rs` has 46 `Instant::now()` calls and real
`thread::sleep` in the worker loop and tests (`tx_advance` paces audio against elapsed real time; `BusRadio` delivers audio
by elapsed real time). So a message that takes 40 s of radio time takes 40 s to test.

Plan (this is `MESH-SPEC.md` step 1): add a `Clock` abstraction (one `now()` source) that is real in the app and
**virtual in tests**, move every `Instant::now()`, `elapsed()` and sleep behind it, and let tests advance the clock
(for example 20x or by jumping to the next due event). `BusRadio` uses the same clock. Acceptance: the existing daemon suite
(about 45 s) runs in under 10 s with the same results, and the two echo tests run in a few seconds.
The many-station simulator in `kahf-core/src/sim.rs` already runs on virtual milliseconds; use it for network-level tests.

## 6. Verified and not verified

Verified this session: `svelte-check` 0 errors; `vitest` 21 pass; design detector clean except a map-grid advisory; the UI in the
browser at 1360x800 and 1100x640 (dial, tabs, dialog focus, no overflow); contrast measured from the colour tokens; the
17 m band-plan facts (read from the IARU Region 1 June 2016 PDF); the lane table (computed).

**Not** verified: the echo fix (no complete test run after it); the real Tauri window (only the Vite preview was used); touch on a
real device; text scaling; rendered contrast on the spectrum and map canvases; anything about KAHF on a real HF channel; the
whole mesh (nothing built); that the 2016 IARU plan is still current; Regions 2 and 3 rules; the narrow-lane modem.

## 7. The website (paused)

Source: `server/src/web/home.js`, `styles.js`, `script.js`, `live-map.js`, `server/src/page.ts` (server-rendered, plain
HTML, no external scripts, fonts or images, a hashed CSP; tests in `server/test/` including `page.test.mjs`, 1139 pass).
Run locally: `cd server && node tools/serve-local.mjs 8787`, then `http://localhost:8787/`.

Findings: it describes the **old OFDM modem** (153 carriers, 15.625 Hz spacing, LDPC, QPSK/16QAM/64QAM) and shows an old app
screenshot (`server/src/web/operate.png`); the app now uses KAMI (Walsh, one 2.4 kHz channel), see `docs/KAMI-MODES.md`.
The owner agreed these facts may be updated from `docs/KAMI-MODES.md` (list every changed claim). The owner said the
**downloads are on the GitHub repo** (`https://github.com/adswill/kahf-modem`); check with `gh release list --repo
adswill/kahf-modem` and link the real release page; do not invent URLs. The Impeccable roll
(`impeccable concept-seed --scope direction --mode persuade`) was about to run when the owner paused the work. Product
context is in `PRODUCT.md`. Keep: licence/responsibility/privacy text word for word, no external resources.

## 8. Practical notes that cost time this session

- zsh: a failing glob like `--include=*.rs` errors; quote it or use `-I` and plain `grep -r`. On macOS use `sed -i ''`.
- The browser pane page is "hidden" at times: `requestAnimationFrame` may not fire; use `setTimeout` (the dialog focus code does).
- Preview server for the app: `cd app && npm run dev -- --port 5199`; open `http://localhost:5199/`. First screen is Welcome;
  "Just look around" enters the demo. Resize with `resize_window` to 1360x800, then reset with preset `desktop`.
- Python with numpy for wav work: `/Users/kristijonasadomaitis/kahf-work/plotenv/bin/python -I script.py`.
- Wavs for SDR++ made this session (in `~/Downloads`): `kami1c-*`, `kami8-*`, and
  `baseband_14106500Hz_kahf-18-lanes_2400ksps.wav` (18 lanes, 2.4 MS/s). The generator for the first two is
  `crates/kahf-sim/examples/kami1c_wav.rs` (argument `1c` or `8`). The 18-lane script was a scratch file and is not kept.
- The Impeccable design hook is wired for this project; it prints findings after UI file edits.
- `git status` shows `server/.wrangler/...` untracked; ignore it (do not commit).

## 9. Where to start (exact order)

1. `pgrep -fl 'kahf-alpha|kahf-bravo'` must be empty. Read `MESH-SPEC.md` sections 0, 1, 5, 14.
2. Remove the debug lines (section 4). Run the two echo tests once. Commit 1 (UI) now regardless: it is independent and checked.
3. If the echo tests pass: commit 2. If not: use the trace as in section 4 and fix, one hypothesis at a time.
4. Do `MESH-SPEC.md` step 1 (virtual time). Everything after is measured with it.
5. Continue steps 3 to 16 of the spec in order. After each step: acceptance test passes, clippy clean, commit, push
   `app-rig-relay`.
6. Update `docs/NETWORK-STATUS.md` as things become real (do not claim anything that has no test).
7. When you reach the website, finish the Impeccable direction round with the owner (it needs their pick).

## 10. Questions the owner still has to answer (do not guess; ask once, in plain words)

`MESH-SPEC.md` section 12 (D1 to D7). The ones that block code: **D1** (are unattended relays allowed in the owner's country, and
do they want a daily operator confirmation), **D2** (accept a daytime-only network, or add 40 m and 80 m rows), **D4**
(lane pitch 700 or 800 Hz after the leakage measurement).

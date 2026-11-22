# Handover D (2026-11-18, end of session): the goal is a FINISHED product. Read this whole file first.

Work only inside this KAHF worktree (`/Users/kristijonasadomaitis/kahf-work/wt-rig`, branch `app-rig-relay`).
Never touch any dvb-t2 or OnAir folder. Read in this order: this file, `docs/HANDOVER-2026-11-18-C.md` (commit
table, engine bugs, commands, gotchas), `docs/NETWORK-STATUS.md` (one row per step: what is real and what is not),
`docs/MESH-SPEC.md` (the design; section 12 holds the owner's answers; NETWORK-STATUS records later changes),
`docs/HANDOVER-2026-11-18-B.md` section 1 (owner rules). When documents disagree, MESH-SPEC.md wins unless
section 12 or NETWORK-STATUS records a later owner decision.

## 0. What the owner wants (their words, last message)

"I want a finished product. Not incomplete, not somewhat missing, fully complete, which I can actually give to users
and they can actually use the relay network and whatnot, all the functionality." And: **17 metres only. The UI must not
mention 80, 40 or 20 metres.** The owner is tired and often away: "you decide". Decide, do it, write down what and why.
The owner is not impressed by partial work: the two test instances they ran today (old build, before the 17 m change)
looked like the old app, because the mesh is not wired into the engine or the UI yet.

## 1. Honest state at the end of this session

Built, tested and pushed (MESH-SPEC section 14 steps 1 to 16), **but only as separate pieces on the bench or in
simulation. Almost none of it is used by the app yet.** That is the gap to close:

| Piece | Where | Used by the daemon/app today? |
|---|---|---|
| Virtual-time engine tests, echo and receive fixes | `kahf-daemon/src/clock.rs`, `lib.rs` | yes (engine) |
| HARQ chase combining | `kahf-core/src/harq.rs` | yes (decode only; no end-to-end test) |
| Band table, lanes, `permit_tx` | `kahf-core/src/lanes.rs` | **no** |
| Narrow lane mode | `kahf-core/src/narrow.rs` | **no** |
| Lane sensing, classes, quarantine | `kahf-core/src/lanesense.rs` | **no** |
| Frame header, bundles, ordered answers | `kahf-core/src/bundle.rs` | **no** |
| Planner (+ bonding plan, HOP grants) | `kahf-core/src/planner.rs`, `server/src/planner.js` | **no** |
| Relay referee (calling lane) | `kahf-core/src/referee.rs` | **no** (simulation) |
| Router, presence, paging, retry schedule | `server/src/router.js` | **no** (not behind any route) |
| Push link | `kahf-daemon/src/push.rs`, `server/src/hub.js` | **no** (no real transport) |
| Receive diversity | `kahf-core/src/diversity.rs` | **no** |
| Hopping | `kahf-core/src/hop.rs` | **no** |
| Propagation, learner, solar, listening pattern | `server/src/propagation.js` | **no** (RAM only) |
| Soak simulation | `server/test/soak*.mjs` | test only |

Verified facts only (do not overstate): daemon suite 77 tests (3 s), core 347, server 1173, svelte-check 0 errors,
vitest 24. **Nothing has ever been on a real radio channel. No radio speed has been measured.** The simulated week
shows no duplicate delivery and 99.7 to 100 percent of deliverable messages arriving; its times are the spec's assumed
air times.

### Uncommitted-then-committed work at the very end: "17 m only" (commit `wip: 17 m only ...`)
Made in the last minutes, **tests partly run, workspace clippy NOT run** (the owner interrupted the last command):
- `app/src/lib/bands.ts` (new): the 12 lanes (R1 to R8 relay, C1 and C2 calling, X1 and X2 spare), `khz` = USB dial,
  `DEFAULT_LANE` = C1 (18113.44 kHz), `laneKhzOrDefault()` (old 20/40/80 m settings become C1). `bands.test.ts` checks
  every number against MESH-SPEC 2.2. `RadioStrip.svelte` (12 ticks, 5-decimal readout), `Radio.svelte` (12 lane tabs,
  17 m text), `Setup.svelte` (one 17 m card, C1), `App.svelte`, `format.ts` (`mhz` shows 5 decimals), `HelpPanel`,
  `mock.ts`, `defaults.ts`, `state.svelte.ts` (migration on start).
- `app/src-tauri/src/settings.rs`: `freq_khz` is now `f64`, `lane_khz()` migration, `DEFAULT_LANE_KHZ`, SDR spec in Hz.
- `crates/kahf-core/src/bandplan.rs`: `KAHF_SEGMENTS` is only 17 m (18.111 to 18.120 MHz); warning text; tests rewritten.
- `crates/kahf-core/src/select.rs`: `Plan::default_plan()` is now the 12 lanes by centre frequency (relay lanes = Online
  pool, calling = Open, spare = Emergency); tests rewritten.
- Passed when last run: `cargo test -p kahf-core` (347), `cd app && npx svelte-check` (0 errors), `npx vitest run` (24),
  `cd app/src-tauri && cargo test` (32). **Not run:** `cargo clippy --workspace`, `cargo test --workspace`, the daemon
  suite after the `select.rs` change (the daemon uses `Plan::default_plan()` in `pick_channel`; it may change behaviour),
  and `crates/kahf-ui` (an older Rust UI that indexes `KAHF_SEGMENTS[i]` in `spectrum.rs` and `view.rs`: it will
  probably not compile or will misbehave with one segment). **First job: run all of those and fix what breaks.**
- Remaining old-band text to hunt: grep the whole repo (app, server web pages `server/src/web/*`, docs, README, release
  notes, `app/src-tauri/tauri.conf.json`, `crates/kahf-ui`) for `80 m`, `40 m`, `20 m`, `14.1`, `7.04`, `3.6`, `7103`,
  `14103`, "metres".

Two test instances are **running right now** (`pgrep -fl 'kahf-alpha|kahf-bravo'` prints two lines): alpha KA1ALF
(Dubai 25.2, 55.27) and bravo KB2BRV (Vilnius 54.69, 25.28), both on BlackHole 2ch in and out, started by the owner's
request from `/tmp/kahf-ab/start.sh` with a build made before the 17 m change. Owner rule: if pgrep prints anything,
**stop and say so before any test or audio**. Ask the owner to close them (or ask once for permission to
`pkill -f 'kahf-(alpha|bravo)'`), then run tests. To restart them on a new build, see section 9.

## 2. Definition of DONE (the owner's bar; do not stop before every line is true)

A stranger can download the app, finish a short wizard (callsign, sound card or SDR, place), and:
1. **Chat, mail with attachments, and voice notes** work between two stations, one-way, both-ways-at-once, with fast
   typing, random timing, the link closing mid-conversation, and each station hearing its own transmission. Each
   message arrives exactly once, in order, never "from" our own callsign. Status in the app is always clear (waiting,
   on air, delivered, failed with a reason) and failures retry automatically.
2. **Relay mode**: a relay owner switches it on once; the relay registers, connects to the server, takes planned slots,
   referees its calling lane, forwards between operators and over the internet, shows a status screen and a stop button.
   Operators type a callsign and press Send; nobody selects band, channel, relay or time.
3. **Paging and routing** find an operator who is not currently reachable; messages wait only in the **sender's** outbox
   (nothing is stored in the network).
4. **Never transmit over anyone**: planned slots, listen-before-talk with the +6 dB rule on the lane and both neighbours,
   voice/CW/FT8 avoidance and quarantine, one transmit token per radio, a PTT watchdog, every transmission checked by
   `lanes::permit_tx` against the signed 17 m table.
5. **17 m only** everywhere (UI, settings, band checks, docs, website, release notes). Per-region tables are data.
6. Server (Cloudflare Worker) runs planner, router, hub (WebSocket push) and propagation learner; signed plans;
   no message bodies stored anywhere (including the **old queued-message backbone, which must be removed or replaced**).
7. Installable app for macOS and Windows (and Linux if the CI already builds it), website text correct, user guide,
   relay-owner guide, privacy text (kept word for word), licence/responsibility text (kept word for word).
8. Everything has tests; all suites and clippy are clean; a soak simulation passes; a written **on-air test plan** exists
   for the owner (see section 8), because the final step needs a real transmitter and the owner's decisions.

What the agent cannot do alone, and must say plainly instead of faking: transmit on air; decide whether
`plan_verified` is true (owner only); decide D3 (Regions 2 and 3) and the country licence question for unattended
relays (owner says it is allowed where they live); deploy the server (owner must ask); measure real HF speed.

## 3. Work packages, in order. Each is complete only when its acceptance test passes, clippy is clean, docs are updated,
##    committed and pushed. No package may be left "partly wired".

**WP0. Green baseline.** Stop-the-line items first: confirm pgrep with the owner, then run: `cargo clippy -q --workspace
--all-targets -- -D warnings`, `cargo test -q --workspace`, `cd app && npx svelte-check && npx vitest run`,
`cd app/src-tauri && cargo clippy -q --all-targets -- -D warnings && cargo test -q`, `cd server && npm test`. Fix what the
17 m change broke (`crates/kahf-ui`, daemon tests that assumed the old plan, any 14.103 default in tests/docs).
Finish the 17 m sweep (section 1). Commit.

**WP1. Transmit gate and lane plan in the engine.** `Config`/engine hold a `BandTable` (built-in, plus signed update
slot). Every transmission goes through `permit_tx(band, first_k, last_k, dial, audio_lo, audio_hi)`. The engine knows its
lane (from settings `freqKhz` -> lane index) and computes the dial/audio correctly (MESH-SPEC 2.3 trap). While the built-in
table is unverified, transmit is refused with a clear message and **the app can still receive and run in test mode**: add
an explicit, loud "test table" mode that is only enabled for BlackHole/in-memory radios (never for a real sound card
without `rigctld`/SDR... decide and document), so the owner's two-instance bench works without enabling real transmit.
Tests: daemon tests with a verified table transmit; with the unverified one they refuse; the dial trap is refused.

**WP2. Sensing before every transmit.** Feed the audio into `LaneSense` for all lanes the receiver covers; `clear_to_send`
before every burst; quarantine; occupancy report summary (12 numbers a minute) queued for the server. Replace the old
`channel_busy` path. Tests: simulated voice/FT8 interferers on the `Bus` are avoided.

**WP3. Frame header, bundles and ordered answers on the air.** Use `bundle.rs` for every new frame; keep the old
link-layer tags working for direct two-station links only where still needed (document the migration). Bundles for several
recipients; answers in listed order 300 ms apart; ack bitmaps; `h24` ids. Tests: three or more engines on a `Bus`.

**WP4. Lane mode in the modem ladder.** Put `narrow.rs` into the engine as the mode used on lanes (transmit and receive,
two-lane receiver for C1 and C2 from one sound card), mode ladder entries, rate/time estimates in the UI
(`app/src/lib/defaults.ts` MODE_TABLE), re-measure net rates with `kahf-sim` and update `docs/KAMI-MODES.md` (measured,
plain-noise and HF channel models). Then bonding: the 2 to 4 lane wide waveform (chip rate scaling), used only when the
plan grants a bonded run. Tests: round trip at all lane centres and bonded widths over the `Bus`; leakage re-measured.

**WP5. Referee and relay mode.** Wire `referee.rs` into relay mode: beacon with window and load, requests, grants, virtual
carrier sense, ordered downlink; the operator side (random slot in the window, backoff doubling to 8, obey grants only).
Replace the old relay slot logic (`relay.rs`, 24 h slot) by the planner. Tests: 10 operators + 1 relay engines on one
calling lane over the `Bus` (the hidden-station test); relay crash and restart; two relays on different lanes.

**WP6. Server: planner, signing, router, hub, learner live.** Durable Objects for Directory/Planner/Router/Presence/
Propagation (MESH-SPEC 4); real WebSocket endpoint using `hub.js` (one socket per relay key, heartbeat, HTTPS poll
fallback); signed epoch plans four epochs ahead (project key; how the key is kept is the owner's call: ASK once); relay
registration kept; `bond` and `hop_grants` ported to JS with parity tests; feed `heard` reports and grant results into the
learner and store it (it may keep aggregates, never bodies); hourly NOAA fetch job; clash-domain graph and colouring
(replace "all relays are one domain"); paging scheduler; rate limits and abuse checks (a relay transmitting off plan is
suspended). **Remove the old queued-message backbone** (`backbone.js`, `MAILBOXES`, its routes, its Rust client
`backbone.rs`) once the push path replaces it; migrate users of the old endpoints; keep the public map/registration.
Tests: server suite plus a local end-to-end with `wrangler dev` or `server/tools/serve-local.mjs`. **No deploy unless the
owner asks**; prepare the deploy command and a rollback note.

**WP7. Daemon <-> server transport.** Real WebSocket client (ASK once before adding a WebSocket crate; say which and why)
and HTTPS poll fallback in `push.rs`, wired to the engine; `backbone.rs` removed or reduced to this. Relay-to-relay latency
test over a local copy of the server (target under 100 ms on loopback; say it is loopback).

**WP8. Delivery layer end to end.** Sender outbox states (Waiting, Sending, At relay, With the delivering relay, Delivered,
Waiting for receiver, Could not deliver with reason) in the daemon and the app; deliver-first-then-page routing;
presence refresh (HERE) rules; message ids; duplicate control; the **resend storm fix** (the soak showed about 29 re-sends
per message: send the receipt in the next beacon window, back off resends, stop once "at relay"); mail (up to 100 kB) and
voice over the mesh; chat while mail is sending. Re-run and extend the soak; add the owner's scenarios: 10 quick chats one
way, both at once, mail while chatting, a message while the other transmits, link closing mid-conversation, random
intervals over 3 minutes, never "from" own callsign, exactly once and in order.

**WP9. Hopping, diversity, HARQ on the air.** `hop.rs` forwarding in relays with HOP grants from the planner; LLR forwarding
to the server and `Combiner` (frame key = lane, slot, source, codeword index); HARQ keyed by codeword id in `link.rs`;
end-to-end tests that show a combined decode and a two-hop delivery over `Bus` engines. Keep the spec's limits.

**WP10. App UI for everything.** Relay screen (status, queue, planned slots, referee load, stop button, supervision note per
D1 = autonomous, no daily tap), Stations/Messages states from WP8, a lane/Radio screen that shows C1/C2 and lane occupancy
from lane sensing, "17 m is closed here until about HH:MM UTC" from the model (still listening, outbox kept), sound/voice
checks, error messages in plain words, first-run wizard (17 m only), settings migration, accessibility check, dark and
light themes, vitest and `svelte-check` clean, Tauri window opened and checked (previous sessions only used the Vite
preview: do not claim the real window works until it was run).

**WP11. Packaging and release.** Build installers with the project's CI (`.github/workflows`; look first), version bump,
release notes (17 m only; keep the licence/responsibility/privacy text word for word), `gh release` only if the owner asks.
Website: update `server/src/web/*` to describe KAMI and the 17 m mesh instead of the old OFDM text (handover B section 7:
list every changed claim; link only real GitHub release URLs; no external resources). User guide (`docs/USER-GUIDE.md`)
and relay-owner guide (`docs/RELAY-GUIDE.md`) in plain words.

**WP12. Final soak and conformance.** Longer soak with all packages in (include real engines for a small cell), the
no-storage conformance test extended to the daemon's data folder with compression-aware scanning, security review
(`/security-review` skill is available), `docs/NETWORK-STATUS.md` fully updated, version of this handover.

**WP13. Hand the owner the on-air plan.** Write `docs/ON-AIR-PLAN.md`: what must be true before the first transmission
(band plan re-verified against the current IARU document by a human, D1 to D3 answered, a dummy load test, power and ALC
settings, station ID, an observer on the band), a step-by-step first test with two real stations on C1, then a relay, with
stop conditions and what to record. Stop there: do not transmit.

## 4. Owner rules (do not break)
1. Only edit KAHF folders. No subagents unless the owner asks.
2. `pgrep -fl 'kahf-alpha|kahf-bravo'` before any test or audio. If anything prints, stop and say so (see section 1).
3. Commit with the configured `adswill` noreply identity; every commit ends with
   `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Push only `app-rig-relay`.
4. No server deploy unless asked. No transmit-enable; `plan_verified` stays false until the owner says otherwise.
5. Plain, short replies; say what is not verified; never present an (est.) number as measured, never quote simulated times
   as radio speed. **Read clippy's output before every commit** (one commit this session went in failing).
6. When the owner says "you decide": choose the best option, do it, record it in `docs/NETWORK-STATUS.md`.
7. ASK the owner (once, plain words) only for: the project signing key handling, a WebSocket crate, deploying the server,
   removing/migrating the old backbone data, tagging a release, anything on air.

## 5. Band plan facts (re-read at the end of this session; keep `plan_verified` false)
Current IARU Region 1 HF band plan: `https://www.iaru-r1.org/wp-content/uploads/2021/06/hf_r1_bandplan.pdf` (same bytes as
`https://www.uba.be/sites/default/files/media/files/hf_r1_bandplan.pdf`), "Effective from 16 OCT 2020, edited by DF5JL". 17 m
rows (kHz): 18068-18095 200 Hz CW; 18095-18105 500 Hz narrow digimodes; 18105-18109 500 Hz narrow digimodes incl. automatically
controlled data stations (unattended); 18109-18111 IBP beacons exclusively; **18111-18120 2700 Hz all modes, digimodes,
automatically controlled data stations (unattended)**; 18120-18168 2700 Hz all modes. Matches MESH-SPEC 1.1 and
`BandTable::builtin_r1()`. Not settled: the wording on unmanned stations and contests, whether a newer plan exists, the
FT8/FT4/WSPR/IBP positions in `excluded` (from memory), Regions 2 and 3 (UBA page `https://www.uba.be/en/hf/bandplanning`
lists their PDFs and the ARRL plan), each country's licence rules. Write the reading into MESH-SPEC 1.1 and NETWORK-STATUS
(the previous agent did not get to it; check `git log`).

## 6. Facts from the soak the owner should know
No message seen twice by a user (about 2000, 3 seeds); every message ended delivered or failed with a reason; 99.7 to 100
percent of deliverable messages arrived; the 400 message flood was handled. Paging first gave a 322 s median on good links;
deliver-first (now in `router.js`) gives 33 s in the sim, 90th percentile 827 s, using the spec's assumed air times (not
radio measurements). Resending is heavy (about 29 per message): WP8.

## 7. Commands
```bash
cd /Users/kristijonasadomaitis/kahf-work/wt-rig
pgrep -fl 'kahf-alpha|kahf-bravo'                                       # must print nothing before tests
cargo clippy -q --workspace --all-targets -- -D warnings
cargo test -q --workspace                                               # daemon ~3 s, core ~5 s
(cd app && npx svelte-check && npx vitest run)
(cd app/src-tauri && cargo clippy -q --all-targets -- -D warnings && cargo test -q)
(cd server && npm test)                                                 # 1173 tests incl. soak
cargo run --release -p kahf-sim --example lane_leak                     # ~5 min
```
Server local: `node server/tools/serve-local.mjs 8787`. App preview: `cd app && npm run dev -- --port 5199`.

## 8. Tools and gotchas
- `curl`/`wget` in Bash are redirected by a hook: fetch URLs with the `ctx_execute` tool (JavaScript `fetch`); downloaded
  files go in their own folder under the scratchpad directory, never the repo.
- zsh: quote globs; macOS `sed -i ''`. Untracked `.github/agents|hooks|skills` and `server/.wrangler/...`: never commit.
- Virtual clock: a loop that never sleeps hangs a virtual-time test; threads blocking elsewhere use `clock.park`.
- Test radios (`BusRadio`, `FakeRadio`, `MeshRadio`) deliver one second of audio per second and mix by time; daemon tests use
  `vspawn(...)` and `test_clock()`. Real-time tests (`poll_events_returns_quickly_during_transmit`,
  `spectrum_arrives_at_least_20_times_a_second`) stay on the wall clock on purpose.
- Rust planner and `server/src/planner.js` must stay identical: `cargo test -p kahf-core known_plans -- --nocapture` prints
  the expected strings held in `server/test/planner.test.mjs`.
- Do not trust a green sim for radio claims; keep the "bench only / not on air" words in NETWORK-STATUS until true.

## 9. Restarting the owner's two-instance bench on a new build (only when the owner is not using it)
```bash
cd /Users/kristijonasadomaitis/kahf-work/wt-rig/app && npm run build && cd src-tauri && cargo build -q
cp target/debug/kahf-app /tmp/kahf-ab/kahf-alpha && cp target/debug/kahf-app /tmp/kahf-ab/kahf-bravo
/tmp/kahf-ab/start.sh          # kills old ones, wipes data-alpha/bravo, starts both with BlackHole 2ch
```
`/tmp/kahf-ab/alpha.settings.json` and `bravo.settings.json` still say `"freqKhz":14103`: change both to `18113.44` (the
app also migrates it). Binary path is `app/src-tauri/target/debug/kahf-app`.

## 10. When you stop
Update `docs/NETWORK-STATUS.md`, write `docs/HANDOVER-<date>-E.md` and a paste-ready `docs/NEXT-SESSION-PROMPT-E.md`
(this session's files are `HANDOVER-2026-11-18-D.md` and `NEXT-SESSION-PROMPT-D.md`). Say in the final reply, in plain words,
which of the "definition of done" lines are true, which are not, and what only the owner can do.

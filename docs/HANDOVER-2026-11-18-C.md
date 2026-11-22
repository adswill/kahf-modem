# Handover C (2026-11-18, end of the mesh build session): read this whole file first

Work only inside this KAHF worktree (`/Users/kristijonasadomaitis/kahf-work/wt-rig`, branch `app-rig-relay`).
Never touch any dvb-t2 or OnAir folder. This file adds to `docs/HANDOVER-2026-11-18-B.md` (owner rules are
repeated below) and to `docs/MESH-SPEC.md` (when documents disagree, MESH-SPEC.md wins, except where section 12
there records an owner answer or a later change). Then read `docs/NETWORK-STATUS.md`: it has one row per step with
what is built, the evidence and what is NOT built. Do not claim anything that has no test.

## 1. Owner rules (do not break)

1. Only edit KAHF folders. No subagents unless the owner asks.
2. Before any test or audio: `pgrep -fl 'kahf-alpha|kahf-bravo'`. If anything prints, stop and say so.
   (It was empty every time this session.) Do not start `/tmp/kahf-ab/start.sh` unless asked.
3. Commit with the repo's configured identity (`git config user.name` is `adswill`, email
   `326940294+adswill@users.noreply.github.com`). End every commit message with
   `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Push **only** branch `app-rig-relay`.
   Merges into `release-0.1` only on request.
4. **No server deploy unless asked.** (`cd server && npx wrangler deploy` needs `dangerouslyDisableSandbox`;
   this Mac's web filter blocks kahf.dev: curl needs `--resolve kahf.dev:443:104.21.3.235 -k`.)
5. **Never transmit-enable anything** until the band-plan facts are re-verified (see section 5) and the owner says so.
6. Plain, short replies. Say what is **not** verified. Never present a number marked (est.) or (verify) in the spec as
   measured. The owner dislikes waiting on slow tests: tests run on virtual time now (3 s for the daemon suite).
7. The owner often says "I am not here, you decide". Then choose the best option, do it, and write down what you chose
   and why (this session: "deliver first, page only on failure").
8. Check clippy before every commit and read its output (one commit this session went in with a clippy error and needed
   two follow-up commits): `cargo clippy -q -p kahf-core -p kahf-daemon -p kahf-sim --all-targets -- -D warnings`.
9. Tools: `curl`/`wget` in Bash are redirected by a hook; fetch URLs with the `ctx_execute` tool (JavaScript `fetch`).
   Untrusted downloads go in their own folder under the scratchpad directory (a downloaded band plan is in
   `.../scratchpad/bandplan-dl/`), never in the repo.

## 2. What was done this session (all committed and pushed; last commit `4db6656` plus whatever is after it)

| Commit | What |
|---|---|
| `a9fafdd` | UI rework (dark slate, three tabs, tuning dial) |
| `75f4579` | MESH-SPEC, handover B, SDR++ wav example |
| `3371457` | **Virtual time** (`kahf-daemon/src/clock.rs`), echo fix, four engine bugs |
| `b0d1c22` | Step 3: `kahf-core/src/lanes.rs` (band table, signing, dial math, `permit_tx`) |
| `53c2610` | Step 4: `narrow.rs` (KAMI at 1/5 chip rate) and `kahf-sim/examples/lane_leak.rs` |
| `c078e17` | Step 5: `lanesense.rs` (per-lane floor, +6 dB rule, classes, quarantine) |
| `5a123f0` | Step 6: `bundle.rs` (9 byte header, bundles, ordered answers, ack bitmaps) |
| `1c495dc` + 2 fixes | Step 7: `planner.rs` and `server/src/planner.js` (identical plans) |
| `517ca56` | Step 8: `referee.rs` (calling lane referee, simulated) |
| `e43de74` | Step 9: `server/src/router.js` (presence, routing, paging, retry schedule) |
| `6c69b55` | Step 10: `kahf-daemon/src/push.rs` and `server/src/hub.js` (push link logic) |
| `8907728` | Step 11: no-storage tests |
| `4603c5e` | Step 12: `diversity.rs` (receive diversity) |
| `3c8ac52` | Step 13: `harq.rs` (used in the daemon decode) and lane bonding plan in `planner.rs` |
| `e49f8ba` | Step 14: `server/src/propagation.js` (model, learner, solar parsing, predictive paging) |
| `8fdfe01` | Step 15: `hop.rs` and `planner::hop_grants` |
| `abb1331`, `4db6656` | Step 16: `server/test/soak*.mjs`; router changed to deliver first |

Owner answers recorded in MESH-SPEC section 12: D1 = autonomous 24/7, status screen only, no daily tap (the owner says it
is allowed where they live); D2 = 17 m only first; D4 = widen to 800 Hz only if the leakage test needs it (it did not:
pitch stays 700 Hz). D3 (Regions 2 and 3) and D5 to D7 are open.

## 3. The real engine bugs found with virtual time (so you know what is in `crates/kahf-daemon/src/lib.rs`)

1. `ECHO_TAIL` 200 ms, shorter than the answer gap `SIFS_MS` 300 ms (compile-time check).
2. The first codeword of a new burst is decoded alone (`n = n.min(1)` while the mode is unknown).
3. The preamble estimate is redone once the whole preamble has arrived (`kami_refined`).
4. A heard but undecoded preamble holds our own transmit for up to 2.5 s (`PREAMBLE_HOLD`, `rx_provisional`).
5. A queued item waiting for the channel no longer stops receiving (the loop falls through to `rx_step`).
6. HARQ: failed soft bits are kept and a repeat is added to them (`harq.rs`, used in `finish_kami`).

Test radios (`BusRadio`, `FakeRadio`, `MeshRadio`) deliver one second of audio per second and mix by time. Daemon tests
call `vspawn(...)` and `test_clock()`; real-time tests (`poll_events_returns_quickly_during_transmit`,
`spectrum_arrives_at_least_20_times_a_second`) use `Engine::spawn` and real sleeps on purpose.

## 4. What is NOT done (the work left, in the order I would do it)

Nothing below needs a radio. Items marked ASK need the owner's OK first (say what and why, in plain words, once).

1. **Wire the written pieces into the daemon** (none of them is used by the engine yet): `bundle.rs` frames, `hop.rs`
   forwarder, `lanesense.rs` before every transmit, `referee.rs` on a relay's calling lane, `lanes::permit_tx` as the
   gate on every transmission, `narrow.rs` as a lane mode and mode ladder entry (`app/src/lib/defaults.ts` too).
   Do it in small steps with a daemon test on virtual time for each (two or three engines on a `Bus`).
2. **Cut the heavy resending.** The soak shows about 29 re-sends per message: a receipt only returns while the sender is
   listening, and every resend still costs air time. Options: send the receipt in the next calling-lane beacon window;
   exponential resend spacing; stop resending once the relay says "at relay B". Re-run `server/test/soak.test.mjs`.
3. **Real transports for the push link** (ASK: needs a WebSocket crate in `kahf-daemon` and a Durable Object plus a
   `wrangler.toml` binding for `server/src/hub.js`). Then change `backbone.rs` to use `push.rs`.
4. **Remove or replace the old queued-message backbone** (ASK: it changes the live server). `server/src/backbone.js`
   with `MAILBOXES` still stores message bodies in Durable Objects, against the no-storage rule. The tests in
   `server/test/nostorage.test.mjs` do not cover it. Listed in NETWORK-STATUS step 11.
5. **Server side of the new modules**: sign and publish plans (JS has no signing yet), a JS twin of `bond` and
   `hop_grants`, feed relay `heard` reports and results into the learner, store the learner (it is RAM only), the hourly
   NOAA fetch job, clash-domain graph from measured footprints (the planner treats all relays as one domain).
6. **Bonded-lane waveform** (2 to 4 lanes as one wide signal; `narrow.rs` makes one lane only) and the lane mode's own
   re-optimised ladder (MESH-SPEC 2.4 build order step 2).
7. **Chase combining test in the daemon** end to end (only unit tested), and `link.rs` HARQ keyed by codeword id.
8. **Website** (`server/src/web/*`): still describes the old OFDM modem; owner paused it. See handover B section 7.
9. **Step 17, on the air**: not allowed yet (section 5).

## 5. The band plan check (done at the very end; only partly written into docs)

I fetched the current IARU Region 1 HF band plan: `https://www.iaru-r1.org/wp-content/uploads/2021/06/hf_r1_bandplan.pdf`
(identical bytes to the UBA copy `https://www.uba.be/sites/default/files/media/files/hf_r1_bandplan.pdf`), header
"IARU Region 1 HF Band Plan, Effective from 16 OCT 2020, edited by DF5JL". Its 17 m rows (kHz):

- 18068 - 18095: 200 Hz, CW
- 18095 - 18105: 500 Hz, narrow band modes, digimodes
- 18105 - 18109: 500 Hz, narrow band modes, digimodes, automatically controlled data stations (unattended)
- 18109 - 18111: International Beacon Project, beacons exclusively
- **18111 - 18120: 2700 Hz, all modes, digimodes, automatically controlled data stations (unattended)**
- 18120 - 18168: 2700 Hz, all modes (18130 SSB QRP, 18150 digital voice, 18160 emergency centres of activity)

These match the rows in MESH-SPEC section 1.1 (read from the 2016 edition) and in `BandTable::builtin_r1()`. What this
does **not** settle, so `plan_verified` stays **false** in `lanes.rs` and nothing may transmit:

- I read only the 17 m rows. I did not re-read the plan's wording on unmanned stations or contests (spec 1.1), nor
  whether a newer plan than 16 Oct 2020 exists (the IARU page links only this one; check the page again).
- The FT8, FT4, WSPR and IBP positions in the table's `excluded` list are from memory, not from the plan (verify).
- Regions 2 and 3 (and the US data limit at 18.110) were not read; D3 is open. The PDFs are linked from
  `https://www.uba.be/en/hf/bandplanning`.
- Each country's own licence rules (unattended stations) are the owner's to check (D1).
Flipping `plan_verified` to true is a decision for the owner, not the agent. Put the reading above into MESH-SPEC 1.1 and
NETWORK-STATUS if the previous agent did not (check `git log` first).

## 6. Facts from the soak that the owner should know (from `NETWORK-STATUS.md` step 16)

- No message was seen twice by a user (about 2000 messages, 3 seeds); every message ended delivered or failed with the
  reason "lifetime over"; 99.7 to 100 percent of the deliverable ones arrived; the 400 message flood was handled.
- The spec section 11 speed (12 to 35 s) did not hold with paging first (median 322 s). With deliver-first the sim gives a
  first-try median of 33 s and a 90th percentile of 827 s. These times are the spec's assumed air times; **no radio speed
  has been measured**. Say so every time you quote them.

## 7. Commands

```bash
cd /Users/kristijonasadomaitis/kahf-work/wt-rig
pgrep -fl 'kahf-alpha|kahf-bravo'                      # must print nothing
cargo test -q -p kahf-core                             # about 5 s (347 tests)
cargo test -q -p kahf-daemon                           # about 3 s (77 tests, virtual time)
cargo clippy -q -p kahf-core -p kahf-daemon -p kahf-sim --all-targets -- -D warnings
(cd server && npm test)                                # 1173 tests, includes the soak (about 10 s)
cargo run --release -p kahf-sim --example lane_leak    # about 5 minutes
git push origin app-rig-relay
```

Debugging the daemon: `KAHF_TRACE=1 cargo test -q -p kahf-daemon <name> -- --nocapture` prints events with virtual
timestamps (the trace in `run_bus` is original). For finer traces add temporary `eprintln!` guarded by an env var and
remove them before committing (grep for the variable name; this session's were all removed).

## 8. Gotchas that cost time

- zsh: quote globs; macOS `sed -i ''`. `git status` shows untracked `.github/agents|hooks|skills` and
  `server/.wrangler/...`: never commit them.
- `Clock`: a virtual clock only advances when every joined thread sleeps on it. A loop that spins without sleeping
  hangs a virtual-time test (it was a real busy-loop bug). Threads that block elsewhere must use `clock.park`.
- A test that panics can leave a stale `/tmp/claude-501/*.bin` from old debug runs; they are not used by the repo.
- The Rust planner and `server/src/planner.js` must stay identical: `known_plans_for_the_js_twin` prints the expected
  strings (`cargo test -p kahf-core known_plans -- --nocapture`), `server/test/planner.test.mjs` holds them.

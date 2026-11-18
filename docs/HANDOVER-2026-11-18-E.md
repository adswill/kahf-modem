# Handover E (2026-11-18, session E)

Read, in order: `docs/HANDOVER-2026-11-18-D.md` (definition of done section 2, work packages section 3, owner rules
section 4), this file, `docs/NETWORK-STATUS.md` (the last sections, one per package), `docs/MESH-SPEC.md`.
Branch `app-rig-relay`, worktree `/Users/kristijonasadomaitis/kahf-work/wt-rig`.

## Done this session (all committed and pushed unless noted)

| Package | State |
|---|---|
| WP0 green baseline, 17 m sweep | done (`9531444`) |
| WP1 transmit gate and bench-only test table | done (`b79d64e`) |
| WP2 lane sensing before every transmit, 10 minute noise floor, occupancy reports | done (`cd037f3`); reports are queued in memory, sending them is WP6/7 |
| WP3 chat on bundle frames, multi-recipient bursts, ordered answers, duplicate control | done (`259a6a4`); mail and files still use the link |
| WP4 lane mode in the engine (12 lanes, streaming lane receiver, two-lane receive, lane speeds in the app) | done except the **bonded waveform** (2 to 4 lanes) and an HF fading sweep of lane bursts |
| WP5 relay as referee of its lane | built and tested with 1 and 3 hidden operators (6 took about 5 minutes); ten operators: see NETWORK-STATUS and `REF_OPS=10 cargo test -p kahf-daemon hidden_operators -- --nocapture` (about 15 minutes of real time). Not done: relay crash and restart, two relays on different lanes, grants on other lanes |
| WP11 (part) | `docs/USER-GUIDE.md`, `docs/RELAY-GUIDE.md` rewritten for the lane app; the kahf.dev home page redesigned and **deployed** (owner asked): plotting-sheet world, lane ruler, a relay's round, honest status, a developer section; no old OFDM text on the page. Release notes not checked. No installer exists |
| WP13 | `docs/ON-AIR-PLAN.md` written (do not transmit) |
| Extras the owner asked for | Ko-fi link in README, Settings > About and the site; a support note in the app (not in the first week, then at most monthly, dismissible, "don't ask again") |

## Not done (in order)

1. **WP6 server**: Durable Objects, WebSocket hub, signed plans, router and learner live; remove the old queued-message
   backbone (`backbone.js`, `MAILBOXES`, `backbone.rs`). Needs the owner for: the signing key handling, a WebSocket
   crate, removing the old backbone data. The old backbone still stores message bodies in Durable Objects: the "no
   stored messages" claim is a design goal, not true of the live server.
2. **WP7 transport** (WebSocket client in `push.rs`, wired to the engine).
3. **WP8 delivery layer**: sender states beyond sent/failed (At relay, With the delivering relay, Waiting for receiver),
   deliver-first routing in the engine, mail and voice over the mesh. The datagram acks already cut the resend storm
   for chat.
4. **WP9** hopping, diversity and HARQ on the air.
5. **WP10 app UI** for relay, lanes and the 17 m closed notice; check the real Tauri window (nothing was run in it this
   session, including the support note and the Ko-fi button).
6. **WP11** release notes and installers; **WP12** soak, conformance, security review.
7. Referee: ten-operator capacity, crash and restart, two relays.

## Facts worth knowing

- The engine sends lane bursts (480 Hz, five times slower) whenever the radio's dial is a lane dial; any other dial is the
  old wide modem and is refused by the gate on a verified table.
- The test bus has no radio frequency translation (every station's 1500 Hz audio lands on 1500 Hz for all), so stations on
  different lanes cannot be tested on it. Receive of other lanes is tested by injecting audio.
- Lane air times (KAMI-1a, one codeword): 1.8 s; KAMI-1c: 0.95 s; KAMI-32: 7.2 s. A request slot of 2 s needs 1a or
  faster, so a weak operator cannot use the referee's request slots yet.
- `plan_verified` is false. Nothing has been on a radio. Never present simulated times as radio speed.

## Owner decisions still open (ask once, plain words)

Signing key handling; a WebSocket crate for the daemon; removing the old backbone data; tagging a release; anything on
air (`docs/ON-AIR-PLAN.md`).

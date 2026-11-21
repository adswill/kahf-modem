# Release readiness (2026-11-21)

What must be true before KAHF is published to people, and where each line stands. Honest: "tested" means tests on a virtual clock and in-memory buses, never a radio.

## Done and tested
- Engine: lane gate, lane sensing, bundle chat with delivery states, relay referee (ten operators in simulation), two relays chained, relay restart, relay keeps no text, resend storm bounded.
- Server: signed mesh (WebSocket and HTTPS), plans, hop and bond grants, security fixes, twelve-lane plan, site and legal pages. 1172 tests.
- App (Alloy): twelve lanes, gate notice, referee counters, delivery states, Ko-fi button, wrapped text. 37 tests plus offscreen screenshots.
- Pull request 40 runs CI on Linux, plus the desktop app job.

## Not done: only the owner can do these
1. **Check the 17 m band table** against the current IARU Region 1 band plan (18.111 to 18.120 MHz; "all modes, digimodes, automatically controlled data stations"; the lane width 480 Hz must be inside the bandwidth limit of that part). Then set `plan_verified` to true in `crates/kahf-core/src/lanes.rs`. Until then the app refuses to transmit on a real radio, so nobody can use the relay network on air.
2. **Deploy the server** (`cd server && npx wrangler deploy`). kahf.dev runs version 58e80977; the twelve-lane plan and page changes (commit 2c33ca8) are not deployed. The auto-mode safety check blocked session F from deploying twice.
3. **Merge pull request 40** after CI is green, then **tag a release** (the release workflow builds the installers).
4. **First on-air test** with two real radios, following `docs/ON-AIR-PLAN.md`.

## Not done: engineering
- Hop grants and combining weak copies from several relays are not used by the engine (WP9).
- No soak with real radios or real sound cards; no HF fading sweep of lane bursts.
- Alloy's wrapped-text layout bug is worked around in the app only.
- A busy report can name only the lanes whose frequency is a whole kHz (nothing sends such reports yet).
- Screens not checked by picture: Setup after the first step, Settings (one tab only), Logbook, Mail, Chats, Welcome.

# Handover, 2026-11-21 (session F)

Branch `app-rig-relay`, worktree `wt-rig`. Read this, then `docs/NETWORK-STATUS.md`, `docs/HANDOVER-2026-11-18-D.md` (plan WP0-WP13, owner rules) and `docs/MESH-SPEC.md`.

## What happened
Main moved on without this branch (Alloy UI app, messaging layer, new modem ladder, GPL). It was merged in (see "Merge with origin/main" in NETWORK-STATUS). The engine stays ours; the Alloy app is the app now.

## State (checked 2026-11-21)
- `cargo clippy --workspace --all-targets -D warnings`: clean. kahf-core 397, kahf-daemon 106, kahf-app-core 56, kahf-node 9 tests pass; app (own workspace) 35 pass, clippy clean; server 1169 pass; deploy dry run builds.
- Real window of the Alloy app: not looked at by this session. Nothing has run on a real radio.
- Server WP6 and the redesigned site were DEPLOYED on 2026-11-21 (kahf.dev, version 58e80977): `PROJECT_KEY` secret uploaded, migration v3 ran (old backbone data deleted, as the owner approved on 2026-11-19; the owner told session F to do it). Checked live: home 200, /network 200, unsigned /mesh/poll refused with 400.

## Definition of done
TRUE: lanes and 17 m only; transmit gate refuses everything while `plan_verified` is false; lane sensing; bundle chat with delivery states; relay referee (few operators); mesh server code and tests; guides and site; Alloy app shows lanes, gate notice, referee counters.
FALSE / NOT DONE: (ten operators in one cell now works, see NETWORK-STATUS) anything verified on air; the engine paging other relays (WP8); bonded waveform, hopping and diversity on air (WP9); two-relay test; clash-domain colouring; HF fading sweep of lane bursts; a longer soak with real engines (WP12; the server security review is done, see NETWORK-STATUS); main's ladder bursts in the engine; relay slots in the app.
ONLY THE OWNER CAN: set `plan_verified` true after checking the band plan; tag a release; transmit on air.

WP9 note: a two-relay delivery (A to R1 to R2 to B, relays chained by relay hints) works and is tested on virtual-clock buses. `hop.rs`, `diversity.rs` and `harq.rs` exist in kahf-core with unit tests, but the engine does not use hop grants or LLR forwarding (combining weak copies from several relays), and server-planned hops are not used.

## Next
Finish WP8, then WP9, WP10 (look at the Alloy window), WP12. Build one crate at a time, `-j 2` (the owner's Mac lags).

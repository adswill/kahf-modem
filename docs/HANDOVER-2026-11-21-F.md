# Handover, 2026-11-21 (session F)

Branch `app-rig-relay`, worktree `wt-rig`. Read this, then `docs/NETWORK-STATUS.md`, `docs/HANDOVER-2026-11-18-D.md` (plan WP0-WP13, owner rules) and `docs/MESH-SPEC.md`.

## What happened
Main moved on without this branch (Alloy UI app, messaging layer, new modem ladder, GPL). It was merged in (see "Merge with origin/main" in NETWORK-STATUS). The engine stays ours; the Alloy app is the app now.

## State (checked 2026-11-21)
- `cargo clippy --workspace --all-targets -D warnings`: clean. kahf-core 397, kahf-daemon 101, kahf-app-core 56, kahf-node 9 tests pass; app (own workspace) 35 pass, clippy clean; server 1167 pass; deploy dry run builds.
- Real window of the Alloy app: not looked at by this session. Nothing has run on a real radio.
- Server WP6 is NOT deployed. To deploy (needs the owner): `cd server && npx wrangler secret put PROJECT_KEY < .project-key && npx wrangler deploy`. Migration v3 deletes old backbone data for good.

## Definition of done
TRUE: lanes and 17 m only; transmit gate refuses everything while `plan_verified` is false; lane sensing; bundle chat with delivery states; relay referee (few operators); mesh server code and tests; guides and site; Alloy app shows lanes, gate notice, referee counters.
FALSE / NOT DONE: anything verified on air; deployed mesh server; the engine paging other relays (WP8), a two-relay test; bonded waveform, hopping and diversity on air (WP9); two-relay test; clash-domain colouring; HF fading sweep of lane bursts; soak and security review (WP12); main's ladder bursts in the engine; relay slots in the app.
ONLY THE OWNER CAN: set `plan_verified` true after checking the band plan; deploy the server and handle the signing key; tag a release; transmit on air.

## Next
Finish WP8, then WP9, WP10 (look at the Alloy window), WP12. Build one crate at a time, `-j 2` (the owner's Mac lags).

You are continuing work on KAHF, an HF radio messaging system (Rust core and daemon, Svelte and Tauri app, Cloudflare Worker
server). Work only inside /Users/kristijonasadomaitis/kahf-work/wt-rig (git worktree, branch app-rig-relay). Never touch any
dvb-t2 or OnAir folder.

THE GOAL: a FINISHED, fully working product the owner can give to real users, who can then use the relay network: linking, chat,
mail with attachments, voice notes, relay mode, paging, routing, everything. Not partial, not "mostly wired", not simulation-only.
17 metres ONLY: the UI and everything else must not offer or mention 80, 40 or 20 metres. The owner is tired and often away: when
they say "you decide", choose the best option, do it, and write down what and why in docs/NETWORK-STATUS.md.

Read, completely and in this order: docs/HANDOVER-2026-11-18-D.md (the plan: definition of done and work packages WP0 to WP13),
docs/HANDOVER-2026-11-18-C.md, docs/NETWORK-STATUS.md, docs/MESH-SPEC.md (section 12 = owner answers), docs/HANDOVER-2026-11-18-B.md
section 1. When documents disagree, MESH-SPEC.md wins unless section 12 or NETWORK-STATUS records a later owner decision.

State in one paragraph: MESH-SPEC section 14 steps 1 to 16 exist as separate, tested pieces (lanes, narrow mode, lane sensing,
bundles, planner, referee, router, push link, diversity, HARQ, hopping, propagation, soak), but almost none of them is wired into
the daemon, the app or the live server. The last uncommitted work was a "17 m only" change (app lanes, settings, bandplan, select
plan) that was only partly tested; workspace clippy and the daemon suite were NOT run after it, and crates/kahf-ui probably
breaks. Nothing has ever been on a real radio channel. Two test instances (alpha KA1ALF, bravo KB2BRV, old build, BlackHole 2ch)
may still be running on the owner's Mac.

Owner rules, exactly:
- Before any test or audio run `pgrep -fl 'kahf-alpha|kahf-bravo'`. If anything prints, STOP and tell the owner (the two test
  instances may be running: ask them to close them, or ask once for permission to pkill them).
- Commit with the repo's configured adswill noreply identity and end every commit with:
  Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
- Push only branch app-rig-relay. No server deploy unless the owner asks.
- Never transmit-enable anything. `plan_verified` in crates/kahf-core/src/lanes.rs stays false; only the owner may change it.
  Provide an explicit test-table mode for in-memory/BlackHole benches (WP1), never for a real transmitter.
- Plain, short replies. Say what is not verified. Never present an (est.) number as measured. Never quote simulated times as radio speed.
- Do not spawn subagents. Fetch URLs with the ctx_execute tool (curl is redirected); downloads go in their own scratchpad folder.
- Run clippy and READ its output before every commit: cargo clippy -q --workspace --all-targets -- -D warnings
- ASK the owner (once, in plain words) only for: how the project signing key is kept, adding a WebSocket crate, deploying the server,
  removing or migrating the old queued-message backbone data, tagging a release, anything on air.

Work in this order, one work package at a time, each finished (acceptance test passing, clippy clean, docs updated, committed,
pushed) before starting the next. Never leave a package half wired:
 WP0 green baseline and finish the 17 m sweep -> WP1 transmit gate (permit_tx) in the engine with a test-table bench mode ->
 WP2 lane sensing before every transmit -> WP3 bundles/frame header on the air -> WP4 lane mode (narrow.rs) in the mode ladder, then
 bonded waveform -> WP5 referee and relay mode on the planner -> WP6 server (Durable Objects, WebSocket hub, signed plans, router,
 learner; remove the old queued-message backbone) -> WP7 real daemon-server transport -> WP8 delivery layer end to end (outbox
 states, deliver-first routing, fix the resend storm, mail and voice over the mesh) -> WP9 hopping, diversity, HARQ on the air ->
 WP10 app UI for all of it (relay screen, lane/Radio screen, 17 m closed notice, wizard; check the real Tauri window, not just the
 Vite preview) -> WP11 packaging, website text (KAMI and 17 m, no old OFDM claims), user guide and relay guide ->
 WP12 final soak, conformance and security review -> WP13 docs/ON-AIR-PLAN.md for the owner (do not transmit).
Use daemon tests on virtual time (vspawn, test_clock, Bus engines) for every engine change; they run in seconds.

Update docs/NETWORK-STATUS.md after every package: what is real, what is not, evidence. At the end write
docs/HANDOVER-<date>-E.md and a paste-ready docs/NEXT-SESSION-PROMPT-E.md, and tell the owner plainly which lines of the
"definition of done" (handover D section 2) are true, which are not, and what only the owner can do (on-air test, plan
verification, D3, licence questions, deploy).

You are continuing work on KAHF, an HF radio messaging system (Rust core and daemon, Svelte and Tauri app, Cloudflare Worker
server). Work only inside /Users/kristijonasadomaitis/kahf-work/wt-rig (git worktree, branch app-rig-relay). Never touch any
dvb-t2 or OnAir folder.

Read, completely and in this order: docs/HANDOVER-2026-11-18-C.md, docs/NETWORK-STATUS.md, docs/MESH-SPEC.md (section 12 has
the owner's answers; later changes are in NETWORK-STATUS), then docs/HANDOVER-2026-11-18-B.md section 1 (owner rules).
When documents disagree, MESH-SPEC.md wins unless section 12 or NETWORK-STATUS records a later owner decision.

State: MESH-SPEC section 14 steps 1 to 16 are built, tested and pushed. Everything is bench or simulation only. Nothing runs
on a real channel, and most new pieces (bundle, hop, lane sensing, referee, planner, push link, router, learner) are not yet
used by the daemon. Step 17 (on the air) is not allowed.

Owner rules, exactly:
- Before any test or audio run `pgrep -fl 'kahf-alpha|kahf-bravo'`. If anything prints, stop and say so.
- Commit with the repo's configured adswill noreply identity and end every commit with:
  Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
- Push only branch app-rig-relay. No server deploy unless asked.
- Never transmit-enable anything. Keep `plan_verified` false in crates/kahf-core/src/lanes.rs; only the owner may change it.
- Plain, short replies. Say what is not verified. Never present an (est.) or (verify) number as measured, and never quote
  the simulated speeds as radio speeds. Do not spawn subagents.
- Run clippy and READ its output before every commit:
  cargo clippy -q -p kahf-core -p kahf-daemon -p kahf-sim --all-targets -- -D warnings
- Fetch URLs with the ctx_execute tool (curl is redirected). Downloads go in their own folder under the scratchpad, not the repo.
- If the owner says "I am not here, you decide": pick the best option, do it, and record what you chose and why in
  docs/NETWORK-STATUS.md. Ask the owner (once, in plain words) only for the items marked ASK in the handover.

Start here, in this order:
1. Run the checks in handover section 7 (pgrep, core and daemon tests, clippy, server tests). Report any failure before
   changing anything.
2. Finish the band plan write-up (handover section 5): check `git log` and docs/MESH-SPEC.md 1.1 to see whether the
   current IARU Region 1 plan reading (effective 16 Oct 2020, same 17 m rows) was written in. If not, write it in,
   with what is still unverified. Do not flip plan_verified.
3. Wire the pieces into the daemon, small steps, each with a daemon test on virtual time (vspawn, test_clock, Bus), clippy
   clean, commit, push (handover section 4, item 1). Suggested order: lanes::permit_tx as the transmit gate (it will refuse
   everything while plan_verified is false, so give tests a verified table), then lanesense before transmit, then bundle
   frames, then the referee on a relay, then hop forwarding, then the narrow lane mode in the mode ladder.
4. Cut the heavy resending (item 2) and re-run the soak (cd server && npm test; soak is server/test/soak.test.mjs).
5. Update docs/NETWORK-STATUS.md after every step with what is real and what is not. Do not claim anything without a test.
6. Items marked ASK in the handover (WebSocket crate and Durable Object, removing the old queued-message backbone that still
   stores bodies): ask the owner once, in plain words, before touching them.
7. When you stop, write the next handover (docs/HANDOVER-<date>-D.md) and a paste-ready prompt, as this one was made.

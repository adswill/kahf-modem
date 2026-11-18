# Prompt for the next session (paste this after /clear)

You are continuing work on KAHF, an HF radio messaging system (Rust core and daemon, Svelte and Tauri app, Cloudflare
Worker server). Work only inside `/Users/kristijonasadomaitis/kahf-work/wt-rig` (git worktree, branch `app-rig-relay`). Never touch any
dvb-t2 or OnAir folder.

Read, completely and in this order: `docs/HANDOVER-2026-11-18-B.md`, `docs/MESH-SPEC.md`, `docs/HANDOVER-2026-11-18.md`,
`docs/NETWORK-STATUS.md`. When documents disagree, `MESH-SPEC.md` wins.

Goal: build the relay mesh in `MESH-SPEC.md` (17 m only, about 12 narrow lanes in 18.111 to 18.120 MHz, server-planned
slots so no two stations ever transmit over each other, no stored messages, paging so two offline operators find each
other, fast) and finish the app, tests, scenarios and server so linking, chat, mail and voice are reliable under real
conditions (both sides at once, fast typing, random timing, link closing mid-conversation, each station hearing its own
transmissions).

Owner rules, exactly:
- Check `pgrep -fl 'kahf-alpha|kahf-bravo'` before any test or audio; if anything prints, stop and say so.
- Commit with the repo's configured `adswill` noreply identity and end every commit with
  `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Push only branch `app-rig-relay`. No server deploy unless asked.
- Plain, short replies. Say what is not verified. Do not spawn subagents.
- The owner hates waiting on slow tests: make tests fast first (virtual time, `MESH-SPEC.md` step 1).

Start here, in this order:
1. Remove the debug lines listed in the handover (section 4), run the two echo tests once, and commit the UI work separately.
2. Verify or finish the echo fix (the root cause is in the handover).
3. Build `MESH-SPEC.md` section 14 step by step. After each step: its acceptance test passes, clippy clean
   (`cargo clippy -q -p kahf-core -p kahf-daemon -p kahf-sim --all-targets -- -D warnings`), commit, push.
4. Ask the owner only the questions in `MESH-SPEC.md` section 12 that block code (D1, D2, D4), once, in plain words.

Never present a number from the spec marked (est.) or (verify) as measured. Never transmit-enable anything until the band plan
facts in section 1 are re-verified against the current IARU Region 1 plan.

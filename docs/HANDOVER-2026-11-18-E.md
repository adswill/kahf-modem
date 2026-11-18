# Handover E (2026-11-18, session E): WP0 and WP1 done, WP2 to WP13 still open

Read, in order: `docs/HANDOVER-2026-11-18-D.md` (definition of done section 2, work packages section 3, owner rules
section 4, commands section 7), this file, `docs/NETWORK-STATUS.md` (last two sections), `docs/MESH-SPEC.md`.
Same owner rules as D. Branch `app-rig-relay`, worktree `/Users/kristijonasadomaitis/kahf-work/wt-rig`.

## Done this session (committed and pushed)

| Commit | Package | What |
|---|---|---|
| `9531444` | WP0 | Workspace clippy clean (unused import in `select.rs`), `crates/kahf-ui` fixed for 17 m, old 20/40/80 m tables removed from `bandplan.rs`, daemon logbook, kahf-gui, kahf-prop, kahf-log fixtures; SDR default 18.11344 MHz; old design docs marked "background only". All suites green: workspace tests, svelte-check 0, vitest 24, Tauri clippy and tests, server 1173 |
| `b79d64e` | WP1 | Transmit gate in the engine: `lanes::permit_tx` before every burst with the real dial and the modem's audio span; unverified table refuses on a real radio; bench-only test table (`"testTable": true`, only BlackHole pairs or in-memory radios); app wiring and Radio screen notice; 7 daemon tests |

## Not done (next, in order): WP2 to WP13 as listed in handover D section 3

Important facts for WP2 and WP4:
- The engine still sends the wide KAMI (312 to 2690 Hz). With a verified table it is refused as "not inside the lane",
  so **no real transmit is possible until WP4 puts the lane mode (`narrow.rs`) in the engine** and calls
  `Engine::set_tx_audio_span(1260, 1740)` (single lane). That is intended.
- `gate_refusal` is in `kahf-daemon/src/lib.rs`; policy is `Flags.policy` (`TxPolicy`). Test engines start with the test table on
  (`#[cfg(test)]` in `spawn_with_clock`); gate tests use `gate_station(tag, bench)`.
- The owner's bench (`/tmp/kahf-ab`) needs `"testTable": true` and `"freqKhz":18113.44` in both settings files. Not edited (owner's folder).
- Nothing was run in the real Tauri window; nothing has been on a radio; `plan_verified` is false.

## Definition of done (handover D section 2), honestly

True: line 5 for code, settings and the app UI (17 m only; website text and release notes not yet checked, WP11); part of line 4 (gate on every
burst, in the engine; sensing, slots, token and watchdog not yet).
Not true: lines 1, 2, 3, 6, 7, 8 and the rest of 4. Only the owner can: on-air test, set `plan_verified`, D3, licence questions, deploy,
signing key handling, WebSocket crate, removing the old backbone data, tagging a release.

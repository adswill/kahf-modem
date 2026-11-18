# Work board

One shared page that says who is doing what, so two people and their Claude Code agents never
edit the same thing. **Read it before you start. Update it before you start and when you stop.**

## Rules (short)

1. **Claim before you touch.** Add a row to *Active work* with your name, the area or files,
   and your branch. Commit and push that row first (a docs-only commit on `release-0.1`, or a
   pull request if you cannot push). If a row already covers the files, pick something else or
   write to the owner of that row.
2. **One task, one branch, one owner.** Branch name: `<who>/<short-task>`, for example
   `friend/android-ui`. Never commit straight to `release-0.1` or `main` except this board.
3. **Stay in your area.** If you need a change outside the files you claimed, add a row for it
   or ask the owner of those files. Shared files that everyone touches
   (`crates/kahf-daemon/src/lib.rs`, `crates/kahf-core/src/phy.rs`) need a claim for the
   *function or section*, not just the file.
4. **Small, frequent merges.** Open a pull request as soon as one piece works. Pull
   `release-0.1` before you start each task and again before you open the pull request.
5. **Tests must pass before a pull request:**
   `cargo test --workspace --exclude kahf-sdr` and
   `cargo clippy --workspace --exclude kahf-sdr --all-targets -- -D warnings`.
   For the web server: `cd server && npm test`.
   Run the heavy checks on GitHub Actions rather than your laptop when you can.
6. **Agents:** every Claude Code session should be told "read `docs/WORK-BOARD.md` first and
   follow it". The agent writes its own row, and moves it to *Done* when finished.
7. **Do not commit secrets, signing keys, personal files or recordings.** Keep disk use small:
   delete build folders of finished worktrees.
8. **Releases and publishing** are decided by the project owner only.
9. **Licences:** keep notices for third-party code; do not add code with an incompatible
   licence. Patented codecs are never shipped; voice codecs are installed by the user.

## How to claim (copy this row)

| Who | Agent | Area / files | Branch | Status | Started | Notes |
|---|---|---|---|---|---|---|
| name | owner / friend / claude | what you change | `name/task` | in progress | YYYY-MM-DD | anything others must know |

Status values: `planned`, `in progress`, `blocked`, `in review`, `done`.

## Active work

| Who | Agent | Area / files | Branch | Status | Started | Notes |
|---|---|---|---|---|---|---|
| owner | claude | session errors instead of panics, clippy clean (`kahf-core/src/session.rs`, test lints) | `fix-session-clippy` | in review | 2026-11-18 | ready to merge |
| owner | claude | link timers skip transmit time; step-up test (`kahf-daemon/src/lib.rs`: `tick_link`, `decode_frames`) | `fix-step-up` | in review | 2026-11-18 | merge before `kami-mode` |
| owner | claude | KAMI as the main modulation: KAMI-32/16/8 (`kahf-core/src/kami.rs`, `phy.rs`, daemon ladder) | `kami-mode` | in review | 2026-11-18 | based on `fix-step-up` |
| owner | claude | higher-rate KAMI tiers, then LDPC 1296 comparison (`kami.rs`, `kahf-sim`) | `kami-highrate` | planned | 2026-11-18 | needs LDPC frame-loss test, not raw SER |
| owner | claude | receive frequency search (`phy.rs` pilot search) | `rx-pilot-freq` | planned | 2026-11-18 | |
| owner | claude | live map tiles through a Worker proxy (`server/src/web/live-map.js`, `server/src/routes.js`) | `fix-map-tiles` | planned | 2026-11-18 | |

## Free to take (good first areas for a second person)

- Android app and its release build.
- Website (`server/`): pages, wording (HF only), tests.
- Documentation: `docs/USER-GUIDE.md`, `docs/BUILD.md`, screenshots.
- SDR drivers in `crates/kahf-radio` (test with real hardware you own).
- Anything not listed above. Claim it first.

## Done

| Who | Area | Branch / PR | Date |
|---|---|---|---|
| owner | docs contradictions fixed | `fix-docs-contradictions` | 2026-11-18 |
| owner | SDR libraries added to the release workflow | `fix-release-workflow` | 2026-11-18 |

## Conflicts and questions

Write here, with a date, when you hit something that needs the other person. Remove it when
solved.

- (none)

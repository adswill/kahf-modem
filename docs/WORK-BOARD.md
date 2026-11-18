# Work board

One shared page that says who is doing what, so two people and their Claude Code agents never
edit the same thing. **Read it before you start. Update it before you start and when you stop.**

Repo: https://github.com/adswill/kahf-modem (public). Main branch: `release-0.1`.

---

## 1. First-time setup (about 15 minutes)

1. **Get the code.**
   - If you were added as a collaborator: `git clone https://github.com/adswill/kahf-modem.git`
   - Otherwise: fork the repo on GitHub, clone your fork, and open pull requests from it.
2. **Install Rust** (stable): https://rustup.rs
3. **Check it builds and passes** (no radio needed). From the repo folder:
   ```bash
   cargo test --workspace --exclude kahf-sdr
   cargo clippy --workspace --exclude kahf-sdr --all-targets -- -D warnings
   ```
   `kahf-sdr` is left out on purpose: it links the HackRF and RTL-SDR C libraries, which you only
   need if you want to build the full app (see `docs/BUILD.md`).
4. **Web server tests** (optional, only for `server/` work): install Node 20+, then
   ```bash
   cd server && npm test
   ```
   This runs offline. **You do not need a Cloudflare account.** Never run `npm run deploy`, and
   skip `npm run dev` (it wants a Cloudflare login). The owner deploys.
5. **Git identity.** Use your own GitHub name and your GitHub *noreply* address
   (GitHub → Settings → Emails → "Keep my email addresses private"), so your commits show your
   account:
   ```bash
   git config user.name  "your-github-name"
   git config user.email "ID+your-github-name@users.noreply.github.com"
   ```
6. **Start Claude Code** in the repo folder and give it this first message:
   > Read docs/WORK-BOARD.md first and follow it exactly. My name on the board is `<name>`.
   > I want to work on task `<task id>`. Claim it on the board, make a branch, do the work in
   > small steps, run the tests, and open a pull request. Do not touch files owned by someone
   > else's row. Do not ask me questions you can answer yourself.

## 2. Rules

1. **Claim before you touch.** Add a row to *Active work* with your name, the files, and your
   branch. Push that one-line change on its own (a pull request is fine). If a row already
   covers the files, pick another task or write in *Conflicts and questions*.
2. **One task, one branch, one owner.** Branch name `<name>/<short-task>`, for example
   `friend/linux-build`. Never commit straight to `release-0.1`, except edits to this board.
3. **Stay in your files.** Need a change somewhere else? Add a row for it, or ask the owner of
   the row that covers those files. The big shared files (`crates/kahf-daemon/src/lib.rs`,
   `crates/kahf-core/src/phy.rs`) are owned by the modem work. Do not edit them.
4. **Small, frequent pull requests.** Open one as soon as one piece works. Run
   `git pull --rebase origin release-0.1` before you start each task and again before the pull
   request.
5. **Before every pull request** both commands from setup step 3 must pass (and `npm test` if you
   touched `server/`). Put the results in the pull request text. Write what you changed and why in
   plain words.
6. **Agents:** every Claude Code session starts with the message in setup step 6. The agent
   writes its own row and moves it to *Done* when the pull request is merged.
7. **No secrets, signing keys, personal files, recordings or large binaries in git.** Keep disk
   use small: delete `target/` folders of finished work (`cargo clean`).
8. **Releases, publishing, deploys and replies to users** are the project owner's decision only.
9. **Licences:** keep notices for third-party code. Do not add code under an incompatible
   licence (the project is GPL-3 compatible only). Patented voice codecs are never shipped.
10. **Public wording:** README and website say what works now. No roadmap, progress tables or
    "not yet" rows. The website is HF only (VHF is not a goal).
11. **Stuck for more than 30 minutes?** Write it under *Conflicts and questions* with the exact
    error, then switch to another task. Do not wait.

## 3. How to claim (copy this row)

| ID | Who | Agent | Files / area | Branch | Status | Started | Notes |
|---|---|---|---|---|---|---|---|
| F0 | name | friend | what you change | `name/task` | in progress | YYYY-MM-DD | anything others must know |

Status values: `planned`, `in progress`, `blocked`, `in review`, `done`.

## 4. Active work (owner's side, do not touch these files)

| ID | Who | Agent | Files / area | Branch | Status | Started | Notes |
|---|---|---|---|---|---|---|---|
| O7 | owner | claude | automatic KAMI and linking are merged; stress tests on branch `stress-net` (gated by `KAHF_STRESS`) | `stress-net` | in progress | 2026-11-18 | do not touch `kahf-daemon` internals |

## 5. Tasks for the second person, available now

### UI-1. Desktop UI for KAMI status (friend, takes this now)
- **Files:** `crates/kahf-ui/src/` and `crates/kahf-gui/src/` only.
- **Do:** the engine sends `Event::Metrics(LinkStatus)` with the mode in use, whether it is
  automatic, net bit/s, SNR each way, frame loss and counters. Show them in the link panel.
  Add a control for auto mode or a fixed mode using `Engine::set_auto_mode` and
  `Engine::set_fixed_mode`, and a list from `Engine::mode_names()`. Today the events only
  reach a log line.
- **Done when:** the panel shows live values in a linked session, and `cargo test -p kahf-ui`
  and `cargo clippy` pass.
- **Branch:** `friend/ui-kami-status`.

Pick one, claim it (section 3), and go. None needs a radio, a Cloudflare account, or any file
from section 4. Ordered by how self-contained they are.

### F1. More tests for small core modules (any OS)
- **Files:** `crates/kahf-core/src/callsign.rs`, `forms.rs`, `outbox.rs`, `access.rs`,
  `bandplan.rs`, `marker.rs` (only the `#[cfg(test)]` blocks, plus fixes if a test finds a bug).
- **Do:** read each module, list its public functions, and add tests for edge cases: empty and
  very long input, odd characters, boundary values, round trips. If a test exposes a real bug,
  fix the bug in the same pull request and say so.
- **Done when:** new tests pass, clippy is clean, each module gained at least 5 tests.
- **Branch:** `friend/core-tests`.

### F2. Web server tests (needs Node only, no Cloudflare)
- **Files:** `server/test/*.mjs` (new files welcome), data under `server/src/` read-only unless a
  test finds a bug. Do **not** touch `live-map.js` or `routes.js` (row O5).
- **Do:** run `npm test`, then add tests for data validity: band plan frequencies are sorted and
  inside HF, zone data has no duplicate ids, page text contains no VHF claims, every route in
  `server/src/routes.js` has at least one test.
- **Done when:** `npm test` passes, 15 or more new tests, pull request lists what is now covered.
- **Branch:** `friend/server-tests`.

### F3. Windows build support (only if you have a Windows PC)
- **Files:** `crates/kahf-sdr/build.rs`, `docs/BUILD.md`.
- **Do:** the build script only searches macOS and Linux library paths, so linking fails on
  Windows. Teach it Windows locations (vcpkg or an env var such as `HACKRF_LIB_DIR`), make it
  fail with a clear message when a library is missing, and update the Windows rows in
  `docs/BUILD.md` with what you actually ran.
- **Done when:** `cargo build -p kahf-cli` works on your PC and the doc says exactly how.
- **Branch:** `friend/windows-build`. Skip if you have no Windows machine.

### F4. Linux build check (only if you have Linux)
- **Files:** `docs/BUILD.md` only, plus `crates/kahf-sdr/build.rs` if a path is wrong.
- **Do:** follow `docs/BUILD.md` from scratch, write down every package you had to install and
  every step that was wrong or missing, fix the doc, and mark the Linux row as verified only for
  what you ran.
- **Done when:** a fresh Linux install can follow the doc to a working `cargo build -p kahf-cli`.
- **Branch:** `friend/linux-build`.

### F5. User guide accuracy pass (any OS)
- **Files:** `docs/USER-GUIDE.md`, `README.md`.
- **Do:** read the guide next to the code (`crates/kahf-cli`, `crates/kahf-gui`). For every
  statement a user could rely on (command names, flags, menu names, limits), check it is true. Fix
  what is wrong, delete what cannot be checked, keep the wording short and plain. Follow the
  public-wording rule in section 2.
- **Done when:** the pull request lists each statement you changed and what you checked it
  against.
- **Branch:** `friend/guide-check`.

### F6. Radio driver tests without hardware
- **Files:** `crates/kahf-radio/src/drivers/*.rs` (the `#[cfg(test)]` blocks only).
- **Do:** pick one driver, read how it parses replies and handles errors, and add tests with fake
  byte streams: truncated packets, wrong lengths, repeated frames, disconnects mid-read. Fix any
  panic or infinite loop you find. Do not change public function signatures.
- **Done when:** tests pass, clippy is clean, a panic-free summary is in the pull request.
- **Branch:** `friend/driver-tests`.

### F7. Anything else you spot
Claim it first. Good areas: error messages that are unclear, missing doc comments on public
functions, dead code, slow tests.

## 6. Done

| ID | Who | Area | Branch / PR | Date |
|---|---|---|---|---|
| D1 | owner | docs contradictions fixed | `fix-docs-contradictions` | 2026-11-18 |
| D2 | owner | SDR libraries added to the release workflow | `fix-release-workflow` | 2026-11-18 |
| D3 | owner | session errors instead of panics; clippy clean on all targets | merged in `release-0.1` | 2026-11-18 |
| D4 | owner | link timers skip transmit time; step-up test fixed | merged in `release-0.1` | 2026-11-18 |
| D5 | owner | KAMI as main modulation (KAMI-32, 16, 8, 8H), QAM fallback | merged in `release-0.1` | 2026-11-18 |
| D6 | owner | receive frequency search up to 100 Hz | merged in `release-0.1` | 2026-11-18 |
| D7 | owner | live map tiles through a cached proxy, deployed | merged in `release-0.1` | 2026-11-18 |

## 7. Conflicts and questions

Write here, with a date and your name, when you hit something that needs the other person.
Remove it when solved.

- (none)

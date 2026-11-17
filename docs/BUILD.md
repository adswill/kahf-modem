# Building KAHF

This page says how to build the KAHF workspace on each platform, and what has
been checked. A platform is only marked verified when the build was run on it.

## Status at a glance

| Platform | Build steps documented | Verified here |
|---|---|---|
| macOS 14+ arm64 (Apple Silicon) | yes | Yes: macOS 27.2 arm64, `cargo check --workspace` and `cargo build -p kahf-cli` pass |
| macOS 14+ x64 (Intel) | yes | Partly: `cargo check --workspace` passes as a cross check from the arm64 Mac. Not built, not linked, not run |
| Linux x64 | yes | No: not run on a Linux machine. The cross check from the Mac is blocked (see below) |
| Linux ARM64 | yes | No: not run |
| Windows x64 | yes | No: not run |
| Windows ARM64 | yes | No: not run |

Nothing has been run on a real Intel Mac, Linux, or Windows machine. Hardware
(HackRF, RTL-SDR, sound cards) has not been tested in this build check either.

## Rust toolchain

- The workspace uses edition 2021 and has no `rust-version` field. It has been
  built with Rust 1.98.1 (Homebrew). Use a recent stable toolchain from rustup.
- Install with rustup: `curl https://sh.rustup.rs -sSf | sh` (macOS, Linux) or
  the installer from rustup.rs (Windows).

## Native libraries

`crates/kahf-sdr/build.rs` links two C libraries on every platform, even when
no radio is attached:

- `hackrf` (libhackrf)
- `rtlsdr` (librtlsdr)

Because of this, `cargo build -p kahf-cli` needs both libraries installed. The
build script looks for them in `/opt/homebrew/lib`, `/usr/local/lib` and
`/usr/lib`. On Windows those paths do not exist, so the link step will fail
until the build script learns about Windows library locations. This is a known
gap. `cargo check` does not link, so it does not need the libraries.

Other native parts:

- `cpal` (sound card access, used by `kahf-audio`). It uses CoreAudio on
  macOS, ALSA on Linux (needs the ALSA development package) and WASAPI on
  Windows.
- `zstd-sys` (compression). It builds a bundled C library, so a C compiler is
  needed for the target platform.
- The GUI crates (`kahf-gui`, `kahf-ui`, built on iced) pull in windowing and
  graphics crates. On Linux they may need Wayland or X11 development packages.
  This is expected from the crate set, not confirmed by a Linux build.

## macOS 14+ (arm64 and Intel)

Packages:

```bash
xcode-select --install                 # C compiler and linker
brew install hackrf librtlsdr          # libusb comes in as a dependency
```

Verified on this Mac (macOS 27.2, arm64, Homebrew): hackrf 2026.01.3,
librtlsdr 2.0.3, libusb 1.0.30.

Build:

```bash
cargo build --workspace
cargo test --workspace
scripts/build-check.sh                 # check + kahf-cli build, with a summary
```

Notes:

- Intel Macs use Homebrew under `/usr/local`. The build script searches both
  `/opt/homebrew/lib` and `/usr/local/lib`, so the link path should be found.
  This is not tested on an Intel Mac.
- The minimum macOS version is 14. This Mac runs 27.2. Nothing has been run on
  macOS 14.
- Cross check from the arm64 Mac to Intel: rustup's stable toolchain has the
  `x86_64-apple-darwin` standard library. The Homebrew Rust in `/opt/homebrew/bin`
  does not, so put the rustup toolchain first on `PATH` for cross checks.

## Linux (x64 and ARM64)

Packages, Debian or Ubuntu (package names may differ on other distributions):

```bash
sudo apt install build-essential pkg-config libasound2-dev libusb-1.0-0-dev \
    libhackrf-dev librtlsdr-dev
```

The GUI crates may also need the windowing development packages, for example
`libxkbcommon-dev`, `libwayland-dev`, `libx11-dev`. These are not confirmed.

Build:

```bash
cargo build --workspace
scripts/build-check.sh
```

Status:

- Not built on any Linux machine. Documented only.
- A cross check from the Mac to `x86_64-unknown-linux-gnu` was attempted. It
  failed in `zstd-sys` because the Mac has no Linux C cross compiler. This is an
  environment limit, not a compile error in KAHF code, but it means the Linux
  target is not verified by this check.
- ARM64 Linux (`aarch64-unknown-linux-gnu`) is not installed here and was not
  checked.

## Windows (x64 and ARM64)

Packages:

- Visual Studio Build Tools with the "Desktop development with C++" workload
  (MSVC compiler and linker).
- Rust from rustup with the MSVC toolchain (the default on Windows).
- Targets, if you build for ARM64 as well:
  `rustup target add aarch64-pc-windows-msvc` and
  `rustup target add x86_64-pc-windows-msvc`.
- hackrf and rtlsdr libraries for Windows. There is no build-script support for
  them yet, so the link step will not find them without changes. Not done.

Build (PowerShell, from the repo folder):

```powershell
cargo check --workspace
.\scripts\build-check.ps1
```

Status:

- Not built, checked or run on Windows. Documented only.
- The toolchain `x86_64-pc-windows-gnu` is installed on the Mac. It was not
  used, because Windows builds were out of scope for this check.
- `scripts/build-check.ps1` has not been run. It has not been parsed by
  PowerShell either, because PowerShell is not installed on the Mac.

## Build check script

`scripts/build-check.sh` (macOS and Linux) runs:

1. `cargo check --workspace`
2. `cargo build -p kahf-cli`

It warns when hackrf or rtlsdr is not found, prints a pass or fail line per
step, and keeps full logs in a temporary folder. It exits with status 1 when any
step fails.

`scripts/build-check.ps1` (Windows) runs the same two steps.

## Not checked

- Intel Mac build and run
- macOS 14 and 15 behaviour
- Linux x64 and ARM64 (all steps)
- Windows x64 and ARM64 (all steps)
- Linking with real HackRF or RTL-SDR hardware
- Audio device output on any platform other than this Mac

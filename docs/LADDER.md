# Speed ladder

`kahf_core::burst` sends 1 to 32 codewords (36 payload bytes each, own CRC) in one OFDM burst.
Rate comes from the modulation, puncturing of the rate-1/2 LDPC code, and repetition with soft
combining. Mode and codeword count are agreed by the link layer. `burst::pick(snr_db, margin_db)`
chooses the fastest mode for a measured SNR.

Measured with `ladder_bench` (`cargo run --release -p kahf-sim --example ladder_bench -- 24 <awgn|good|moderate|poor> -12 36 3`;
curves in `ladder-curves.csv`). SNR is in a 2500 Hz noise bandwidth. Cells give the lowest tested SNR
(3 dB steps) with under 10 percent codeword loss, and the peak goodput in bit/s of a full burst.
A dash means no tested SNR up to 36 dB reached 10 percent. Goodput counts payload only; the one-off
preamble is included, turnaround and ARQ are not.

| Mode | Modulation, code | AWGN | Good | Moderate | Poor |
|---|---|---|---|---|---|
| R32 | QPSK 1/2, x32 | -9 dB / 41 | -9 / 41 | -9 / 41 | -9 / 41 |
| R16 | QPSK 1/2, x16 | -9 / 78 | -9 / 78 | -6 / 78 | -6 / 78 |
| R8 | QPSK 1/2, x8 | -6 / 140 | -6 / 140 | -3 / 140 | -3 / 140 |
| R4 | QPSK 1/2, x4 | -3 / 228 | -3 / 228 | 0 / 228 | 0 / 228 |
| Q12 | QPSK 1/2 | 3 / 1326 | 6 / 1326 | 6 / 1326 | 6 / 1326 |
| Q23 | QPSK 2/3 | 6 / 1717 | 9 / 1717 | 9 / 1717 | 9 / 1717 |
| S12 | 16QAM 1/2 | 9 / 2483 | 9 / 2483 | 12 / 2483 | 12 / 2483 |
| S23 | 16QAM 2/3 | 12 / 3156 | 15 / 3156 | 15 / 3156 | 18 / 3156 |
| T12 | 64QAM 1/2 | 12 / 3502 | 15 / 3502 | 15 / 3502 | 18 / 3497 |
| T34 | 64QAM 3/4 | 18 / 4820 | 21 / 4820 | 27 / 4738 | - / 2918 |
| T56 | 64QAM 5/6 | 21 / 5213 | 24 / 5213 | - / 4629 | - / 1622 |

Fading channels are the ITU-R F.1487 Watterson set (see `BENCHMARK.md`). Use a 3 dB margin in
`pick` on a fading path. The long bursts of the fastest modes (about 2 s) are what fails first on the
moderate and poor channels; shorter bursts (fewer codewords) trade rate for robustness.

## How it works

- **Weak-signal sync.** A 0.26 s noise-like preamble is found by FFT cross-correlation at a grid of
  carrier offsets (+-15 Hz, 1.5 Hz step), giving timing and CFO with about 33 dB of processing gain.
  The old one-symbol sync stopped working near +4 dB.
- **Channel estimate.** All pilots of every data symbol refine the estimate (known symbols, so no
  decision errors at low SNR). A phase tracker with drift prediction keeps long frames from wrapping.
  The impulse-response model is 40 taps (was 24): equal-strength echoes 16 samples apart used to fail.
- **Fading.** Each codeword is decoded with a static estimate first, then with estimates tracked over
  7 and 3 symbols; the first CRC pass wins. This removed the 10 to 60 percent error floors on
  fading channels.
- **Rate.** Puncturing and repetition are done on the one rate-1/2 802.11 code, so there is one
  encoder and one decoder.

## In the daemon

Link transmissions go out as ladder bursts (`Flags::set_hf_burst`, default on; off sends the old single
frames). A link starts at Q12 and moves one rung per peer report: up under 2 percent frame loss, down at
8 percent, two rungs at 30 percent. A connected link never goes below R4, because slower bursts outlast
the 2.5 s link timeouts. Weak mode (`set_weak_mode`) sends every frame as an R32 burst (about 7 s each,
about -9 dB) instead of eight copies.

There is no header. Each codeword's id byte (covered by its CRC) holds a flag, the rung, the frame kind
(`FrameKind`: link, beacon, receipt, relay; the daemon routes non-link bursts to `on_unlinked_frame`, which
the messaging layer fills in) and short or long burst (1 codeword, or 5 for Q12, 4 for Q23, 8 for 16QAM, 16 for 64QAM). The receiver finds the
preamble once, then tries every (rung, size) whose audio has arrived, and takes the first whose codewords
pass their CRC and carry the matching id. It scans the last 16 s of audio every 1 s while a link is active
and every 4 s otherwise, about 0.2 s of CPU per scan. The ladder is on top of the old receiver, which still
runs: old single frames are still heard.

## Receive cost and the R4 floor

`burst::scan` tries every (rung, size) guess the audio allows. A wrong guess used to cost a full
demodulation with tracked channel estimates and a failed 50-iteration decode, so a scan took 0.9 to
1.8 s whenever the sender sat on another rung than the receiver expected (`scan_cost` example). The
receiver scans every second while a link is live, so it fell behind and dropped audio. Now every
guess is tried first with the static estimate and 10 decoder iterations, and only when none decodes
does the thorough pass run: average 0.12 s, worst 0.3 s, same decodes. R4 bursts are also 1.26 s
long, so they collide more often and wait longer for acknowledgements than Q12; that is link
behaviour, not modem loss.

Probe for real audio: `cargo run --release -p kahf-daemon --example r4_probe -- "BlackHole 2ch" R4 R4 5`
(rung of each station as arguments, also mixed such as `R4 Q12`). Simulated two-engine tests:
`both_stations_on_r4_talk_over_a_shared_bus`, `stations_on_different_rungs_still_hear_each_other`.

## Not yet done

- Reach is -9 dB (about 40 bit/s), not FT8's -21 dB. A lower-rate mother code is needed for that.
- Chase combining of retransmitted frames (the receiver keeps the soft bits of a failed codeword).
- Mode choice from a measured SNR (today it follows the peer's frame loss report, one rung at a time).

## Regulatory note (17 m, US)

Per-carrier symbol rate is 8000/576 = 13.9 baud. The aggregate over 114 data carriers is about 1.6 kBd,
inside an occupied band of 312 to 2688 Hz (2.4 kHz). Since the FCC's 2024 rule change (97.307(f)),
the 300 baud ceiling for HF data is replaced by a 2.8 kHz bandwidth limit, which every mode meets.
Under the older reading, per-carrier baud is the figure that counts and is also far below 300.
Station identification still has to be sent; the daemon's callsign beacon covers it.

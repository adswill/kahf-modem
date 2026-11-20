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

## Not yet done

- Reach is -9 dB (about 40 bit/s), not FT8's -21 dB. A lower-rate mother code is needed for that.
- The link layer still sends single 36 byte QPSK frames. Wiring bursts, mode choice from measured SNR
  and chase combining of retransmissions into the daemon is the next step.
- A header that announces mode and codeword count (today both ends must be told).

## Regulatory note (17 m, US)

Per-carrier symbol rate is 8000/576 = 13.9 baud. The aggregate over 114 data carriers is about 1.6 kBd,
inside an occupied band of 312 to 2688 Hz (2.4 kHz). Since the FCC's 2024 rule change (97.307(f)),
the 300 baud ceiling for HF data is replaced by a 2.8 kHz bandwidth limit, which every mode meets.
Under the older reading, per-carrier baud is the figure that counts and is also far below 300.
Station identification still has to be sent; the daemon's callsign beacon covers it.

# HF channel benchmark

`cargo run --release -p kahf-sim --example hf_bench -- [frames] [from] [to] [step]` prints CSV
(`channel,mod,snr_2500,fer,goodput_bps`). Raw results are in `benchmark-baseline.csv`.

- Channels: AWGN and the ITU-R F.1487 / CCIR 520 Watterson set (two equal-power Rayleigh
  paths, Gaussian Doppler): good 0.5 ms / 0.1 Hz, moderate 1 ms / 0.5 Hz, poor 2 ms / 1 Hz,
  flutter 0.5 ms / 10 Hz. Also selectable in `kahf-cli e2e --profile watterson-*`.
- SNR is quoted in a 2500 Hz noise bandwidth, the reference used for FT8, JS8, VARA and ARDOP
  figures. The sim's per-sample SNR is 2.04 dB lower.
- Frame: 36 byte payload, LDPC rate 1/2, one codeword per frame, silence around it. Goodput is
  payload bits per second of frame airtime (no ARQ, no inter-frame gap).
- 200 frames per point, so a FER of 0 means under about 1.5 percent.

## Baseline (before the DSP work), QPSK / 16QAM / 64QAM frame error rate

| SNR (dB, 2500 Hz) | AWGN | Good | Moderate | Poor | Flutter |
|---:|---|---|---|---|---|
| 0 | .99 / 1 / 1 | 1 / 1 / 1 | 1 / 1 / 1 | 1 / 1 / 1 | 1 / 1 / 1 |
| 3 | .20 / 1 / 1 | .47 / 1 / 1 | .58 / 1 / 1 | .78 / 1 / 1 | 1 / 1 / 1 |
| 6 | .10 / .39 / 1 | .12 / .90 / 1 | .32 / .95 / 1 | .51 / .95 / 1 | 1 / 1 / 1 |
| 9 | .02 / .03 / 1 | .01 / .37 / 1 | .21 / .48 / 1 | .38 / .75 / 1 | .99 / 1 / 1 |
| 12 | .01 / 0 / .01 | .01 / .20 / .67 | .16 / .38 / .76 | .31 / .56 / .92 | .98 / 1 / 1 |
| 15 | .01 / 0 / .01 | 0 / .14 / .42 | .15 / .33 / .55 | .29 / .50 / .77 | .98 / 1 / 1 |
| 30 | 0 / 0 / 0 | 0 / .09 / .33 | .13 / .31 / .45 | .28 / .45 / .62 | .98 / 1 / 1 |

Peak goodput: QPSK 800, 16QAM 1000, 64QAM 1333 bit/s (AWGN).

## What the baseline shows

1. Error floors on every fading channel: raising SNR does not help. The frame is 0.36 s long and one
   codeword, so a single fade takes it out. Needs time diversity (chase combining across
   retransmissions, longer interleaving) and not more power.
2. Flutter (10 Hz Doppler spread) fails completely. The channel estimate is static per frame, and
   the data symbols are not tracked.
3. Peak speed is limited by overhead (two sync symbols per 36 byte frame) and a single rate-1/2 code.
4. Weak-signal reach is about +4 dB (AWGN, QPSK, 10 percent FER) against -21 dB for FT8. Below about 0 dB the frame is not even found: the sync metric spans one OFDM symbol. There is no robust low-rate mode.

## Published reference points (vendor and project figures, not measured here)

FT8 decodes to about -21 dB in 2500 Hz at roughly 6 bit/s. JS8 normal about -24 dB at roughly 8 bit/s.
VARA HF claims up to about 5.6 kbit/s at 2300 Hz in good conditions and decodes to about -10 dB
in its slowest level. ARDOP tops out near 2 kbit/s. Compare against these only for orders of magnitude.

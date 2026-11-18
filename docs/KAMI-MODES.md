# KAMI modes

All modes share one channel: complex chips at 2000 chips/s, root-raised-cosine pulses (roll-off
0.2) on a 1500 Hz audio carrier, so every mode occupies 300 to 2700 Hz. The number in the name
is the Walsh word length in chips: the bigger the number, the lower the rate and the lower the
SNR needed. KAMI-1a to 1d send one chip per symbol with 2, 4, 8 or 16 amplitude levels on each
of the in-phase and quadrature branches.

Measured on simulated white noise, 100 frames per point, LDPC (648, 324) with 50 iterations.
SNR is per real audio sample, in the 4 kHz audio band. "Net" counts the 324 information bits
of each frame over its air time, including the preamble and pilots. Frames sent in one burst
share a single preamble (up to 8 per burst).

| Mode | SNR for under 5% lost frames (dB) | Raw coded bit/s | Net info bit/s, 1 frame | Net info bit/s, burst of 8 | Net info bit/s, burst of 16 |
|---|---:|---:|---:|---:|---:|
| KAMI-256 | -16.0 | 125 | 40 | 41 | 41 |
| KAMI-128 | -13.2 | 219 | 75 | 80 | 80 |
| KAMI-64 | -10.5 | 375 | 144 | 161 | 163 |
| KAMI-32 | -8.1 | 625 | 235 | 284 | 288 |
| KAMI-16 | -5.3 | 1000 | 347 | 462 | 473 |
| KAMI-8 | -2.3 | 1500 | 462 | 687 | 712 |
| KAMI-4 | -1.8 | 2000 | 546 | 895 | 937 |
| KAMI-2 | -0.6 | 2000 | 546 | 895 | 937 |
| KAMI-8H | -0.2 | 2500 | 612 | 1090 | 1155 |
| KAMI-1a | -0.2 | 4000 | 1074 | 1764 | 1850 |
| KAMI-1b | +4.8 | 8000 | 2056 | 3439 | 3606 |
| KAMI-1c | +8.9 | 12000 | 2500 | 4931 | 5281 |
| KAMI-1d | +13.0 | 16000 | 2755 | 6033 | 6599 |

The app chooses from KAMI-256 up to KAMI-1d. KAMI-2 and KAMI-8H still decode but are never
chosen: KAMI-4 and KAMI-1a are faster at the same SNR. The ladder floors in
`crates/kahf-core/src/phy.rs` are each mode's 5 percent point plus 1 dB.

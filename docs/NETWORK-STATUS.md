# Network: what is built, what is not (2026-11-18)

Built and tested (the numbers are from the tests, not promises):

| Piece | Where | Evidence |
|---|---|---|
| Listen before talk, back-off, answer-first timing, train and share limits | `kahf-core/src/mac.rs`, used by the daemon for every transmit | 7 unit tests; daemon tests |
| BUSY reply with a wait time, link idle time-out, take-over of a quiet link | `kahf-core/src/link.rs` | simulator |
| Many-station simulator (collisions, hidden stations, fading) running the real link and MAC | `kahf-core/src/sim.rs` | ten stations to one hub: 95 percent or more delivered (was 30); two starting together: all; hidden stations: 39 of 40 runs; 30 percent fading: nearly all |
| One burst for all answers, up to 8 frames of one send in one burst, no duplicate queued frames | daemon | live two-engine test over BlackHole: a 2.7 kB mail arrives in 27 s (before: not in 100 s); overlapping transmit time 11.7 s down to 2.9 s |
| A burst counts as received only once its first codeword decodes | daemon | removed an eight second silence after a false start |
| Announce myself (with location by the person's choice), relay beacon with time and load | link, daemon, app | unit tests |
| Files direct only, 100 kB limit, time estimate in the app | daemon, app | unit tests |
| Server backbone: heard, submit (first id wins), pull, receipt, wanted; worldwide slot planning | `server/src/backbone.js`, deployed to kahf.dev | 1139 server tests; client round trip through a local copy of the server code |
| Relay carries messages for stations it cannot hear through the server, delivers the ones the server holds for stations it hears, sends receipts back as a short message | `kahf-daemon/src/backbone.rs`, daemon | unit tests; `backbone_round_trip_through_a_local_server` (needs `node server/tools/serve-local.mjs`) |

## What the HF channel tests showed (new, and the most important)

`cargo run --release -p kahf-sim --example hf_sweep` runs every mode through the ITU-R F.1487 HF
channels (two Rayleigh paths with Doppler spread; good 0.5 ms and 0.1 Hz, moderate 1 ms and 0.5 Hz,
poor 2 ms and 1 Hz). SNR in dB for 10 percent frame loss (">30" means never reached):

| mode | plain noise | HF good | HF moderate | HF poor |
|---|---:|---:|---:|---:|
| KAMI-256, 64, 32 | -16, -10, -8 | >30 | >30 | >30 |
| KAMI-16, 8 | -4 | 0 | >30 | >30 |
| KAMI-4 | -2 | 2 | >30 | >30 |
| KAMI-1a | 0 | 10 | 6 | >30 |
| KAMI-1b, 1c, 1d | 6, 10, 12 | >30 | >30 | >30 |

1. **Before this work KAMI could not take any echo.** Two equal paths 0.5 to 2 ms apart (ordinary HF) lost
   72 to 82 percent of KAMI-32 frames at 30 dB SNR. A preamble-trained MMSE equalizer fixed that (0 to 2
   percent lost); on a channel without echoes it does nothing, and plain-noise results are unchanged.
2. **The slow modes are the fragile ones on HF.** Their bursts last 2 to 8 s and the channel changes during a
   burst, so the single channel estimate from the preamble goes stale. KAMI-256/64/32 never reach 10 percent
   loss on any HF channel; KAMI-16 to KAMI-1a do on "good" and "moderate". The link now never falls below
   KAMI-16 while the signal is not weak (-3 dB or better).
   Frame loss at fixed SNR (`hf_floor` example), so the shape is visible: on "good", KAMI-32 sits at a
   12 to 15 percent floor at any SNR (deep fades: ARQ repeats these) while KAMI-256 loses 82 to 85 percent
   (a wall); on "moderate", KAMI-32 and KAMI-16 lose 51 to 76 percent at any SNR. A loss that does not
   fall as SNR rises is the channel changing inside the frame, not noise.
3. **Not solved:** the poor channel (1 Hz Doppler) for everything, and the slow modes everywhere. The fix is
   tracking the channel through a burst: short known training blocks inside the burst, or decision-directed
   re-estimation. That changes the frame format and is the next PHY task. Until it is done, quote HF
   performance from this table, not from the plain-noise table in docs/KAMI-MODES.md.

Not built yet (designs are in NETWORK-DESIGN.md, NETWORK-SCENARIOS.md, NETWORK-CHANNELS.md):

- Relay as traffic controller: request window, grants (the last hidden-station cases), virtual carrier sense.
- Three channels: C0, D1, D2, moving a long transfer to a data channel, return-by timers.
- Server-planned HF time slots used by the relay (the server plans them; the relay still uses one 24 h slot).
- Message ids on the radio side (relays derive one from the content and the minute).
- Mailbox codes in the beacon (a station hearing its own code asks for its mail). Today the relay delivers
  waiting mail when it hears the station (any frame, an advert, a beacon answer).
- Delivery states beyond what exists, a channel busy strip, queue position in the app.
- Channel tracking inside a burst (see above), then a re-measured mode table and ladder.
- Any test on a real HF channel. (Transmit level: a setting with a 50 percent default and a 5 second test tone is in; automatic drive control is not.)
- Relay mode needs the area shared (location not off) to register; without it the relay is radio only.

## Mesh build (MESH-SPEC.md section 14), as of 2026-11-18

| Step | State | Evidence |
|---|---|---|
| 1 Virtual time | built | `kahf-daemon/src/clock.rs`; the daemon suite (70 tests) runs in about 3 s |
| 2 Echo bug | fixed | both echo tests pass; four engine bugs found with virtual time (see the commit message) |
| 3 Band table and lanes | built | `kahf-core/src/lanes.rs`, 10 unit tests; every number of the spec table checked; the built-in table is **not verified** against the current IARU plan, so `permit_tx` refuses everything |
| 4 Narrow lane mode | built, first version | `kahf-core/src/narrow.rs`: KAMI at one fifth of the chip rate (400 chips/s). Tests: frames decode at three lane centres in two modes; signal below -26 dB outside +-240 Hz. `cargo run --release -p kahf-sim --example lane_leak`: two neighbours 30 dB stronger cost 0 to 2 dB at 700 Hz pitch (KAMI-1c -1, 1a +2, 8 0; 30 frames per point, 1 dB steps, so +-1 dB); 800 Hz pitch gives the same, so the pitch stays 700 Hz (12 lanes) |
| 5 to 17 | not started | |

Measured on the test bench only. Nothing here has been on a real channel.

Step 4 limits: ideal channel and ideal transmitter only (no radio non-linearity, no drift between neighbours, no fading). The needed SNR is on the whole 8 kHz audio, not per lane. The speeds in MESH-SPEC 2.4 are still estimates; no speed was measured. The lane mode is not yet in the engine or the mode ladder.

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

Not built yet (designs are in NETWORK-DESIGN.md, NETWORK-SCENARIOS.md, NETWORK-CHANNELS.md):

- Relay as traffic controller: request window, grants (the last hidden-station cases), virtual carrier sense.
- Three channels: C0, D1, D2, moving a long transfer to a data channel, return-by timers.
- Server-planned HF time slots used by the relay (the server plans them; the relay still uses one 24 h slot).
- Message ids on the radio side (relays derive one from the content and the minute).
- Mailbox codes in the beacon (a station hearing its own code asks for its mail). Today the relay delivers
  waiting mail when it hears the station (any frame, an advert, a beacon answer).
- Delivery states beyond what exists, a channel busy strip, queue position in the app.
- A Watterson HF channel model in the simulator (kahf-sim has channel models to reuse).
- Transmit level safeguards, and any test on a real HF channel.
- Relay mode needs the area shared (location not off) to register; without it the relay is radio only.

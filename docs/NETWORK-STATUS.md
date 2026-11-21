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
| 5 Lane sensing | built, bench only | `kahf-core/src/lanesense.rs`, 7 tests: per-lane floor, +6 dB rule (+4 dB ignored, +8.5 dB busy), neighbours checked, synthetic voice, FT8-like (15 s cycle), carrier, CW and a KAHF lane frame recognised, quarantine 10 min / 30 min / 2 h, voice blocks lane and neighbours 20 min. Synthetic signals only; never heard a real band. Not yet wired into the engine, and the one-minute whole-segment report to the server is not built |
| 6 Frame header and bundles | built, not on the air | `kahf-core/src/bundle.rs`, 8 tests: 9 byte header, bundle of 5 recipients read by each and ignored by others, answers in listed order 300 ms apart, ack bitmap, lost codeword loses only its item. Not yet sent or received by the engine; HARQ and early abort (8.5) not built |
| 7 Planner | built, bench only | `kahf-core/src/planner.rs` and `server/src/planner.js` (identical plans, checked on three plans). 30 relays x 300 epochs random queues: no (lane, slot) shared, no relay twice in a slot, idle relays hold nothing, every cell used under demand; a busy relay among idle ones keeps one lane for all 6 slots; 30 busy relays all served; 70 relays rotate; server outage of 60 s: 4 epochs from signed plans, then the local plan at one quarter duty with no clash, then back to server plans. One clash domain only (all relays treated as in conflict); no domain graph or colouring yet; signing and publishing wait for step 10; not in the daemon |
| 8 Relay referee | logic and slot-level simulation only | `kahf-core/src/referee.rs`, 5 tests using the real frame encoding: beacon names a 2 to 4 slot window, requests in random slots, collisions lost, grants in one burst, data only in granted slots, backoff doubling to 8. 10 operators + 1 relay, 3 messages each, 20 seeds: all delivered, no two operators ever in the same data slot, operators only in window slots; also with 15 percent missed relay bursts. Operators cannot hear each other in the model. Not in the daemon; no radio, no real timing, no channel errors on the frames themselves; one relay, grants always on one relay lane |
| 9 Routing and presence | built, in memory, not connected | `server/src/router.js`, 9 tests: hashed presence (6 h), route with score = P x freshness / (1 + queue/capacity), warm second choice, page-first when not heard in the last 2 min, paging in the first 20 s of every 10th minute with one cell per relay, the first relay that hears the answer ends it, not reachable after 3 windows (about 30 min), sender retry schedule (10 min doubling to 1 h, 24 h lifetime), duplicate hashes kept 2 h. Both-offline cases (known, stale, unknown, not reachable, retry) are tested with a simulated clock only; the routes, the Durable Object and the relay side are not built; P comes from a constant until the learner (step 14); the older queued-message backbone still exists and still stores messages |
| 10 Push link | client logic and hub logic only | `kahf-daemon/src/push.rs` (5 tests, virtual clock, fake hub with 40 ms delay) and `server/src/hub.js` (4 tests). A pushed delivery reached the other relay in 40 ms of model time (limit 100 ms) because nothing polls; heartbeat every 30 s; one socket per key (the second kicks the first); socket down: poll every 10 s, reconnect 2 s doubling to 30 s; items held 60 s in RAM only; hub dead 70 s survived, a 70 s old item dropped. **Not built:** the real WebSocket and HTTPS transports (no WebSocket crate in the daemon yet), the Durable Object and wrangler binding, wiring into `backbone.rs` (still polls), signing of the hello. The 40 ms is the fake link's own delay, so it shows there is no poll wait, not real internet latency |
| 11 No-storage test | built, with a known exception | `server/test/nostorage.test.mjs`: the mesh modules (router, hub, planner) import only node:crypto and name no storage; a message run moves only ids, sizes and hashes; the hub refuses any message with a body, text, subject, payload, content, data or message field. Daemon: after a relayed message, no file in the relay's data folder contains the text (and the same scan is shown to find a body in a sender's outbox). **Known exception:** the older queued-message backbone (`server/src/backbone.js`, deployed, with `MAILBOXES` in wrangler.toml) still stores message bodies in Durable Objects, against the no-storage rule; it is not covered by the test and has not been removed or replaced. The relay scan is plain bytes only and covers one scenario (one relayed chat message) |
| 12 Receive diversity | built, bench only | `kahf-core/src/diversity.rs`, 3 tests: 4 bit LLR packing (324 bytes per codeword), combiner adds the soft bits and decodes. KAMI-8 at -6.5 dB per sample in plain noise, three independent relays, 24 frames: **0 of 72** single looks decoded; **15 of 24** frames that failed at all three relays were decoded by the sum. Independent Gaussian noise only (no fading, no correlated interference, no timing offset between relays); the frame key and the relay-to-server transport are not built; relays are not yet told to forward; the gain is not measured on any real channel |
| 13 HARQ and lane bonding | HARQ built and in the daemon decode (bench only); bonding is planning only | `kahf-core/src/harq.rs`: a failed codeword's soft bits are kept, a repeat is tried alone first, then added to the kept ones; wrong mode or older than 60 s starts over; an unrelated frame is never spoiled. KAMI-8 at -7.5 dB per sample, plain noise: 0 of 24 frames decode within four fresh looks, 17 of 24 within four combined looks. Used in the daemon's `finish_kami` (all 77 daemon tests still pass) but **no end-to-end daemon test exercises a combined decode**, and `link.rs` is untouched. Combining is keyed by codeword position in the burst, so a repeat with a different frame order simply fails to combine. `planner.rs` `bond`/`bond_count`: runs of 1 to 4 adjacent physical lanes (spares included, never the calling lanes), never over another relay's lane or each other's, width at most 2580 Hz and accepted by `permit_tx`; the eligibility thresholds are guesses. **Not built:** the wide waveform for 2 to 4 lanes (`narrow.rs` makes one lane only), the relay using a bonded grant, a JS twin of `bond`, the speed gains (still estimates) |
| 14 Propagation learner, solar inputs, predictive paging | built, in memory, not connected | `server/src/propagation.js` (8 tests in `propagation.test.mjs`): great circle, midpoint, sun elevation (checked to about 3 degrees at a few points), a first-guess model (skip zone under 700 km, 1000 to 4000 km best, day better than night, flux up, storms down: **all weights are guesses (est.)**), a learner per place pair and hour of day that replaces the model by measurements (prior worth 4 observations, forgets over about 7 days), test beacon rule (few samples, under 1 percent air time), NOAA parsing (the JSON shapes were read from the live service on 2026-11-18; the fetch is injectable and was only run against a fake), a per-operator listening pattern, and `Router.page(dest, now, {pattern})` that pages in the learned hours. **Not built:** the hourly fetch job and storage of the learner (it is RAM only, so a restart forgets everything), feeding relay `heard` reports and grant results into `observe`, mapping callsigns to places, the clash-domain graph from the footprint, wiring into the Durable Object. Nothing is checked against real propagation |
| 15 Hopping | logic and simulation only | `kahf-core/src/hop.rs` (6 tests) and `planner::hop_grants` (2 tests). Hop frame = bundle message frame with a relay as next hop, body: origin(3) message id(2) destination(3) payload(19). Rules: only the named next hop acts; delivered if it is the destination; forwarded if a route exists and the frame has made fewer than 4 transmissions; dropped as a loop if the origin is itself or the (origin, id) was seen in the last 10 min; passive acknowledgement = hearing the next hop forward it with the hop count one higher; no ack in 30 s reports the hop failed (the message then takes the internet way). Simulated: a line of 5 relays delivers (4 transmissions), 6 relays do not; a ring stops after one lap; a lost hop expires. A HOP grant uses one of the sender's own planned cells in a slot where the target holds no cell and is not already receiving. **Not built:** sending or receiving hop frames in the daemon, the target tuning to the sender's lane, publishing HOP grants in signed plans (the JS planner has no HOP), soft-combining across hops, the reach-out booking for long paths. No radio, no real timing |
| 16 Soak | built; logic only, the air is a model | `server/test/soak-sim.mjs` and `soak.test.mjs` (6 tests, about 7 s): a simulated week with the real router, hub, learner, listening pattern, retry schedule and paging; 8 relays, 14 operators, 660 messages per run. The air is the propagation model (sun, distance, flux) with a 0.9 factor, plus night closure, 20 minute voice interferers, relay crashes of 1 to 6 hours that wipe the relay's RAM, relay RAM of 30 messages for 60 s, and a flood of 400 messages from one operator in 10 minutes. **Results, 3 seeds:** the user never saw a message twice (0 of about 2000); every message ended delivered or failed with the reason "lifetime over"; of the messages that could be delivered (3 separate good 10 minute moments inside 24 h) 99.4 to 100 percent arrived; the flood: all 303 deliverable ones arrived while relays refused 11817 attempts when full. **Found by the soak:** (1) my first run lost most messages because the sim never expired relay RAM; the spec's 60 s rule fixed it. (2) Resending is heavy: about 29 re-sends per message because a receipt only returns when the sender is listening, and each resend is stopped only at the server and the receiver app, so it still costs air time; the retry rule needs the receipt to be sent earlier or the resend to be cheaper. (3) **The speed numbers of spec section 11 do not hold in this sim:** on good links with everyone listening, first-try delivery took a median of 322 s (90th percentile 985 s), not 12 to 35 s, because presence is refreshed only every 10 minutes, so a message nearly always waits for the next paging window. The spec's figure holds only if the receiver was heard in the last 2 minutes. **Decision (the owner said to choose what is best): deliver first, page only on failure; done in `router.js` and MESH-SPEC 6.3.** After the change, the same sim gives a first-try median of 33 s and a 90th percentile of 827 s (the half that fail first wait for a paging window), delivery 99.7 to 99.8 percent of deliverable messages. These times are still only the assumed air times, not radio measurements. The air times in the sim are the spec's own estimates, so the sim cannot confirm radio speed at all |
| 17 On the air | not started, not allowed yet | needs D1 to D3 settled, the band plan re-verified, and a real transmitter |

Measured on the test bench only. Nothing here has been on a real channel.

Step 4 limits: ideal channel and ideal transmitter only (no radio non-linearity, no drift between neighbours, no fading). The needed SNR is on the whole 8 kHz audio, not per lane. The speeds in MESH-SPEC 2.4 are still estimates; no speed was measured. The lane mode is not yet in the engine or the mode ladder.

## WP0 green baseline and 17 m sweep (2026-11-18, session E)

| Item | State | Evidence |
|---|---|---|
| Workspace clippy (`-D warnings`, all targets) | clean | fixed an unused import in `select.rs` left by the 17 m change |
| `cargo test --workspace` | all pass | core 347, daemon 77 (about 3.5 s), ui 15, log 47 and the rest |
| `crates/kahf-ui` | fixed | band selector is 17 m only (`Band::M17`), segment check uses the 480 Hz lane width, sample data says 17 m |
| Old band tables | removed | `bandplan::ALLOCATIONS` holds only 17 m (same rows in all three regions); the daemon logbook band table, kahf-gui, kahf-prop `HF_BANDS` and the kahf-log fixtures are 17 m only; SDR default frequency is 18.11344 MHz (lane C1 dial) |
| App, Tauri, server | pass | `svelte-check` 0 errors, vitest 24, `app/src-tauri` clippy clean and 32 tests, server 1173 tests |
| Docs | marked | NETWORK-CHANNELS, NETWORK-DESIGN and NETWORK-SCENARIOS carry a "background only" note. Old band names remain in history docs and in negative tests (they check that old bands are refused), by design |

Not verified: the real Tauri window was not run in this package; nothing has been on a radio. Website text (`server/src/web/*`) and release notes are WP11.

## WP1 transmit gate (2026-11-18, session E)

| Item | State | Evidence |
|---|---|---|
| Gate on every burst | built, in the engine | `kahf-daemon/src/lib.rs`: `gate_refusal` runs `lanes::permit_tx` with the dial the radio is really on and the audio span the modem fills. It is checked in the main loop (a refusal holds transmit and logs once) and again in `tx_advance` before PTT, so no burst skips it |
| Built-in table | still **not verified** | `plan_verified` stays false (owner only). On a real radio every transmission is refused with the text "Transmit is off: the 17 m band table is not verified ... Receiving works." Receiving is unchanged |
| Dial trap | refused | tests: lane centre typed as dial, an old 14.103 MHz setting, and an unknown dial are all refused; the lane dial 18113.44 kHz passes (on a verified table and a lane-width signal) |
| Today's modem is too wide | known limit, closed by WP4 | the engine still sends the wide KAMI (312 to 2690 Hz of audio). On a verified table it is refused as "would not fit inside the lane" until the lane mode is in the engine. So no real transmit is possible yet, on purpose |
| Test table (bench mode) | built | `Engine::set_test_table(true)`, setting `"testTable": true`. It opens **only** a bench radio: in-memory test radios, or a sound card pair where both devices are named BlackHole and no rig control or SDR is set. A real radio with the test table on stays refused ("The test table is on, but this is a real radio"). Logged loudly when the radio opens, and the Radio screen shows an amber "TEST TABLE ON" notice. Default off |
| App | wired | Tauri `engine_start` sets the dial from the lane setting and the test table from settings; command `engine_tx_gate`; the app polls it every 5 s and shows the reason (or the bench notice) on the Radio screen |
| Tests | 7 new daemon tests (gate), 1 settings test | unverified refuses and never keys PTT; verified lane signal passes; dial trap and unknown dial; wide signal refused; test table opens bench only; every burst rechecked; an SDR reports its own dial |

Decision (owner said "you decide"): the bench is defined by the device names (BlackHole) and the absence of rig control or an SDR, so the owner's two-instance bench works with `"testTable": true` in its settings files and no real sound card can use it. Not verified: the real Tauri window.

## WP2 lane sensing before every transmit (2026-11-18, session E)

| Item | State | Evidence |
|---|---|---|
| Sensing in the engine | built, bench only | `kahf-daemon/src/lib.rs`: when the radio's dial is a lane dial, a `LaneSense` over the lanes the audio covers (for C1: lanes 5 to 9 less the one too low) is fed all received audio. `pick_channel` asks `own_lane_clear`: the lane and both neighbours quiet (6 dB over their own floor), the lane listened to at least half a second, and not in quarantine. This replaces the old `channel_busy` carrier sense on a lane dial; with no lane dial the old sense still decides |
| Answers | own lane level skipped | an answer follows the other station's burst on the same lane (300 ms), so only the neighbours and the quarantine are judged for it |
| Quarantine and classes | wired | every 5 s each covered lane is classified; voice or CW blocks it and its neighbours 20 min, a steady carrier or a 15 s digital signal blocks it 10 min, 30 min, 2 h. Classes are rules tested on synthetic signals only |
| Noise floor | changed | found by the new tests: the floor was a 30 s minimum, so a foreign signal that stayed longer became the "noise floor" and the lane read clear. The floor is now the lowest half-second of the last 10 minutes (`lanesense.rs`). Limit: a signal already on the lane when KAHF starts, and staying, is seen as floor for the first 10 minutes only if it was there the whole time |
| Occupancy report | built, queued in memory | once a minute: lane number and dB over floor for each covered lane (at most 12 numbers, no audio). `Engine::take_occupancy()`. Sending it to the server is WP6 and WP7 |
| Tests | 4 new daemon tests (88 total) | quiet lane passes a message; a noise signal in the lane holds the transmit until it is gone and the message then arrives; noise on the next lane up also holds it; the report names the busy lane and not the quiet one. Background noise is on the test bus (an all-zero bus has an infinite floor) |

Not done or not verified: a voice or CW interferer in an engine test (the class rules are only unit tested), the wide modem still cannot pass the gate on a verified table (WP4), no real band has been heard. The daemon suite now takes about 15 s because the lane tests run long virtual spans.

## WP3 frame header, bundles and ordered answers on the air (2026-11-18, session E)

| Item | State | Evidence |
|---|---|---|
| Chat on bundle frames | built, bench only | `kahf-daemon/src/datagram.rs`. Every chat of up to 400 bytes now goes out without a link: the session chunks (26 bytes, `session::try_encode_sized`) travel one per codeword as `bundle` Message frames with the 9 byte header (next hop = 24 bit hash of the destination or of the relay, source hash, burst index and count) and a sequence byte. A station that is not the next hop drops the frame after the header. The receiver answers once per burst with an Ack frame (bitmap of the codewords it got, the sequence, its own SNR). The sender repeats only the chunks not acknowledged, after a random back-off that doubles |
| Bundles for several recipients | built | up to 4 messages are in flight; one burst of up to 8 codewords carries chunks for different stations. Test: a hub queues three messages for three stations, sends **one** burst, and all three arrive once |
| Ordered answers | built, **spec changed** | each recipient answers in the order the burst lists it. MESH-SPEC 8.3 says 300 ms apart. Found by the test: an answer is a whole burst (preamble and a codeword, about 1.7 s at the test speed), so 300 ms spacing would overlap. The gap is now one answer length plus 300 ms (`dg_ack_air`). The test shows the three answers in order and not overlapping |
| Duplicates | built | a message already delivered (its answer was lost) is answered again and not shown again (kept 10 min per source and sequence); frames not for this station get no answer. Unit tests |
| Link kept for | mail and files, and an explicit "Connect" | chats sent while the operator holds a link still go over the link. Mail and files move to the mesh in WP8. The auto-connect no longer opens a link for a chat |
| Mode choice | works | the ack carries the receiver's SNR; the sender uses it like a link report |
| Tests | 4 new daemon tests (92 total) | hub bundle to three stations in order; three stations talking at once, every message once and in order; repeat answered again but shown once; frame for another station ignored. Three older tests were adapted (a chat no longer opens a link; a relay forward is counted after the next hop's answer) |

Not done: relay forwarding of bundles over the air to a second relay is WP9; `h24` collisions are not handled beyond the callsign in the message; acknowledgement frames are not sent in the "next beacon window" (WP5, resend storm WP8); the multi-station tests run on the test bus with plain noise, no fading; no real radio. Mail with attachments and voice notes still use the link until WP8.

## WP4 lane mode in the engine (2026-11-18, session E)

| Item | State | Evidence |
|---|---|---|
| Lane mode on transmit | built, bench only | `kahf-daemon/src/lib.rs` `push_burst`: when the radio's dial is a lane dial, every burst is the plain KAMI burst turned into 480 Hz lane audio at 1500 Hz of audio by `narrow::to_lane` (silences stay short). The transmit gate now checks the lane span (1260 to 1740 Hz), so a lane signal on the right dial passes `permit_tx` on a verified table and the wide signal is refused |
| Lane mode on receive | built | `narrow::LaneRx` (new, streaming, same samples as `from_lane` within 1e-3, test `the_streaming_receiver_matches_the_whole_recording`) feeds the existing preamble search; everything sample driven in the engine works unchanged; time constants are scaled by five (`slow()`: preamble hold, answer waits, air estimates, status speed, search step) |
| All 12 lanes | tested | daemon test `a_chat_arrives_on_every_one_of_the_twelve_lanes`: two engines on a Bus on each lane dial in turn, a chat arrives |
| Width and speed | tested | test `a_lane_burst_on_the_air_stays_inside_its_lane_and_is_five_times_slower`: on the Bus the burst is five times the plain length and the level outside +-240 Hz is under -26 dB |
| Two-lane receiver | built | a station on C1 also decodes C2 (and the reverse) from the same sound card: a second `RxLane` state swapped into the worker's decoder (`swap_rx`). Tests inject lane bursts at the audio positions of C1, C2 and a spare lane: own lane and the other calling lane are heard, a relay lane station hears only its own lane, a calling lane station does not hear the spare lane. The test Bus has no radio frequency translation (every station's 1500 Hz audio lands on 1500 Hz for all), so stations on different lanes cannot be tested on it; the injection tests cover the receive side |
| Speeds in the app | exact, not measured | the app's mode table is the plain table divided by five (modulator arithmetic, documented in `docs/KAMI-MODES.md` "Lane mode"); needed SNR is the lane's own measure, unchanged |
| Leakage re-run | same as before | `lane_leak` re-run today: at 700 Hz pitch, neighbours 30 dB stronger cost -1 dB (KAMI-1c), +2 dB (1a) and 0 dB (8) (30 frames per point, 1 dB steps, so about +-1 dB). Pitch stays 700 Hz. Ideal channel and transmitter |
| Not done | **bonded waveform** | 2 to 4 lane wide signal is not built (needs a rational slowdown of 2.5, 1.3 and 1, a dial at the run centre and a gate for runs). The planner can plan bonded runs but nothing uses them; no grant path exists until the server planner is live (WP6). Also not done: lane mode HF channel sweep (fading in a five times longer burst is probably worse than the plain figures), and the ladder minimum SNR rungs were not re-tuned for lane mode |

## WP5 relay as referee of its lane (2026-11-18, session E), partly done

| Item | State | Evidence |
|---|---|---|
| Referee on the air | built, bench only | `kahf-daemon/src/referee.rs`, encodings in `kahf-core/src/referee.rs`. A relay on a lane keeps a 12 s round on UTC: slot 0 beacon (window, load, epoch, callsign) together with the answers to last round's data, slots 1 and 2 request window, slot 3 grants, slots 4 and 5 data. The relay no longer needs the 24 hour slot (`relay_slot_open` is true on a lane while relay mode runs). Requests, beacons and grants go out in KAMI-1a (one codeword fits a slot: 1.8 s in a lane) |
| Operator side | built | an operator that hears a beacon lays the slot grid on the burst it heard (no UTC), asks in a random window slot, sends its chat only in the granted slots, and gets its answer in a later beacon. A lost request or grant backs off, the window doubling up to 8. No beacon: listen-before-talk as before. A chat goes through the relay while its beacon is fresh |
| Tests | passing | one operator, the relay and a destination (hidden: the operator and the destination hear only the relay): request, grant, data, forward, delivered. Three operators, one relay: all delivered once. Plus core tests for the encodings and the slot rule |
| **Not finished** | ten operators | with ten hidden operators the relay delivered fewer than ten messages in 2400 virtual seconds (the run takes 15 minutes of real time). Capacity is two data slots per 12 s round on one lane, requests collide in two window slots, and an unanswered operator waits for a full 36 s before asking again. Needs: tuning (more window slots under load, shorter await, more data slots, grants on relay lanes), then a faster test. Also not done: relay crash and restart test, two relays on different lanes, grants that name another lane (the radio would have to follow), the relay answering through a planned slot from the server |
| Found while doing it | classifier | KAHF's own regular beacons looked like a periodic narrow signal and the sensing put the lane in quarantine for 10 minutes. A lane (and its neighbours) where a KAHF frame was decoded in the last 90 s is no longer classified as a foreign signal |

The ten-operator run is available with `REF_OPS=10 cargo test -p kahf-daemon hidden_operators -- --nocapture`.

## WP10 (part): 17 m by the sun in the app (2026-11-18, session E)

The Radio screen shows a plain note from the sun's height at the person's own place (`app/src/lib/geo.ts` `reachNotice`):
daylight (17 m usually works on 1000 to 4000 km paths), dawn or dusk, or night (usually closed, the outbox keeps the message and KAHF keeps
listening). It is a rule of thumb from the sun, not a measurement or a forecast, and it needs a shared place. Tests: vitest (26). The model learner on the server is not used yet.
Not done in WP10: the relay screen's referee status, lane occupancy bars from the sensing reports, first-run wizard changes, accessibility pass, and a check in the real Tauri window.

### WP5 update: ten operators

After the relay's downlink was limited to the first data slot, with the next slot left free for the answer (the answer of the station that receives it had collided with granted slots), `REF_OPS=10 cargo test -p kahf-daemon hidden_operators` **passes**: ten operators that cannot hear each other, one relay, every message delivered once, the last after 2038 virtual seconds (about 34 minutes; six operators took 314 s). That is simulation, with the spec's own slot plan, and it is slow: one lane gives two data slots per 12 s round and requests collide in two window slots. Capacity needs the planner's other lanes (WP6) or a bigger round. Still not done: relay crash and restart, two relays on different lanes.

## WP6 and WP7: the server mesh and the relay's link to it (2026-11-19, session E)

Owner decisions (asked once, 2026-11-19): the project signing key is a Cloudflare secret (`PROJECT_KEY`, base64 32 byte seed) with the public key in the
app; a WebSocket crate may be added (tungstenite, with rustls); the old queued-message backbone and its stored data may be removed.

| Item | State | Evidence |
|---|---|---|
| Mesh brain | built, tested | `server/src/mesh.js`: one socket per relay key, heartbeats, presence, routing with deliver-first, paging, duplicate control, receipts, plans four epochs ahead signed with the project key, relay suspension (three off-plan reports in ten minutes). 12 tests in `server/test/mesh.test.mjs` |
| Durable Object and routes | built, **not deployed** | `src/mesh-object.js` (`MeshObject`, RAM only; the one thing on disk is the learner's counts), signed `GET /mesh/ws`, `POST /mesh/frames`, `POST /mesh/poll` (`mesh-routes.test.mjs`: bad signature, old time, unregistered relay, another key, content-carrying frames, WebSocket upgrade). `wrangler deploy --dry-run` bundles |
| What passes through | decision | frames carry ids, sizes and hashes; a message travels as one opaque `blob` (at most 12 000 base64 characters) that only passes through the object's memory to the delivering relay and is never stored (tests: nothing of it remains after the receipt; the mesh object's only `storage.put` is the learner). Any field named body, text, subject, payload, content, data or message is refused |
| Old backbone | removed in code | `backbone.js`, `/msg/*`, `/relay/*`, `MAILBOXES`, `BACKBONE` and their tests are gone; migration v3 deletes the `MailObject` and `IndexObject` data **when deployed** (not undoable). The "known exception" in the no-storage test is gone |
| Planner twins | done | `bond`, `bondCount`, `hopGrants` in `server/src/planner.js` give exactly the strings the Rust planner gives (`known_bonds_and_hops_for_the_js_twin`). Plans carry bonded runs and hop grants (`P1|epoch|seed|grants|runs|hops`) |
| Learner and sun | built | per (relay, receiver hash, hour) counts, forgotten over about a week, saved by the object; today's NOAA solar flux and K index (hourly cron `7 * * * *`, not yet run live) scale the first guess |
| Relay side (daemon) | built | `backbone.rs` now speaks the mesh: signed HTTPS (`heard` as hashes, `submit` with the blob, poll, receipts) and a WebSocket push link (tungstenite, reconnect with back-off, HTTPS while it is down, one socket per key). A message nobody in the mesh hears goes on the radio. Plans are used only when signed by the project key (`PROJECT_PUBLIC_KEY_B64` in `backbone.rs`; the seed is in `server/.project-key`, git-ignored). Tests: signed call, frame parsing, plan signature (3 cases), plus two ignored tests against a local server: a round trip through two relays and the push latency |
| Latency | measured on loopback only | relay to relay over loopback with the local WebSocket server: 53 to 63 ms (limit 100). That is not internet latency |
| **Not done** | | the relay does not yet use the server's plan to decide when to transmit (its own 12 s referee round still runs; plans are verified and kept); the clash-domain graph and colouring (all relays are still one domain); the learner is fed by `heard` frames and receipts only; the NOAA job and the WebSocket path have not run on the live Worker; sending the occupancy reports to the server |

**To deploy (owner asks first):** `cd server && npx wrangler secret put PROJECT_KEY < .project-key && npx wrangler deploy`. Rollback of the code: `npx wrangler rollback`; the deleted backbone data cannot come back. Old app versions that call `/msg/*` will get 404.

## WP8 (part): delivery states and presence (2026-11-19 to 2026-11-21, session F)

Done and tested: a sender sees where a chat stands (`Event::Delivery`: AtRelay, Delivered, Waiting) and the
app shows it. An operator that hears a relay says "I am here" in a window slot at most every ten minutes
(`Kind::Here`); the relay counts these (`heres_heard`) and passes only a 24 bit hash on to the mesh.
Decision (recorded because the owner was away): two operators may pick the same window slot and then the
relay hears neither. That is by design (they repeat after ten minutes), so the test uses one operator.
Deliver-first (relay delivers straight to the destination, pages other relays only on failure) lives in the server
router (`router.js`) and in the engine's relay forward; the engine does not page other relays itself. Added tests (virtual clock, in-memory buses): six chats cost at most 18 bursts and arrive once each; mail, files and
long texts never take the chat path; a message written while the relay is down arrives after it restarts; a relay's data
folder holds no trace of the text it carried. Not measured on air: everything here runs on
the virtual clock and in-memory buses.

## Merge with origin/main (2026-11-21, session F)

The owner's other agent moved main on (62 commits: Alloy UI app, messaging layer, new modem ladder, GPL-3.0-or-later).
Decisions, taken because the owner was away, and why:
- Engine: our `kahf-daemon` (lane gate, referee, bundle chat, mesh link) is the base. Main's daemon was built on
  ladder bursts and the old queued-message relay; the two modem paths cannot both drive one engine, and the lane
  design is the 17 m plan. Main's `kahf-core` work (burst ladder, msg layer, weak-signal sync) is kept in core but is
  not used by the engine. Not in the engine: main's `lbt.rs`, `server_sync.rs` and daemon examples (removed).
- Server: our mesh backend and home page; main's extra pages (/network, /start, /protocol, /legal) and the GPL text
  are kept; the old backbone is gone (owner decision 2026-11-19).
- Apps: the Alloy UI app (`app/`) and `kahf-app-core` are main's and are kept, adapted to our engine. Old Tauri app
  files are deleted; `kahf-ui` and `kahf-gui` (egui) are no longer built (they used the removed relay API).
- Alloy app: twelve lanes (R1-R8, X1-X2, C1-C2) replace the three channels; the dial the person sets is the lane's USB
  dial and is passed to the transmit gate; the Radio screen shows why transmit is off; the Relay screen shows referee
  counters; delivery states and the Ko-fi button exist.
Open: the server still has a three-channel band constant from main (used by its stress tools only); the relay switch
in the app enables one lane for 24 h, not all relay lanes; relay slots/server sync are no longer shown.

Alloy UI layout issue (found 2026-11-21, not fixed here, it belongs in the Alloy library): a wrapped paragraph does not
add its extra lines to the layout height, so the next widget draws over it (seen on the Relay intro text). Without
`fill_w()` the text does not wrap and runs off the right edge of the 420 px side panel instead. Pictures: docs/screens.

## WP12 (part): security review of the mesh server (2026-11-21, session F)

Read `routes.js` (meshAuth), `mesh.js`, `mesh-object.js`. Fixed, each with a test: (1) one relay could suspend any other
relay with three false clash reports; now three reports from at least two different relays are needed; (2) the table of
receipts waiting for an answer grew without bound; now entries expire after 30 minutes and at most 5000 are kept;
(3) a message id of any JSON type and size was stored and echoed; now a string or number of at most 40 characters;
(4) no size limit on a WebSocket frame; now 16 KB. Known and accepted: a signed connect URL can be replayed within its ten
minute window (it needs a captured TLS request, and only kicks the relay's own earlier socket); relays are trusted for
the sizes and hashes they report (a registered relay can lie about what it heard, which the learner tolerates because it
only counts). Not done: an outside review, a longer soak with real engines in a cell, the daemon's parsing of server
frames under fuzzing.

Deployed 2026-11-21 (owner: "you do it"): signing-key secret `PROJECT_KEY` uploaded, Worker deployed (version 58e80977), old backbone data deleted by migration v3. Decision: the relay switch in the app enables the one lane the plan gives it (a relay serves one lane at a time), not all eight relay lanes.

Soak in a small cell (virtual clock, in-memory buses; not radio): three operators, four chats each at uneven gaps over three minutes, one relay: all twelve arrive exactly once and in order per sender, never from the receiver's own callsign (`a_cell_of_three_operators_delivers_every_message_once_and_in_order`, about 90 s of real time). Two relays chained by relay hints deliver (`a_message_goes_over_two_relays`). Still open: ten operators in one cell (not finished inside 2400 virtual seconds, WP5 capacity), and any soak with real radios.

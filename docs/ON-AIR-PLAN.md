# KAHF: plan for the first transmission (for the owner)

Written 2026-11-18. **Nothing in this file has been done.** The software has never transmitted: the transmit
gate (`kahf-core/src/lanes.rs` `permit_tx`, used by the engine before every burst) refuses on a real radio
because `plan_verified` is `false`. Only the owner changes that, and only after step 1 below.

## 1. Before anything goes on the air

1. **Check the band plan, by a person.** Compare the 17 m table (`BandTable::builtin_r1`, MESH-SPEC 1.1 and 2.2)
   with the current IARU Region 1 HF band plan (the 2020 edition was read: 18111 to 18120 kHz is 2700 Hz, all modes,
   digimodes and automatically controlled data stations; 18109 to 18111 kHz is beacons only). Check the wording on
   unattended stations and contests, and whether a newer plan exists. The FT8, FT4, WSPR and IBP positions in the
   table's `excluded` list are from memory and must be checked. Only then set `plan_verified: true` (one line), and record
   the date and who checked in `docs/NETWORK-STATUS.md`.
2. **Answer the open questions.** D1 (unattended relays: you said allowed where you live; each country differs),
   D3 (Regions 2 and 3: the US allows data only up to 18.110 MHz, so those lanes are not usable there), your own
   licence conditions for automatic stations and for third-party messages.
3. **Station identification.** Every transmission carries the sender's callsign in the frame; check whether your licence
   also needs a spoken or CW identification, and add it if so (not built).
4. **Pick the first radio set-up.** A transceiver on USB (upper sideband) with a sound-card interface, PTT by CAT
   (rigctld) or VOX. The dial must be exactly the lane's USB dial (C1: 18.11344 MHz). The app refuses any other dial.
   A radio whose SSB filter passes 300 to 2700 Hz reaches lanes at 800, 1500 and 2200 Hz of audio, which is why a
   station on C1 also hears C2.

## 2. Bench checks with a dummy load (no antenna)

1. Dummy load, transmit level at its lowest. Run the app's test tone; set the audio drive so that the ALC does not move
   (no ALC action: a driven rig splatters across neighbouring lanes, which are only 220 Hz apart).
2. With a receiver or SDR on a second antenna, look at the signal: it must be 480 Hz wide, centred 1500 Hz above the
   dial, with nothing outside +-240 Hz that you can see. If there is, lower the drive.
3. Check the transmit cut-off: the software stops a burst after its planned length; confirm PTT drops.
4. Confirm the app refuses to transmit on any other dial setting (change the dial by one kHz; it must say so).

## 3. First on-air test, two stations (you and one other licensed operator)

Conditions: daytime path of 1000 to 4000 km (17 m skips the first thousand or so; check with the sun line on kahf.dev),
a quiet frequency, an observer listening on the band, both stations on **C1** (the app's default).

1. Listen for ten minutes first. If anyone is on the lane or its neighbours, stop; try later or on C2.
2. One short chat, one direction. Expected: it goes as a few codewords, the other station answers once, "delivered".
3. Both directions, then several messages, then a mail of about 1 kB.
4. **Record**: UTC time, distance, the measured SNR the app shows, the mode it chose, how long each message took, what
   failed. These are the first real numbers. Everything quoted today (speeds, SNR limits) is a bench or simulation
   figure.

Stop conditions: any sign of interference to others, a stuck transmitter, a message to the wrong station, the app
transmitting when it should not, or an observer's report. Stop, switch the radio to receive, write down what happened.

## 4. Then a relay

1. A relay needs a licence that allows automatic stations, the terms accepted in the app, a shared location (area
   shared is enough) and internet. Today the relay works on the radio side only: the server link (routing, planning)
   is not connected.
2. Start relay mode on **C1** with two operators on the same lane that cannot hear each other, if you can arrange it.
   Watch the round: beacon every 12 seconds, requests, grants, data.
3. Stop conditions as above, plus: two relays on one lane (not supported yet), a relay that does not stop on the
   stop button.

## 5. What this plan does not cover yet

- Several relays at once and the server's planned slots (WP6, WP7).
- Wider signals (bonded lanes), hopping between relays, combining weak copies from several relays.
- Any HF fading measurement of the five times slower bursts. They may be worse than the plain modem on a fading path.

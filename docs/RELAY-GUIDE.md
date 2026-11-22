# KAHF relay guide

A relay is a licensed station that keeps order on a lane and passes messages on for others. This guide says what
the current build does. **Nothing here has been tried on a real radio.** The app refuses to transmit until the 17 m band
table has been checked against the current band plan (see `docs/ON-AIR-PLAN.md`).

## What a relay does today

- Runs a 12 second round on its lane: a **beacon** (who it is, how busy it is), two **request** slots, a **grant**
  slot, two **data** slots. Operators who hear the beacon ask in a random request slot; only the operator that was
  granted a slot sends in it. Operators that do not hear each other cannot collide.
- Takes a chat from an operator and sends it on to the station it is for, in a free data slot. The sender's computer
  keeps its own outbox; the relay holds the message for a short time only and keeps nothing on disk.
- Answers everything it took with its next beacon.

## What it does not do yet

- It does not join other relays through the server (routing, paging and planning exist as code and tests, not as a live
  service).
- Only one relay per lane is supported. Two relays on the same lane would talk over each other until the server plan
  is live.
- Grants name only the relay's own lane.
- With many operators asking at once, throughput is low: two data slots per 12 seconds. Six simulated operators took
  about five minutes to get one chat each through.

## Starting a relay

1. Finish setup (callsign, sound card or SDR, place). Share at least your area on the Stations page; a relay needs a
   place to register.
2. Open **Relay**. Confirm that you hold a licence that allows an automatic station and accept the terms. Both boxes
   start empty every time.
3. Press **Start relay**. The screen shows the status and a **Stop relay** button. Stopping is immediate.
4. The relay uses the lane you set on the Radio screen. Use a calling lane (C1 or C2) so operators can find you.

## Your responsibility

You stay responsible for everything your station sends: bands, power, identification and the rules for third-party
traffic and unattended operation differ from country to country. The relay yields to anyone on the lane, KAHF or not.

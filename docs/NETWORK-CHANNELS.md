> **Background only (2026-11-18).** KAHF now works on 17 m only; where this file mentions 20 m, 40 m or 80 m, channel pools or one 24 h relay slot, `docs/MESH-SPEC.md` replaces it.

# KAHF with three channels

Owner decision: one channel will clog (mail is long, a busy network is busy), so each area uses
**three channels**. This file says how, what can go wrong, and what we do. It sits beside
NETWORK-DESIGN.md (layers) and NETWORK-SCENARIOS.md (single-channel scenarios, S and H numbers).
The scenarios here are numbered M1 to M32.

## 1. The idea in one paragraph

One channel is the **meeting channel (C0)**. It is always listened to, and it carries only short
things: beacons, requests, grants, answers, adverts, chats, urgent messages. It is never held for
more than a few seconds. Long things (mail, files, voice) are moved to one of two **data channels
(D1, D2)**: the relay says "go to D1 now, for this long", the two stations talk there, then both
come back to C0. C0 stays free, so the network keeps answering while a long transfer runs.

```
C0  beacon | request | grant | chat | answer | chat | beacon | urgent | ...      (always short)
D1          [ ........ long mail, station A <-> relay ........ ]
D2                       [ ..... file, station B <-> relay ..... ]
```

Why three and not more: C0 plus two data channels allows two long transfers at once and a free
control channel. A fourth channel adds little until a city is large. Where the plan has four
channels (20 m, see 2) the fourth stays the existing **Emergency** channel.

## 2. What the channel plan gives us today

`select.rs` and `bandplan.rs`: channels are 2.7 kHz wide on a grid inside the KAHF segments.
There are five in total (40 m: one; 20 m: four). They are split by thirds into pools Online,
Open and Emergency. The pools were written for a different idea (who may use which channel), not
for roles. Proposed change:

| Channel | Role | Replaces |
|---|---|---|
| C0 | meeting and control, short traffic, urgent | the Open pool |
| D1, D2 | data (long transfers), assigned by the relay | the Online pool (relays are the online stations) |
| E | emergency only, never assigned | the Emergency pool (unchanged) |

On 20 m this is exactly C0, D1, D2, E. On 40 m there is one channel, so 40 m runs single-channel
(C0 only, chunked transfers, see M4). The server may still change the plan (existing rule).

An honest limit: on HF the same four frequencies are shared by the whole world. Two cities
thousands of km apart can hear each other (M17). Three channels per area help a lot inside one
area and do nothing against a distant area on the same frequency. Section 7 lists what this means.

## 3. How a transfer works

1. **Beacon on C0** (every 30 to 60 s, and after each exchange): relay name, load, the state of
   D1 and D2 (free, in use until T), the time, and when its next request window opens.
2. **Request** (one codeword, in a random slot of the window): who, to whom, how many bytes,
   priority, and **capability**: can this station change frequency (yes or no) and how long it needs
   to retune.
3. **Grant** (on C0): station, **which channel**, **start** (counted from the end of the grant
   burst, "start in 2 s"), **length**, and **return-by**. Everyone who hears it knows C0 is not
   being used by those two, and which data channel will be busy. The relay chooses the data channel
   by its own sensing, load, and what is already assigned.
4. **Both sides QSY.** The station changes to the data channel, the relay too. If the relay has
   only one radio, it leaves C0 for the transfer (M10).
5. **Transfer** on the data channel. Normal link rules there (answer after each train, back-off),
   only the two parties transmit, so collisions inside a data channel are rare. The relay can send
   an **extend** if the transfer is going well and needs more time, within a hard limit.
6. **Done** (one short frame) or **timeout** (nothing heard for 20 s, or return-by reached). Both
   return to C0. The relay sends a catch-up beacon at once.

Which traffic goes where (by estimated air time at the current speed, not by bytes):

| Air time | Where |
|---|---|
| under 6 s, or any urgent message | C0 |
| 6 s or more (mail text over about 1 kB at KAMI-32, files, voice notes of more than a few seconds) | a data channel |
| less than 10 s for a station that needs more than 1 s to retune | C0, in a chunk (M5) |

## 4. Hardware reality

| Tier | Relay hardware | What it can do |
|---|---|---|
| 1 | one transceiver | listens on C0. Leaves it only for a transfer it grants, for at most 60 s at a time, then returns to C0 and checks. During the away time C0 is not heard, and the last beacon says "away until T". |
| 2 | a wideband receiver (SDR) plus a transceiver | hears all three channels at once, so no missed requests. Still sends on one channel at a time. |
| 3 | two or more radios | sends and listens on different channels at once. The best case. |

Operators have the same tiers: a rig controlled by Hamlib can change frequency (1 to 2 s); a
rig without control cannot (single-channel operator); an SDR receiver hears all channels.

The design **must work at tier 1** and use the better tiers when they exist.

## 5. Single-channel operators stay supported (M29)

An old app version, a rig you tune by hand, a handheld: these use C0 only. The relay serves them
on C0 in **chunks**: bursts of at most 8 s with a gap, a long file in several grants. Slow, fair
to others, and they cannot clog the control channel because the train and air-time limits apply
(NETWORK-DESIGN, L0 R4).

## 6. Scenarios

### Meeting and moving

**M1. The grant is heard by the station but a third station near it misses it.** The third
station hears C0 quiet for a while and may start a request. Harmless: C0 is only for short things.
It sees D1 listed as busy in the next beacon.

**M2. The station moves to D1 but the relay is not there.** The relay's grant was heard but the
relay's own radio was late or lost its link. Rule: the station listens on D1 for 10 s; hearing
nothing it returns to C0 and sends a new request (back-off). The relay, on its side, times the
same way. Neither waits for ever.

**M3. D1 is busy when the station arrives** (another operator, non-KAHF energy, a contest). The
station checks D1 for a second before sending. If busy, it does not transmit, tells the relay on
C0 ("D1 busy, try D2"), and the relay re-grants on another channel or says wait. A busy-channel
note goes into the beacon so the next grant avoids it.

**M4. The station cannot change frequency.** It said so in the request. The relay grants on C0
in chunks (section 5) and says "no channel change possible, will take about N minutes". The
station can also remove the long item and send it to a station that can.

**M5. Retuning costs 1 to 2 s.** Moving for a 7 s transfer wastes a third. The retune time is in
the request, and the relay counts it in the air time: moves only above 6 s plus twice the retune.
Short items go on C0.

**M6. The transfer needs longer than the grant.** The relay sends **extend** (on the data
channel, to the station) with a new return-by, up to a limit (5 minutes per grant, then back to
C0 and a new request). A station that does not get an extend returns at the return-by time.

**M7. The transfer ends early or breaks.** Done frame; or 20 s of silence; both return to C0. The
data channel is shown free again in the next beacon.

**M8. The station is stuck on a data channel** (relay gone, power, crash). Fail-safe: every
station has a return-by timer set from the grant, and goes back to C0 when it expires, whatever
else happens. The same for the relay.

**M9. Beacons missed while on a data channel.** The relay sends a catch-up beacon right after
each grant ends. A station with a wideband receiver does not miss any.

### Relay and its radio

**M10. The relay has one radio.** It leaves C0 only for a granted transfer and returns after at
most 60 s to look at C0 (the transfer can pause and resume). While away the beacon said "away
until T", so stations hold their requests. **Urgent traffic** can wait at most 60 s for a relay
with one radio. If an urgent message cannot wait, it goes out on the Emergency channel, which
relays with one radio check on return, and which an operator can address directly.

**M11. The relay has a wideband receiver.** It hears all channels, takes requests during
transfers, and answers on C0 when its transmitter is free. It is faster, and the beacon says it
has this ("full coverage").

**M12. The relay can only transmit on one channel at a time.** Two stations on D1 and D2 both
sending **to** the relay is fine (it hears both with a wideband receiver or tier 3). The relay
sending **to** stations on D1 and D2 at the same moment is not possible on one transmitter. Rule:
the relay serialises its own transmissions; the grants say which direction; downloads from the
relay are scheduled in turn, and their waiting time is in the grant.

**M13. Two directions on one channel.** A transfer has data one way and answers the other.
The normal link rule (one answer per train after a short gap) keeps them apart on the data
channel. A long download is a train of at most 20 s, then a listening gap for the answer.

### Neighbouring areas and relays

**M14. Two relays serve the same area.** Each needs three channels. With only four on 20 m they
cannot both have their own set. Rule: relays in the same area **share** C0 (one beacon schedule,
alternating phases; the lower callsign keeps phase 0) and **share** D1 and D2 (each grant says
which one is free, and relays hear each other's grants on C0). They also coordinate over the
backbone (they are online): the second relay learns the first's grants from the server and avoids
the same channel at the same time.

**M15. Two relays, different areas, same band.** On VHF or low-power local use, areas are
separate and channels can be reused. On HF they are not (M17).

**M16. A relay joins and picks channels.** It listens on all channels for a minute, notes beacons
and busy ones, then claims C0 and the two data channels that look free, and says so in its first
beacons. If it hears another relay's beacon on C0, it joins as in M14.

**M17. HF skywave: a station far away on the same frequency.** Distance stations sometimes
overlap an area for minutes (skip). They are hidden stations: nobody hears them with ground wave,
but they are there. Rule: sensing and back-off as in L0; grant timing is relative to heard bursts;
and the relay's beacon carries the **measure of foreign traffic** it hears on each channel, which
makes the choice of channel for the next grant better. We cannot stop skywave. We can avoid the
quietest channel always going to the same far station.

**M18. Day and night: the band stops working.** 20 m is good by day and 40 m at night, or the
other way round for local traffic. Rule: the relay is on one band at a time and announces on the
current band when it will switch (and to which); stations that miss it scan the bands in the
plan. Relays also publish their bands and times on the backbone, so the app shows "try 40 m
after 18:00". This is the largest unsolved scenario (section 7).

**M19. A channel is spoiled** (a contest weekend, a jammer, a nearby noise source). The relay marks
it bad after three failed grants in a row, stops using it for an hour, and announces so. If C0
is spoiled the relay moves C0 (M28).

### Direct, no relay

**M20. Two operators want to move a file to a data channel.** The sender's file offer on C0
carries a proposed channel and its size. The receiver answers accept or a different channel (it
may hear D1 busy where the sender does not). Both QSY, the sender goes first and sends a short
"here" frame; the receiver answers. If it fails, M23.

**M21. Only one of the two can change frequency.** The offer says so; the transfer stays on C0 in
chunks.

**M22. The proposed channel is busy at the receiver but quiet at the sender.** Hidden station.
The receiver's counter proposal is the answer. If both channels are bad, C0 chunks.

**M23. The meeting fails** (no signal after QSY on either side). Both return to C0 after 10 s;
the sender retries once with the other data channel, then falls back to C0 chunks and says so.

### Capacity and fairness

**M24. Everyone wants a data channel.** The relay queues requests, grants them in order of
waiting time (with priority classes), and tells each station its position and an estimate. The app
shows "second in line, about 6 minutes". If the line is long the relay says so in the beacon and
the app suggests sending the files later. (Typical capacity: at 1.7 kbit/s a 5 kB mail takes about
30 s and a 100 kB file about 8 minutes, so two data channels carry roughly 500 mails or 15 files
an hour; to be measured, not promised.)

**M25. A station asks for a data channel and then does not use it.** Rule: a grant is wasted
if nothing is heard within 10 s; the next request from that station waits longer, and three in a
row cost it a longer penalty. The relay counts this per callsign.

**M26. One big file against many mails.** Per-station air-time share, file size limit (100 kB),
and priority classes so mail goes ahead of files when both wait. Files are only taken by a
relay that has a data channel free; never on C0.

**M27. Urgent traffic during a long transfer.** Urgent messages go on C0 and may interrupt:
the relay stops granting new transfers when an urgent message is seen and the next beacon says
so. A transfer already running is not cut unless the message is an emergency, in which case the
relay sends "pause" on the data channels and returns to C0 (stations hold until "resume").

### Finding and moving C0

**M28. The relay has to change C0.** Announced in the beacon for at least 10 minutes ("C0 moves to
Y at time T", with the time measured from the beacon), and repeated on the old C0 for 10 minutes
after the move. A station that missed it scans the channel list (M31).

**M29. Stations with no channel change** (section 5): C0 only, chunked.

**M30. A station with a wideband receiver** hears beacons while on a data channel and while
waiting; it never misses a catch-up beacon and can start a request while on D1.

**M31. A new station, or one that lost the relay, searches.** It listens on the default C0 from
the plan, then on the other channels in turn (one minute each, a few minutes in all), and keeps
the last known relay's channels from the cache. If it hears nothing it uses direct mode (H5).

**M32. Old and new versions meet.** The beacon has a version number and a feature list. Old
versions ignore frames they do not know and use C0; new versions see the old station in the
request ("single channel") and serve it in chunks.

## 7. What three channels do not solve, and what we should decide

1. **HF is a shared global commons.** Four frequencies for the world is thin. Real capacity needs
   either VHF/UHF (local, can reuse channels in every town; fits the "area" idea), or more HF
   segments, or both. Is HF the first target, or VHF (a 2 m or 70 cm FM rig with a sound card)?
   KAMI is narrow, so it fits a 12.5 kHz FM channel with room for three channels there too.
2. **Band switching (M18).** A relay with one radio can only be on one band at a time. We either
   accept "this relay is on 20 m by day and 40 m at night" with scanning, or require operators to
   choose a band.
3. **A relay needs three frequencies** and, to be good, a wideband receiver. Is "relay = tier 1
   one transceiver" the minimum we promise, with tier 2 as the recommended setup?
4. **Retuning:** most operators will use Hamlib for the rig, which gives 1 to 2 s retuning. The
   QSY design depends on this. A rig without CAT (or a handheld) is single-channel.
5. **Legal:** automatic frequency changes under a relay's control are normal in some services and
   restricted in others. We warn, we do not block (owner rule), and the first-run screen asks the
   country (open question in NETWORK-SCENARIOS).

## 8. What gets built, and in what order

1. The channel roles in the plan (C0, D1, D2, E), replacing the three pools.
2. Beacon, request, grant, extend, done and return-by frames; the time rules (all relative to
   heard bursts).
3. A relay and a station that move channels, with the fail-safe timers (M2, M7, M8).
4. Tier 1 relay behaviour (away time, catch-up beacon), then tier 2 and 3.
5. Chunked single-channel fallback.
6. The simulator with **channels** (a shared medium per channel, retune time, one radio per
   station) so M1 to M32 are tests, not opinions.
7. App: channel strip ("C0 quiet, D1 busy until 14:20"), queue position, and the transfer
   estimate that already exists, now showing the data channel.

Steps 1 to 3 give the two-instance test something real to do (three BlackHole devices, one per
channel, are enough to test it on the laptop).

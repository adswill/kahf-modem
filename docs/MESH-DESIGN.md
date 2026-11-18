> **SUPERSEDED by docs/MESH-SPEC.md (2026-11-18).** Kept for history. Where this first draft says messages are stored, that 20 m and
> 40 m are the bands, or that relays hand traffic between themselves only over the internet, the spec replaces it.

# The KAHF network: how messages get anywhere, fast, with nobody touching anything

Status: design, 2026-11-18. Nothing in this document is built yet unless it says so. It replaces the
parts of NETWORK-DESIGN.md, NETWORK-SCENARIOS.md and NETWORK-CHANNELS.md that disagree with it;
the rest of those documents still holds (the scenario list there is still the test list).

Part 1 is the full design. Part 2, at the end, is the same thing in plain words.

---

# Part 1. The full design

## 1. What the network must do

1. **Anyone to anyone.** An operator in Europe types a callsign in China or Australia, presses
   Send, and does nothing else. The answer comes back the same way.
2. **Fast.** A short message should reach the other person in about 10 to 20 seconds when both
   are near a relay, and the "delivered" tick should be back in about 20 to 40 seconds. Section 7
   shows where those seconds go.
3. **No settings.** Nobody picks a band, a channel, a relay or a time. Operators only choose who
   to write to. Relay owners only switch relay mode on.
4. **Runs by itself, all day, every day.** Relays survive crashes, internet drops, band changes
   and busy weekends without anyone at the keyboard.
5. **Good neighbour on the bands.** KAHF never sits on frequencies other activities depend on
   (FT8, WSPR, beacons, emergency nets, and the rest in section 5), never transmits over a signal
   it can hear, and always identifies itself.
6. **Works without internet for operators.** An operator with only a radio and a computer is a
   full member. Only relays need internet.

## 2. Three kinds of station

| | Operator | Relay | Server (kahf.dev) |
|---|---|---|---|
| Who | any licensed ham (anyone may listen) | a licensed ham who leaves a station in relay mode | Cloudflare, run by the project |
| Internet | not needed | **required**, always | is the internet side |
| Radio | yes | yes, with rig control (it changes band by itself) | no |
| Decides | nothing; follows the rules in its app | nothing on its own; follows the server's plan | everything that can be decided centrally |

The owner's rule, kept: **a relay is only a relay while it is connected to the server.** A relay
that loses the internet stops acting as one (section 9.5).

Why this split is the fast one: the slow, scarce, shared thing is HF air time. The internet is
practically instant and free. So **radio is used only for the first and the last step** (operator
to nearest relay, nearest relay to operator), and everything in between travels over the
internet. A message from Europe to China crosses two short radio hops, not twenty long ones.
Long radio hops are kept only for one job the internet cannot do: reaching an operator in an area
with no relay at all (section 3.3, "reach-out").

## 3. The path of a message

### 3.1 The normal path (both people near a relay)

```
 Anna (EU, offline)          Relay R1 (EU)           Server            Relay R2 (CN)          Bo (CN, offline)
      |  1. request (tiny)     |                       |                    |                       |
      |----------------------->|                       |                    |                       |
      |  2. grant              |                       |                    |                       |
      |<-----------------------|                       |                    |                       |
      |  3. message burst      |                       |                    |                       |
      |----------------------->|                       |                    |                       |
      |  4. "at relay" ack     |  5. submit (HTTPS)    |                    |                       |
      |<-----------------------|---------------------->|  6. push (socket)  |                       |
      |                        |                       |------------------->|  7. in next bundle    |
      |                        |                       |                    |---------------------->|
      |                        |                       |                    |  8. ack in its slot   |
      |                        |                       |  9. receipt        |<----------------------|
      |                        |  10. receipt push     |<-------------------|                       |
      |  11. receipt in bundle |<----------------------|                    |                       |
      |<-----------------------|                       |                    |                       |
```

Anna sees three states: **Sent** (on the air), **At relay** (step 4: R1 has stored it), and
**Delivered** (step 11: Bo's radio has it). Each has a time stamp. Section 10.4 gives the full list.

### 3.2 The shortcut (the two people can hear each other)

If Anna's app has heard Bo recently, clearly enough, on the channel she is on, it sends to Bo
directly with no relay. If Bo does not answer in two tries (about 15 s), the app falls back to the
relay path without asking Anna. The same message id is used, so Bo never gets it twice.

### 3.3 Reach-out (the other person has no relay near them)

The server knows roughly where every operator was last heard (section 4.1). When a message is
for someone whose area has no online relay, the server chooses an online relay that **can reach
that area by sky-wave at this hour** (from its propagation table, section 4.3), and books it a
**reach-out window**: a few minutes on a long-distance channel. In that window the relay calls
the person by callsign and delivers. Operators listen for reach-out calls at fixed, publicly known
times (the band clock, section 5.5), so an offline operator can be found without knowing anything
in advance.

Reach-out is slower (minutes, not seconds) and is the only place long radio hops are used.

### 3.4 Last resort

If nothing can reach the person (no relay hears them, no reach-out works in 48 hours), the message
expires and Anna sees **Could not deliver**, with the reason. Nothing is ever dropped silently.

## 4. The server: the brain

The server runs on Cloudflare: Workers for requests, Durable Objects for anything that must be
consistent, Cron triggers for the planner, KV for things that are read often and change rarely,
D1 for history.

### 4.1 What it keeps

| Table | What | Kept for |
|---|---|---|
| Relays | callsign, key, area, bands and power it can use, antenna class, country, status, last heartbeat, queue length | while registered |
| Heard | who heard whom: station, relay, band, channel, SNR, time | 48 h |
| Presence | for each operator: the relays that hear them best, per band and hour of day | 30 days, decays |
| Mailboxes | messages waiting for each callsign, with state | until delivered or expired (48 h) |
| Ids seen | every message id, first relay that submitted it | 50 h (longer than any message lives) |
| Propagation | measured SNR between areas, per band and hour, plus the current solar data | 30 days rolling |
| Plans | the channel and time plan handed to each relay (section 6) | while valid |

Operators' locations are never needed: an operator is placed by **which relays hear them**. If
an operator chooses to share a location for the map, that is separate and stays opt-in.

### 4.2 Decisions it makes

1. **Which relay serves which operator** ("home relay"): the one that heard them best lately,
   adjusted for that relay's load. Re-decided every time new hearings arrive.
2. **Which relay delivers a message**: the destination's home relay; if it has not heard them for
   an hour, the next best; if none, reach-out (3.3).
3. **Who ignores what**: when two relays hear the same message, the first to submit it owns it;
   the server tells the other "drop it" in the reply. On the air, every frame names its next hop,
   so only the named station acts on it.
4. **Band, channel and time for every relay** (section 6).
5. **When to give up** and send "could not deliver" back.

### 4.3 How it predicts propagation

Two sources, blended:

- **A built-in model**: which bands work over which distance at which local time, from the sun's
  position over the path and the current solar flux and geomagnetic index (fetched hourly from
  NOAA's public space weather service). Rough, but it works from the first day.
- **What KAHF actually hears**: every decoded frame reports band, SNR and both ends' areas. After
  a few days the measurements replace the model for every pair of areas that has traffic. A band
  that the model says is open but nobody hears on is marked closed for that hour.

### 4.4 How it talks to relays

Each relay holds one **WebSocket** to the Durable Object for its area, open all the time
(Cloudflare can park an idle socket at no cost). The server **pushes** a message to the
delivering relay the instant it arrives, instead of waiting for the relay to ask. Relays send a
heartbeat every 30 s on the same socket. If the socket drops, the relay reconnects with back-off,
and in the meantime uses plain HTTPS polling every 10 s, so nothing stalls.

(Today the backbone is HTTPS polling only. Push is the single biggest speed gain on the internet
side.)

### 4.5 If the server is down

Plans are handed out with an expiry of 30 minutes, so relays keep their bands and times for that
long. Relays keep accepting messages, keep delivering anything they already hold, and queue the
rest. When the server returns they submit the queue; ids stop any doubles. After 30 minutes without
a plan a relay falls back to its last band with plain listen-before-talk (section 6.4).

## 5. Frequencies

### 5.1 The facts

- A KAHF signal uses audio 300 to 2700 Hz, so 2.4 kHz of spectrum. Channels sit on a 2.7 kHz grid,
  leaving a small guard between neighbours.
- HF bands are shared by the whole world. The same frequency in Europe and in Japan is the same
  frequency: when a band is open between them, they hear each other.
- The usable windows are small. Today KAHF has five channels: one on 40 m and four on 20 m, in
  IARU Region 1's segments (`bandplan.rs`).

### 5.2 What KAHF never touches

The table below lists activities KAHF must keep clear of. Frequencies are the usual dial
frequencies (USB); most signals sit 0.3 to 3 kHz above them. **Every number in this section must
be checked against the current IARU band plans of all three regions and the national rules before
any transmit is allowed; they are written from memory and are a starting list, not a source.**

| Activity | Typical frequencies (MHz) |
|---|---|
| FT8 | 3.573, 7.074, 10.136, 14.074, 18.100, 21.074, 24.915, 28.074 |
| FT4 | 3.575, 7.0475, 10.140, 14.080, 18.104, 21.140, 24.919, 28.180 |
| WSPR | 3.5686, 7.0386, 10.1387, 14.0956, 18.1046, 21.0946, 24.9246, 28.1246 |
| JS8Call | 3.578, 7.078, 14.078, 21.078, 28.078 |
| PSK31 and similar | 3.580, 7.040 (Region 1), 7.070 (Region 2), 14.070, 21.070, 28.120 |
| International beacon project | 14.100, 18.110, 21.150, 24.930, 28.200 |
| Emergency centres of activity (Region 1) | 3.760, 7.110, 14.300, 18.160, 21.360 |
| SSTV | 14.230, 21.340, 28.680 |
| 60 m (5 MHz) | whole band: channels and rules differ in every country |
| 30 m (10 MHz) | whole band for KAHF: its digital segment is too narrow for 2.4 kHz |

One finding to check now: today's 40 m channel (7.040 to 7.045 MHz) starts on the usual Region 1
PSK31 frequency. It should move up to sit between PSK31 and FT4, or into the part of the 40 m plan
marked for automatically controlled data stations, whichever the current plan allows.

### 5.3 The bands KAHF uses, and what each is for

The key idea: **use short-range bands for short links, so the same channel can be reused all over
the world, and use long-range bands only when a long link is the point.**

| Band | Reach at night | Reach by day | KAHF use |
|---|---|---|---|
| 80 m | 0 to 1500 km | 0 to 300 km | local: operator to relay at night |
| 40 m | 0 to 3000 km | 0 to 600 km | local by day; regional at night |
| 20 m | often closed | 700 to 4000 km | reach-out by day |
| 17 m, 15 m | closed | 1000 to 4000 km+ | reach-out when the sun is active |

(Typical values; the server's measurements replace them, section 4.3.)

So: **80 m and 40 m carry almost all traffic**, because operators are normally within a few hundred
kilometres of a relay. A 40 m channel used by a relay in Germany at noon is not heard in Spain, so
Spain can use the same channel at the same moment. 20 m and up are kept for reach-out, where a long
reach is wanted, and are therefore where clashes are most likely and the server's time plan matters
most.

Proposed windows (Region 1, to be confirmed as in 5.2): 80 m inside the digital segment that
allows automatically controlled stations (two channels), 40 m inside the part marked for
automatically controlled data stations (one or two channels), 20 m 14.101 to 14.112 MHz (four
channels, as today), 17 m and 15 m inside their automatic data segments (one or two each).
Regions 2 and 3 get their own tables. **The plan is data**: the server sends it, signed, and the app
carries a built-in copy for operators who are offline. A frequency is used only if it is in the
table for the station's own region.

### 5.4 Channel roles

Inside each band's window (unchanged from NETWORK-CHANNELS.md where the band has room):

- **C0, meeting**: beacons, requests, grants, short messages, acknowledgements, reach-out calls.
  Never held for more than a few seconds by anyone.
- **D1, D2, data**: long items (mail with attachments, files, voice). A relay moves one operator
  there for one transfer, then both come back to C0.
- **E, emergency**: never assigned, never used for normal traffic.

A band with one channel runs everything on C0 in short pieces.

### 5.5 The band clock: how offline stations know where to listen

An offline operator cannot ask the server where to listen. So the app works it out from **the time
and where the sun is**, with rules that every station has built in:

1. **Home channel.** If the sun is up at the operator's position, listen on 40 m C0; if it is
   down, on 80 m C0 (or 40 m C0 if the operator has no 80 m antenna; the app asks once, at setup,
   which bands the radio and antenna can do).
2. **Reach-out minutes.** In minutes 0 to 2 of every 10 minutes (UTC), every idle operator
   listens on the reach-out channel instead: 20 m C0 while the sun is up there, 40 m C0 while it is
   down. Relays doing reach-out call only in these minutes.
3. **Following a relay.** As soon as the operator hears a relay's beacon, the beacon's schedule
   wins over rules 1 and 2: the beacon says which band and channel this relay is on and when it will
   change.

The operator's computer clock is good enough for this (rules 1 and 2 need about plus or minus 30 s).
The first beacon heard sets the clock to the relay's time, which comes from the internet.

## 6. Keeping channels from clashing

Four layers, from the planned to the instinctive.

### 6.1 Frequency reuse, planned by the server

The planner (a Cron job every minute, plus whenever a relay joins or leaves) builds a **clash
graph**: two relays clash on a band at an hour if either can hear the other there, or if their
operators can hear the other relay. "Can hear" comes from the propagation table (4.3), not from
plain distance, because sky-wave skips over nearby areas and lands far away.

It then gives every relay a channel, so that relays which clash never share one: graph colouring,
where the colours are the channels. On 80 m and 40 m most relays get a channel to themselves
because the footprints are small. The plan is recomputed when the hour changes the footprints
(dawn and dusk move them most).

### 6.2 Time sharing, where frequency is not enough

Where more relays clash than there are channels (a dense city, or 20 m by day across a continent),
clashing relays share a channel **by time**: a 30 s cycle cut into slots, each relay owning some
slots in proportion to its queue. Slot edges are in UTC; relays keep time from the internet. The
server only adds time sharing where it must, because every slot a relay waits is latency.

Reach-out windows (3.3) are booked the same way, across the world: one reach-out channel, one
relay at a time per footprint.

### 6.3 Inside one relay's area: the relay runs the channel

Within its channel and slots, the relay is the referee (this is NETWORK-DESIGN.md L2, kept):

1. **Beacon** every 30 to 60 s: relay callsign (this is also the legal identification), its
   schedule, its load, the codes of operators it holds messages for (9.3), and the start of the
   next request window.
2. **Request window**: a short window after each beacon and after each exchange, cut into slots.
   An operator with something to send picks a random slot and sends a one-frame request (who,
   how much, how urgent).
3. **Grant**: the relay answers each request it decoded, in order, with how long that operator may
   send. Everyone who hears a grant keeps quiet for that time. This removes the hidden-station
   problem (two operators who cannot hear each other but both reach the relay).
4. **Answers in a fixed order.** After any burst, the station it was for answers first (300 ms);
   in a bundle (7.2), recipients answer in the order the bundle lists them. Nobody's answer is left
   to chance.

A request costs one frame; a collision costs one frame. Long sends never collide.

### 6.4 Sense before every transmit, always

Every station, every time: listen to the channel before sending (`sense.rs`). If there is any
signal on it (KAHF or not), wait. If a non-KAHF signal stays on a channel, the relay reports
**channel occupied** and the server moves that relay to another channel at the next plan. KAHF
yields to everyone; it never assumes a channel is its own.

Where there is no relay and no plan (operators talking directly), this layer plus the back-off in
`mac.rs` is all there is, and that is fine for light traffic.

## 7. Speed

### 7.1 Where the seconds go (normal path, short chat, estimates)

| Step | Time | Why |
|---|---|---|
| Wait for a request slot | 0 to 3 s | request windows open after every beacon and exchange |
| Request and grant | about 1 to 2 s | one frame each way at a fast mode |
| Message on the air | 1 to 5 s | 100 bytes is 3 to 4 frames: about 1 s at KAMI-1a, 3 s at KAMI-16, 5 s at KAMI-32 |
| "At relay" ack | 0.3 s after | the relay answers first, by rule |
| Relay to server to other relay | under 1 s | HTTPS in, push out |
| Wait for the other relay's next send | 0 to 5 s | it sends whenever its channel is free; bundles go out every few seconds when busy |
| Delivery bundle on the air | 1 to 5 s | shared with others' items |
| Bo's ack | 0.3 to 2 s | in his place in the bundle's answer order |
| Receipt back to Anna | 2 to 10 s | same path in reverse, as a tiny bundle item |
| **Delivered to Bo** | **about 5 to 20 s** | |
| **Anna sees "Delivered"** | **about 10 to 35 s** | |

Weak signals push this up: at KAMI-256 a single frame takes about 8 s. The app always picks the
fastest mode the measured signal allows (already built), so slow modes are used only when they
are the only thing that gets through.

### 7.2 Bundles: the relay's "collect and send together", done without waiting

The owner's idea, turned into a rule: **a relay never waits to fill a batch. Every time it may
transmit, it sends everything that is waiting, in one burst.** Waiting to collect three messages
would add seconds to the first one; sending whatever is ready costs nothing extra.

A bundle is one burst of up to 8 frames. Each frame stands alone (a lost frame loses only its own
items) and holds several small items:

| Item | Size | Use |
|---|---|---|
| Short message | up to about 27 bytes of text | most chats fit in one or two items |
| Receipt | about 7 bytes | "your message X was delivered" |
| Mail notice | about 10 bytes | "mail for you, this size; ask for it" (then moves to D1) |
| Ack | about 5 bytes | "I got items 2 and 5" |

Each item names its recipient by a short code made from the callsign (3 bytes). A station reads the
frame header and the codes, and skips everything not for it, so ignoring other people's traffic
costs nothing. Recipients answer in the order the bundle lists them (6.3, rule 4).

What this saves: one preamble and one channel access per item, so ten receipts go out in one
burst of about 2 to 3 s instead of ten exchanges of about 3 s each. Operators do the same on the way
in: their acks for several items travel in one frame.

### 7.3 The other speed rules

- **Push, not poll**, on the internet side (4.4).
- **No long waits inside a relay area**: the relay is the referee, so there is no random back-off
  inside its channel; only the request slot is random, and it is short.
- **The answer goes first**: 300 ms after a burst, by rule, for everyone.
- **Short things never queue behind long things**: long items are moved to D1 or D2 (5.4).
- **Direct when possible** (3.2).
- **Receipts are tiny and ride in bundles**, so "Delivered" never waits for a slot of its own.

## 8. Operators without internet

### 8.1 Finding a relay

Follow the band clock (5.5). The first beacon heard tells the app which relay, which channel and
which times. The app keeps a list of relays it has heard, with their schedules, and returns to the
best one when it starts up. If no beacon is heard for 10 minutes, it tries the other band for 10
minutes, then goes back to the band clock.

### 8.2 Time

The computer's clock is used until a beacon is heard; then the relay's time is used (relays have
internet time). Nothing an operator does needs better than about a second, because inside a relay's
area all timing is counted from the relay's own transmissions.

### 8.3 Messages waiting for someone who is not listening

The server keeps the message in that person's mailbox (48 h). Every relay that could reach them
puts a short code for each waiting callsign in its beacon. When the operator's app hears its own
code, it asks for its mail in the next request window. So an operator who switches on after a day
away gets everything within a minute or so, without the network having called them blindly.

### 8.4 Knowing who someone is

Messages are signed (already built). Keys are learned on first contact and kept; a changed key is
shown as a warning. Nothing is encrypted (amateur rules), so anyone can read traffic, but no one can
fake a sender.

### 8.5 No relay anywhere

Operators still reach each other directly (3.2) with listen-before-talk. Operators can also be
reached by reach-out (3.3) if a relay anywhere can hear their area. An operator never relays other
people's traffic unless they switch on an "emergency relay" option (off by default), which runs
the store-and-forward rules of NETWORK-DESIGN.md L2 without the server.

## 9. Relays that run themselves

### 9.1 Starting

Switch on relay mode once: confirm the licence and the terms (already built), let the app read the
radio's bands through rig control, register with the server (key, area, bands, power, country).
From then on, the relay starts with the computer and needs nothing.

### 9.2 What it does every minute

Keep the socket to the server open, send a heartbeat (status, queue, radio health, measured noise
on its channels), take the newest plan, switch band and channel through rig control when the plan
says so, beacon, run its channel (6.3), submit what it receives, deliver what it is pushed.

### 9.3 Never losing a message

- **Custody**: a relay says "at relay" only after the message is written to disk. If it crashes,
  it reads the disk on restart and carries on.
- **Hand-over**: a message leaves the relay's care only when the server confirms it stored it.
- **The sender keeps a copy** until "Delivered" (or "Could not deliver") comes back, and resends
  through another relay if "At relay" does not arrive within a minute.
- **Ids stop doubles** at the server and at the recipient, so every resend is safe.

### 9.4 Health checks

A watchdog inside the app restarts the radio side if it stops hearing anything for a long time,
restarts the network side if the socket stays down, and tells the server what it did. The server
marks a relay **unhealthy** if heartbeats stop or its deliveries stop being acknowledged, and moves
its operators to the next best relay.

### 9.5 When a relay loses the internet

It stops being a relay at once: its beacon says "local only". It still delivers what it already
holds and still takes messages for operators it can hear directly, but it does not accept
traffic for anywhere else. Its operators move to the next relay they can hear. When the internet
returns, it submits what it collected and becomes a relay again.

### 9.6 Too much traffic

Per-callsign limits (messages and bytes per hour) and a "busy, try at 14:20" answer (built in the
link layer). The server spreads operators across relays by load. Long items are refused on 20 m
reach-out windows, where air time is most precious.

### 9.7 The law

Relay mode asks for the licence each time it starts (built). The server keeps a list of countries
and bands where unattended operation is allowed and does not give a relay a plan it may not use.
Every beacon carries the relay's full callsign for identification.

## 10. Message rules

1. **Id**: origin callsign plus a counter (already the backbone's format). Everything (doubles,
   receipts, retries) hangs on it.
2. **Doubles**: the server keeps ids for 50 hours; the recipient's app keeps them too.
3. **Lifetime**: 48 hours by default; the sender sees "Could not deliver" after that.
4. **States the sender sees**: Waiting (no relay or channel yet), Sending, At relay, With their
   relay (the delivering relay has it), Delivered, Read (if the recipient allows it), Could not
   deliver (with the reason).
5. **Sizes**: chat up to 1 kB, mail up to 4 kB of text (backbone limits today); attachments and
   files go direct only, on data channels, as now.
6. **Urgent** messages jump every queue and use the short back-off; they are rate-limited hard.

## 11. When things go wrong

| What happens | What the network does |
|---|---|
| Two relays hear the same message | first to submit owns it; the server tells the other to drop it |
| The recipient moves to another relay's area | presence updates from heard frames; the next delivery goes through the new relay |
| The recipient is off for two days | mailbox; beacon codes; delivered when they appear; expires after 48 h with a notice |
| Band closes (dusk, solar storm) | measurements show it; the plan moves relays to the band that is open; queued items wait |
| A non-KAHF signal sits on a channel | sense before sending; relay reports it; server moves that relay |
| Two operators send at once | they are in a relay area: request slots and grants; outside: back-off |
| Hidden stations | grants are heard around the relay, so both keep quiet |
| Relay crashes mid-transfer | disk custody; the sender resends after a minute if "At relay" never came |
| Relay loses internet | becomes "local only" (9.5); its operators move |
| Server down | plans valid 30 min; relays queue; ids sort out doubles later |
| Wrong clock on an offline computer | first beacon fixes it; band clock tolerates about 30 s |
| Same callsign used by two keys | server keeps the first key (existing rule); messages from the other key are marked |
| Someone floods | per-callsign limits at the relay and the server |
| Contest weekend, bands full | sensing yields; the planner moves to quieter channels; delivery is slower but nothing is lost |
| A station hears its own transmission (sound-card loopback) | own echo is ignored for a short tail after each send, shorter than the answer gap, so answers are never thrown away (fixed in this branch) |
| No relay hears the recipient anywhere | reach-out in the next windows; after 48 h, "Could not deliver" |

## 12. Security and abuse

Relays authenticate to the server with their key; every message is signed by its sender; the
server cannot forge messages, only route them. Rate limits at relay and server. The server can
suspend a relay or a callsign. Nothing is encrypted, as amateur rules require.

## 13. How we prove it works

1. **Simulator on virtual time.** The many-station simulator (`sim.rs`) gets a virtual clock, a
   map of areas, sky-wave footprints by band and hour, and a fake server. A full simulated day of a
   hundred relays and a thousand operators should run in minutes, not a day. The scenarios in
   NETWORK-SCENARIOS.md and section 11 become tests there. This also fixes the slow tests: today
   every daemon test runs on the real clock.
2. **Two-app tests** over BlackHole on one Mac (the owner's set-up), now with a fake relay and a
   local copy of the server.
3. **Soak**: a week of the simulator with relays failing, bands closing and floods, checking that
   no message is lost, none is delivered twice, and the delivery times stay inside section 7.1.
4. **On the air**: two real relays and a handful of operators before anyone else relies on it.

## 14. Build order

1. Virtual time in the simulator, and the section 11 scenarios as tests (fast tests come first:
   everything else is measured with them).
2. Server push (WebSocket per relay) and presence ("who hears whom").
3. Bundles and ordered answers on the air.
4. Relay as referee: beacon schedule, request windows, grants.
5. Planner: clash graph, channel colouring, time sharing, plans with expiry.
6. Band clock for offline operators; mailbox codes in beacons.
7. Reach-out windows.
8. Propagation learning from measurements, plus solar data.
9. Frequency tables for all three regions, after the numbers in section 5 are checked.

## 15. Decisions for the owner

- Which bands to enable: 80 m and 40 m (local), 20 m (reach-out), and whether to add 17 m and
  15 m. More bands mean more channels and more reuse; each needs a checked frequency window.
- The exact frequencies, after checking the IARU and national plans (section 5.2), including moving
  today's 40 m channel off the PSK31 frequency.
- Message lifetime (48 h proposed) and whether "Read" receipts are shown (off by default proposed).
- Whether the "emergency relay" option for operators (8.5) should exist at all.

---

# Part 2. The same thing, simply

**Three kinds of station.**
- **Operators** are ordinary hams. They need a radio and a computer, nothing else.
- **Relays** are hams who leave a station running in relay mode. They must have internet.
- **The server** (kahf.dev) is the brain.

**How a message travels.** Anna writes to Bo and presses Send. Her radio sends it to the nearest
relay. That relay passes it over the internet to the relay nearest Bo. That relay sends it to Bo's
radio. Bo's radio says "got it", and that tick travels back to Anna the same way. Radio is only used
at the two ends, because air time is slow and shared, and the internet in the middle is instant.
That is why a message from Europe to China takes seconds, not hours.

**What Anna sees.** Sent, then At relay, then Delivered. Each tick has a time. If it really cannot
be delivered within two days, she is told why.

**If they can hear each other,** the message goes straight from Anna to Bo, with no relay.

**If Bo has no relay near him,** the server finds a relay far away that can reach Bo's area at that
hour by bouncing off the sky. At set times every ten minutes, every radio listens for such calls,
so Bo can be found even though his computer is offline.

**The server decides everything.** Which relay looks after which operator, which band and channel
each relay uses, and when. Who delivers which message, and who ignores it. When to give up.
Relays just follow the plan; operators just follow simple built-in rules.

**How channels don't clash.**
1. Most traffic uses 80 m and 40 m, which only reach a few hundred kilometres by day. So the same
   channel can be used in many countries at once without anyone hearing each other.
2. The server works out which relays could hear each other and gives them different channels.
3. Where there are too many relays for the channels, they take turns, a few seconds each.
4. Inside one relay's area, the relay is the referee. You ask with a tiny signal, it says "go",
   and everyone else waits. So two people never talk over each other.
5. Every radio always listens before it sends, and backs off if anything is on the channel,
   KAHF or not.

**Which frequencies.** Only small windows inside the parts of each band meant for this kind of
data traffic. KAHF stays well away from FT8, FT4, WSPR, JS8, PSK31, the beacon network, emergency
frequencies and SSTV. The exact numbers must be checked against the official band plans before
anyone transmits. One thing to fix: today's 40 m channel starts on the usual PSK31 frequency.

**Your batching idea.** A relay never waits to collect messages, because waiting makes the first
one slower. Instead, every time it gets to send, it sends everything that is waiting in one go:
messages, "delivered" ticks and "you have mail" notices for many people. Each person's radio picks
out what is for them and ignores the rest. Then they answer in turn, in the order the bundle lists
them, so their answers never collide.

**Operators without internet.**
- Their app knows where to listen from the clock and the sun: 40 m by day, 80 m by night.
- Once they hear a relay, they follow that relay's schedule.
- If someone has mail waiting, every relay that could reach them says a short code for their
  callsign in its regular announcement. When their radio hears its code, it asks for the mail.

**Relays run themselves.** Switch on relay mode once. After that, the relay:
- changes band and channel by itself, through the radio's control cable;
- saves every message to disk before saying "got it";
- restarts itself if something hangs.

If it loses the internet, it stops being a relay and tells everyone, and its operators move to
another relay.

**If something breaks,** messages wait instead of disappearing. Every message has an id, so
sending it again can never make a duplicate.

**How fast.** About 5 to 20 seconds to reach the other person when both are near a relay. About 10
to 35 seconds until you see "Delivered". Weak signals and "no relay nearby" take longer.

**What you need to decide.**
- Which bands to use.
- The exact frequencies, after checking the band plans.
- How long a message may wait (two days proposed).
- Whether operators may ever act as emergency relays.

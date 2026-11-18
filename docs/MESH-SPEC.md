# KAHF mesh: the specification (17 m, server-planned, no stored messages)

Written 2026-11-18. This is the authoritative design. It **supersedes docs/MESH-DESIGN.md** (the first
draft) and the parts of NETWORK-DESIGN.md / NETWORK-SCENARIOS.md / NETWORK-CHANNELS.md that disagree
with it. Those three still hold as scenario lists and as background.

Nothing here is built yet unless a line says **BUILT**. Numbers marked **(est.)** are estimates from
the modem's frame times, not measurements. Numbers marked **(verify)** must be checked before anyone
transmits. Read section 0 and section 1 first; they say what the owner decided and what is true about
17 m.

---

## 0. Owner decisions (binding, in the owner's words where it matters)

1. **Band: 17 m only, for now.** About 10 channels: 6 to 8 for relays, 2 for operators calling.
2. **Relays must be connected to the server, always.** Operators may be offline. The server
   calculates "what should transmit when and where, and who should ignore which packets".
3. **No stored messages.** The owner does not want messages kept in the network. (Section 7.)
4. **Hopping is cool.** Keep it in the design. For now build: operator to relay, a *different* relay to
   the receiver, and direct operator to operator. Relay-to-relay over the air is designed (section 9)
   but built later.
5. **Autonomous.** A relay owner switches relay mode on once. An operator types a callsign and
   presses Send. Nobody selects band, channel, relay or time.
6. **Fast.** The message and its confirmation must come back as fast as the air allows.
7. **Never transmit over anyone.** Offline operators must not clash with other traffic on the
   frequency. Relays must never clash with each other or with themselves. This is the main risk and
   gets the most design (section 5).
8. **Be thorough and advanced.** The owner expects a complex system and has handed the design over.

Reading rule for the next session: when this file and an older document disagree, this file wins.

---

## 1. What is true about 17 m (investigated, with sources)

### 1.1 The band plan (IARU Region 1, June 2016, effective 01 June 2016)

Source: `iaru.region1.hf.pdf` (copy on hamwaves.com), read 2026-11-18. The file was deleted after
reading. **(verify)**: this is the 2016 text. Later IARU Region 1 meetings may have changed it. Before
any transmission, compare with the newest plan (UBA lists it on https://www.uba.be/en/hf/bandplanning).

| Segment (kHz, transmitted frequency) | Max bandwidth | Use in the plan |
|---|---:|---|
| 18068 - 18095 | 200 Hz | CW; 18086 QRP centre of activity |
| 18095 - 18105 | 500 Hz | narrow-band digimodes (attended only: not marked for unattended) |
| 18105 - 18109 | 500 Hz | narrow-band digimodes, **automatically controlled data stations (unattended)** |
| **18109 - 18111** | - | **IBP, exclusively for beacons. KAHF never goes here.** |
| **18111 - 18120** | **2700 Hz** | **all modes, digimodes, automatically controlled data stations (unattended)** |
| 18120 - 18168 | 2700 Hz | all modes (SSB); 18130 SSB QRP; 18150 digital voice centre of activity |

Facts that decide the design:

- **KAHF's relay lanes go in 18111 - 18120 kHz.** It is the only 9 kHz block that is marked for
  unattended data, allows 2700 Hz, and sits clear of the common digital frequencies.
- The plan's frequencies are **transmitted frequencies**, not dial settings. For USB the radio's dial
  is the transmitted frequency minus the audio frequency (section 2.3 has the dial table).
- "Automatically controlled data stations **includes store-and-forward stations**." Relays are that.
- The plan **asks member societies to limit unmanned HF stations**, and recommends that unmanned
  transmitting stations are *activated under operator control*, except agreed beacons and specially
  licensed experimental stations. They must obey the plan's frequency and bandwidth limits.
  **A 24/7 unattended relay is therefore a licensing question in every country** (section 12, decision
  D1). The design includes an operator-presence mechanism (5.9) so a relay can honestly be "activated
  and supervised by its operator".
- "The operator connecting to an automatically controlled unmanned station is responsible for not
  causing interference." The app must make that responsibility easy to meet (sensing, 5.5).
- 17 m **may not be used for contests** (Region 1 recommendation, ratified or not: **(verify)**), and
  non-contesters are told to use it during contests. So it is quieter than 20 m, and busy periods are
  fewer, but not zero.
- Today's code (`crates/kahf-core/src/bandplan.rs`) has a 20 m segment at 14.101 - 14.112 MHz and a 40 m
  one at 7.040 - 7.045 MHz. The owner reports **VARA and packet already use 14.101 - 14.112**. The 7.040 start
  sits on the usual PSK31 frequency. Both are wrong for KAHF and are replaced by the 17 m plan.

### 1.2 What else lives on 17 m (kept clear of)

Usual dial frequencies (USB) of other activity, from memory **(verify)**:

| Activity | Dial (MHz) | Occupies about |
|---|---|---|
| FT8 | 18.100 | 18.100 - 18.103 |
| FT4 | 18.104 | 18.104 - 18.107 |
| WSPR | 18.1046 | 18.1060 - 18.1062 |
| IBP beacons | 18.110 | 18.110 (CW, time shared) |
| SSB, digital voice (18.150) | - | 18.111 upward: **the plan's 2700 Hz "all modes" also allows SSB at 18.111 - 18.120**, so listen before every transmission and expect the odd voice QSO. |

All of FT8, FT4 and WSPR sit **below** 18.108. That is why the plan here starts at 18.111.2.

### 1.3 What 17 m can and cannot do (physics, stated plainly)

17 m is a **long-distance daytime band**. This matters more than anything else in this document, so it
is stated without softening:

1. **No short links.** At 17 m the signal usually jumps over the first several hundred to roughly a
   thousand kilometres (the skip zone). An operator 100 to 600 km from a relay will often hear
   nothing. The links that work are roughly **1000 to 4000 km** by sky wave, and the band is open for
   most of the daylight hours on the path, much less at night. **(est.)** Exact skip distance depends on
   the solar cycle; the server must measure it (section 6.3), not assume it.
2. **Closed most nights.** Solar cycle 25 is past its peak (late 2026 is on the declining side), so
   17 m openings are shrinking. Expect it to close soon after local dusk on a path and open again
   after dawn. **(verify)** with live solar flux and MUF data.
3. **Consequence for the owner's idea "a relay in every European country".** Inside one country the
   operator-to-relay link is usually *inside the skip zone* on 17 m. The network works with relays
   **spread 1000 to 3500 km apart** (so every operator is in sky-wave range of several relays and in
   the skip zone of the nearest), not one per country. The server plans this (6.2). A single-band 17 m
   network is a **daytime long-haul network**. Short links and night need 40 m and 80 m, which the
   design carries as a one-line configuration (section 3.1 is data-driven), and the owner should expect
   to add them. This is decision D2 in section 12.
4. **Other IARU regions have different 17 m rules. (verify, not yet read.)** The US allows data only up
   to 18.110 MHz (ARRL listings, FCC), which excludes 18.111 - 18.120. Region 3 (China, Australia) is
   different again. A Europe to China or Australia message crosses regions only through the *internet*
   leg of the path, so each operator must use the lanes allowed in their **own** region: the lane table
   is per region and data (3.1). Region 2 and 3 tables are not yet written. Fetch the two PDFs from the
   UBA page above and add their rows before enabling those regions.

### 1.4 How many channels this gives

| Block | Width | At today's 2.4 kHz signal | At a 480 Hz signal on a 700 Hz pitch |
|---|---:|---:|---:|
| 18111 - 18120 | 9.0 kHz | **3** | **12** (with 200 Hz at the bottom and 620 Hz at the top kept clear) |
| 18105 - 18109 | 4.0 kHz | 0 (500 Hz limit) | 5 on paper, **about 2 to 3 clear of FT4/WSPR**: not used in this plan |

So the owner's "about 10 channels" needs a **narrow lane mode**. It does not exist yet. Section 2 says
what it must be.

---

## 2. The channel plan

### 2.1 Twelve lanes in 18111 - 18120 kHz

- Lane pitch **700 Hz**, signal width **480 Hz**, so **220 Hz of empty space** between neighbours.
- The lowest lane starts 1.2 kHz above the IBP beacon segment. The highest ends 620 Hz below 18.120.
- Roles. The spare lanes double as buffers between the calling lanes and the busy relay lanes:

| Lane | Role | Lane | Role |
|---|---|---|---|
| 0 | R1 (relay) | 6 | **C2 (calling)** |
| 1 | R2 | 7 | X2 (spare / buffer) |
| 2 | R3 | 8 | R5 |
| 3 | R4 | 9 | R6 |
| 4 | X1 (spare / buffer) | 10 | R7 |
| 5 | **C1 (calling)** | 11 | R8 |

- **Relay lanes R1 to R8**: the downlink (relay to operators), data transfers, and later relay-to-relay hops.
- **Calling lanes C1, C2**: operator requests, paging, relay beacons, short acknowledgements, urgent messages. They are
  kept **short**: no one holds a calling lane for more than 6 seconds.
- **Spare lanes X1, X2**: (a) guard space; (b) **bonding**: a strong link can borrow a spare lane
  and its neighbours to send a large transfer faster (2.5); (c) emergency; (d) where the plan moves a
  relay that is being interfered with. A relay never uses X1/X2 without a plan from the server.

### 2.2 The exact table (computed from the plan above)

Frequencies in MHz. "Dial" is the USB dial setting that puts the lane's centre at **1500 Hz audio**
(the audio carrier used by the modem today).

| Lane | Role | Lower edge | **Centre** | Upper edge | **USB dial** |
|---:|---|---:|---:|---:|---:|
| 0 | R1 | 18.111200 | 18.111440 | 18.111680 | 18.109940 |
| 1 | R2 | 18.111900 | 18.112140 | 18.112380 | 18.110640 |
| 2 | R3 | 18.112600 | 18.112840 | 18.113080 | 18.111340 |
| 3 | R4 | 18.113300 | 18.113540 | 18.113780 | 18.112040 |
| 4 | X1 | 18.114000 | 18.114240 | 18.114480 | 18.112740 |
| 5 | C1 | 18.114700 | 18.114940 | 18.115180 | 18.113440 |
| 6 | C2 | 18.115400 | 18.115640 | 18.115880 | 18.114140 |
| 7 | X2 | 18.116100 | 18.116340 | 18.116580 | 18.114840 |
| 8 | R5 | 18.116800 | 18.117040 | 18.117280 | 18.115540 |
| 9 | R6 | 18.117500 | 18.117740 | 18.117980 | 18.116240 |
| 10 | R7 | 18.118200 | 18.118440 | 18.118680 | 18.116940 |
| 11 | R8 | 18.118900 | 18.119140 | 18.119380 | 18.117640 |

Rule for the code: **never hard-code these**. Store them as signed data (3.1). The generator is
`lane(k) = lower_k = 18_111_200 + 700 * k Hz; centre = lower + 240; dial = centre - 1500`.

### 2.3 The dial-frequency trap

Radios label frequencies as the *dial* (suppressed carrier) in USB. The band plan limits are
*transmitted* frequencies. If the app tunes a radio's dial to a lane's centre by mistake, the signal lands 1.5 kHz
too high, on top of the next four lanes. **The app must compute the dial from the centre, show both,
and refuse to transmit if the dial implies an occupied span outside the lane table.** Existing
`allowed_segment()` in `bandplan.rs` has the same dial/transmit ambiguity and must be rewritten to take
the transmitted span.

### 2.4 The narrow-lane modem ("KAMI-N")

A lane is 480 Hz wide. Today's KAMI occupies 2.4 kHz (chip rate 2000/s). The simplest narrow mode is
the same waveforms at **one fifth of the chip rate (400/s)**: occupied width 480 Hz, five times
slower. That is a useful first target (the owner can already see it in the 18-lane .wav made during this session,
which was exactly that). Estimated speeds for it, from `docs/KAMI-MODES.md` net rates divided by five
**(est.)**:

| Mode | Net bit/s | 100 byte chat | 3 kB mail |
|---|---:|---:|---:|
| 1c (needs +9 dB) | 986 | 0.8 s | 0.4 min |
| 1a (needs 0 dB) | 353 | 2.3 s | 1.1 min |
| 8 (needs -2 dB) | 137 | 5.8 s | 2.9 min |
| 16 | 92 | 8.7 s | 4.3 min |
| 32 | 57 | 14 s | 7 min |
| 256 (needs -16 dB, weakest) | 8 | 98 s | 49 min |

Slowing by five is the worst case. Narrowing *also* cuts the noise by a factor of five (7 dB), so a
purpose-built narrow mode that uses the extra 7 dB does better: Shannon limits say a weak link keeps
**90 percent** of its speed in 480 Hz, a mid link about **65 to 75 percent**, a strong link about **40 percent**
**(est., upper bounds)**. **Build order for the modem:** (1) a 5x slowed KAMI as the first "lane mode"
(reuses everything, tests the whole network); (2) a re-optimised narrow mode with its own mode ladder;
(3) lane bonding (2.5) for strong links, which restores speed where it is lost most.

Requirements for any lane mode: occupied bandwidth **at most 480 Hz** at the -26 dB points, **adjacent
lane leakage at least 40 dB down** at the 700 Hz pitch (measure it in `kahf-sim` before use; if it is
worse, widen the pitch to 800 Hz and accept 10 lanes, which is still inside the owner's range), no
spurs outside the lane, and a preamble that works at -16 dB.

### 2.5 Lane bonding (speed for strong links)

A relay can be granted **2, 3 or 4 adjacent lanes** (spares included) for one transfer. They are sent as
**one wider signal**: the chip rate scales with the occupied width, so speed scales with it too.

| Lanes bonded | Occupied width (n x 480 + (n-1) x 220 Hz) | Speed versus one lane |
|---:|---:|---:|
| 1 | 480 Hz | 1x |
| 2 | 1180 Hz | about 2.5x |
| 3 | 1880 Hz | about 3.9x |
| 4 | 2580 Hz | about 5.4x (this is today's 2.4 kHz KAMI, which already fits the 2700 Hz cap) |

Five lanes would be 3280 Hz and is **not allowed** (2700 Hz cap). A bonded grant is made only when the
link's measured SNR can use a fast mode, the lanes are all free in the plan, and the transfer is larger
than a few frames. Bonding shortens how long lanes are busy, so it raises everyone's speed, not just the
sender's. Numbers are **(est.)**.

---

## 3. Data that drives everything

### 3.1 Band table (signed, versioned, region-specific)

```
BandTable { version, region: R1|R2|R3, valid_from, signed_by: ProjectKey,
  bands: [ { name: "17m", segments: [ { lo_hz, hi_hz, max_bw_hz, unattended_ok } ],
             lanes: [ { k, role, lower_hz, centre_hz, dial_hz } ],
             excluded: [ { lo_hz, hi_hz, why } ],          // IBP, FT8, FT4, WSPR, ...
             rules: { sense_ms, max_tx_s, duty_cycle_pct, ... } } ] }
```

The server signs it. The app carries a built-in copy for offline operators and refuses an unsigned
or older one. Adding 40 m, 80 m or 20 m later means adding rows, not changing code. The code must
check every transmission against the table, **never against constants**.

### 3.2 Time

All timing is **UTC epochs**. `epoch = floor(utc_seconds / 12)`; a **slot** is 2.0 s with a 0.4 s guard,
six slots per epoch **(tune by measurement)**. Relays take UTC from the server over the socket (offset
by half the round-trip, checked every 30 s) and must be within **50 ms**; a relay that is out by more
than 150 ms **does not transmit** (5.4). Offline operators set their clock from the first relay beacon
they hear (the beacon carries the epoch counter and the sub-second offset); they are good to about
**250 ms**, which the 0.4 s guard covers.

---

## 4. Roles and what each knows

| | Operator (maybe offline) | Relay (always online) | Server |
|---|---|---|---|
| Needs internet | no | yes, always | is it |
| Sends on air | when granted or on a calling lane after sensing | only in slots the plan gives it, after sensing | never |
| Knows | its own outbox, relays it has heard, its own keys | its plan, its queue (RAM), the operators it hears | the whole picture, but **no message bodies** |
| Decides | nothing beyond built-in rules | nothing beyond the plan | everything central |

**Server components (Cloudflare):**

- **Directory** (Durable Object per region): relays, keys, lanes, status, heartbeats.
- **Planner** (Durable Object, one per *clash domain*, section 5.2): builds epoch plans. A Cron
  Trigger is only the watchdog; the planner runs on its own alarms every epoch.
- **Router** (Durable Object per region): `route(dest)`, paging, presence.
- **Presence** (in memory with a short TTL, hashed callsigns only): which relay heard which operator,
  on which lane, at what SNR, when.
- **Propagation learner** (Durable Object + D1 for aggregates): per relay pair and per operator
  region, measured SNR by hour; plus NOAA space-weather inputs.
- Everything that touches a message handles **only ids, sizes, hashes and routing facts**. Bodies
  pass through memory and are never written to disk (section 7).

Each relay holds a **WebSocket** to its region's Durable Object. The server **pushes** plans,
deliveries and cancels. Heartbeat every 30 s, with a fallback to HTTPS polling every 10 s if the
socket is down.

---

## 5. How channels never clash (the heart of the design)

The owner's main worry. Eight rules, enforced in layers. The first four are *by construction* (cannot
clash if followed); the rest are *safety nets*.

### 5.1 Invariants (what must always be true)

- **I1.** In any clash domain, **at most one KAHF transmitter per (lane, slot)**. The planner
  guarantees this when it builds a plan.
- **I2.** An **operator** transmits only (a) on a calling lane, in a slot the nearest relay's
  beacon marked as an operator window, or, if no beacon is heard, in the default operator window
  of the *band clock* (5.7); (b) on a relay lane, only in the exact slot a grant assigned to it.
- **I3.** **Sense before every transmission, always** (5.5), including for relays that have a grant.
- **I4.** **Silence is the safe default.** No valid plan for this epoch, a clock error over 150 ms, a missed
  heartbeat, an unsigned or stale band table: the station does not transmit.
- **I5.** **One instance per callsign.** The server allows one live socket per relay key. A second
  connection from the same key disconnects the first (the first stops at once, 5.9).
- **I6.** **Limits.** Maximum transmit time per slot run, per station per hour, and per lane per
  minute, from the band table (`max_tx_s`, `duty_cycle_pct`). A hung PTT is cut after `max_tx_s` by
  a hardware-independent software watchdog.
- **I7.** **Frequency and mask.** Only table lanes, only inside the table's segments, with the occupancy
  of the transmitted signal inside the lane (2.4 requirement).
- **I8.** Every transmission carries the sender's callsign (in the frame header, and as CW or voice ID
  where a country requires it).

### 5.2 Clash domains and the plan (relays never talk over each other)

A **clash domain** is a set of relays that can interfere on the same lane at the same hour. Two relays
are in the same domain if **either** of these is true:

- one can hear the other on that lane (measured SNR above a threshold), **or**
- some operator hears both (measured by `Heard` reports).

On 17 m during the day most of Europe is one domain, because of the long paths. At night, or when the band
closes, the domain splits into small ones (or disappears). The server recomputes the graph **every hour
and whenever propagation changes** (the learner pushes a "footprint changed" event).

The planner turns the graph into **epoch plans**:

1. **Colour the graph** with the 8 relay lanes: relays that clash get different lanes. With few relays,
   most relays get a private lane for the day.
2. If a domain has more relays than lanes, **time-share the surplus**: the extra relays take turns on a
   lane, a slot at a time, in proportion to their queue length.
3. **Demand-driven slots.** Each relay reports its queue (items, bytes, urgency) in every heartbeat and
   ahead of each epoch. The planner gives slots to relays that have something to send and takes them
   from relays that do not. An idle relay owns **no** slots, so an idle network costs no latency, and a
   busy relay gets almost the whole lane. This is what stops the "only one transmits for hours" problem.
4. **Plans are published 3 s before the epoch they cover**: `Plan { epoch, domain, grants: [ {relay, lane,
   slot_from, slot_to, purpose} ], signed }`. Relays act only on signed plans.
5. **Plans for the next four epochs are published together** (48 s lookahead). If the server stops, every
   relay keeps transmitting by the plan it holds for up to 48 s, then **falls silent (I4)**, not into a
   free-for-all.
6. **Deterministic fallback.** The plan generator is deterministic from its inputs (relay list, lanes,
   hour, a seed). The server also publishes those inputs every 10 minutes. If the server is down for
   more than 48 s, every relay can compute the **same** plan locally from the last published inputs and
   continue at a **reduced duty cycle (25 percent of normal)** with sensing. That is a degraded mode,
   not a normal one, and is labelled in the beacon (**"local plan"**).

Why this is fast: lanes run in parallel (8), idle relays take no time, relays announce queue
length so grants follow demand, and the internet carries everything between relays (7.1).

### 5.3 The operator side: calling lanes and request slots

Operators are the only stations that act without a plan. They are controlled by **three** things.

1. **Where.** C1 and C2 are the only lanes an operator may use unprompted. Which of the two: an
   operator hashes its callsign (`H(call) mod 2`) and uses that one unless the relay's beacon names
   another. That halves collisions without coordination.
2. **When.** The calling lanes are cut into **request slots** (2.0 s each). The relay's beacon names
   the next *operator window* (a run of slots) and its length, and says how loaded the lanes are
   (`load` 0 to 255). An operator picks a **random slot** in the window, and after a collision backs off
   with a window that doubles each time (up to 8 slots), with jitter.
3. **How much.** A request is **one codeword** (about 1.5 to 2 s at 1a in a lane **(est.)**): who it is
   for, how many bytes, urgency, SNR estimate, and the operator's ids. A collision costs one codeword,
   never a whole message. The relay answers each decoded request with a **grant**: lane, slots, mode.
   Everyone who hears a grant keeps quiet for its duration (virtual carrier sense), and the grant is
   heard around the relay, which also solves the hidden-operator problem (two operators who cannot hear
   each other).

An operator may transmit on a relay lane **only** with a grant. So uncontrolled traffic is confined to two
lanes and short frames.

### 5.4 Relays never transmit on top of themselves or each other

All of these are required:

1. **Plan only.** A relay transmits only inside a grant that names its callsign, lane and slot, in a
   plan whose signature checks.
2. **Clock gate.** Offset from UTC within 150 ms, measured in the last 60 s, else silence (I4).
3. **One transmitter per radio.** The radio driver exposes a single `Tx` token. A second request for the
   token while it is held is refused, and logged as a software error. A relay cannot overlap its own
   transmissions, and cannot switch lanes while the token is held.
4. **Self-hearing.** A station's own echo (BlackHole, VOX leakage, a sound card loop) is **never decoded
   as a message** and is discarded by sample position: the echo window after each transmission must be
   shorter than the answer gap (fixed in this branch: `ECHO_TAIL` is 200 ms and `SIFS_MS` is 300 ms, with
   a compile-time check).
5. **Single instance (I5).** The server accepts one socket per relay key. The dropped instance stops at
   once and shows a clear message.
6. **PTT watchdog.** Software cuts the transmission after `max_tx_s`, regardless of what the modem or
   plan says.
7. **Doubles.** If a relay receives its own relayed frame back (loop), it drops it (frames carry a hop
   counter and the origin's relay id).
8. **Conflict report.** If a relay hears **another KAHF transmitter on its own granted lane and slot**,
   it stops, reports `CLASH(lane, slot, heard_relay)`, and the planner treats the two relays as one
   domain at once and re-plans (section 5.2 is self-correcting).

### 5.5 Sensing: listen before every transmission, even with a grant

- **Energy sense.** 300 ms before the slot, measure energy in the target lane **and in the two lanes
  either side** (leakage). If it is more than **6 dB over the lane's noise floor** (noise floor tracked per
  lane with a slow minimum filter), the station **does not transmit**, reports `OCCUPIED(lane)` with the
  measured level, and the planner moves the relay (5.2) and quarantines that lane for that domain with
  an increasing cool-off (10 min, 30 min, 2 h).
- **Foreign signal classes.** The sensing module classifies what it hears: KAHF preamble, voice
  (SSB spectrum shape), CW, FT8/FT4/WSPR-like (periodic, 15 s or 7.5 s), carrier. Voice or CW on a lane =
  a human QSO: the lane and its two neighbours are blocked for 20 minutes and the plan avoids them.
- **Whole-segment scan.** Every minute the app scans all 12 lanes plus the 2 kHz either side of the
  segment and reports a **spectrum summary** (not audio) to the server: this is how the server learns
  which lanes are really clear, region by region, without anyone configuring it. The summary is 12
  numbers a minute, a few bytes.
- **Operators without internet** keep the same sensing on their own, and keep an **occupancy memory**
  (per lane, last 10 minutes), so a lane that was busy a minute ago is not tried first.
- **No KAHF-only assumption.** KAHF yields to **anyone**. If the segment is busy with voice, KAHF waits.

### 5.6 What if the server is not available to an operator (the owner's case)

Operators do not need the server. Their rules (5.3, 5.5, 5.7) are built in and work alone. The only thing
they can lose is *speed* (no grant means a random request slot and back-off), never *safety*.

### 5.7 The band clock (where an offline operator listens and calls)

Every operator app knows, without any message:

1. **The band table** (signed copy in the app).
2. **Time**, from the computer's clock, corrected by the first relay beacon.
3. **Rules:**
   - Idle operators **listen on C1 and C2** (both: two receivers in software, since the audio is 480 Hz
     wide each; one sound card captures both lanes in the same 2.4 kHz of audio, section 8.3).
   - **Paging windows** (6.3): the first 20 seconds of every 10th minute of the UTC hour (minutes 0, 10, 20,
     30, 40, 50). Every idle operator is listening on C1 and C2 then. To spread the load, relays page in
     different epochs inside that window (the plan assigns them), not all at second 0.
   - **Operator windows** for requests are announced by relay beacons. Without a beacon: slots 0 to 2 of
     each epoch whose number is a multiple of 5 **(tune)**.
4. **A closed band.** When the learner or the built-in sun model says 17 m is closed on the path to the
   nearest known relay, the app says so ("17 m is closed here until about 06:40 UTC") and keeps the
   outbox. It does not transmit into a closed band. It still **listens** (to learn when it opens).

### 5.8 Pairs of relays and the "relay can hear the receiver" case

The owner's scenario: the sender cannot reach the receiver, can reach a relay, and that relay can reach
the receiver. This is the **normal path** (section 8). The server decides which relay delivers, from
presence (6.1). The sender never has to know who the receiver's relay is (section 6, the both-offline
case).

### 5.9 Operator supervision (licensing)

Because of the plan's wording on unmanned stations, relay mode includes a **supervision mechanism** that
the owner can choose to require (decision D1):

- The relay's operator sees a **status screen** and a **"stop relaying" button** (BUILT: the Relay
  screen exists in the app).
- Optionally a **daily confirmation**: if the operator has not confirmed in the last 24 h, the relay
  stops transmitting (it keeps listening and still reports). One tap to confirm.
- The server can **suspend** a relay at once; a suspended relay stops within one epoch.
- Every beacon carries the relay's callsign, and the app announces its identity at least every 10
  minutes, as most licences require.

---

## 6. Routing: how the message finds the receiver

### 6.1 What the server knows (and does not)

Presence record (RAM, TTL 6 h, extended while heard): `H24(callsign)`, set of `(relay_id, lane, SNR,
time)`. It stores **the 24-bit hash**, not the callsign, plus the full callsign only for operators who
have registered it (so the server cannot trivially list who is listening where). `H24 =
first 3 bytes of SHA-256(upper-case callsign)`. Hash collisions (about 75 pairs worldwide among 50 000
operators, **est.**) are harmless: a hash only *wakes* an app, and the follow-up carries the full callsign.

### 6.2 The normal path

1. Sender's app hears relay A's beacon (calling lane). It sends a request: `dest callsign (6 bytes), message id,
   size, urgency`. A grants a relay lane and slots.
2. The sender transmits the message. A **acknowledges within 300 ms** ("at relay A", with an ack bitmap
   for any frames that need repeating).
3. A asks the server: `route(dest)`. The server answers with the best delivering relay B (presence,
   propagation, load) **in under a second**, and A pushes the message to B over the internet. A deletes its
   copy **only after B confirms receipt** (the hand-over rule, 7.2).
4. B asks the plan for a downlink slot on a relay lane, delivers to the receiver (or pages first, 6.3),
   and the receiver acks **in its place in B's ordered ack list** (7.3).
5. B sends a receipt to the server, the server pushes it to A, and A transmits the receipt to the sender
   in its next downlink (batched with anything else for the sender, 8.3).

If A and B are the **same relay** (both operators near one relay), the internet leg is skipped.

### 6.3 Both operators offline: how does the message find the receiver?

The sender knows only the relay it can hear (A). It knows nothing about the receiver. This is solved by
**the server's presence table plus paging**:

- **Known** (receiver heard by some relay in the last 6 h): the server names the best relay B and
  delivers as in 6.2. B pages the receiver's hash first (below), because "heard 4 hours ago" does not
  mean "listening now".
- **Unknown or stale**: the server **pages the callsign** from every relay that can plausibly reach the
  receiver's region. "Plausibly" comes from the propagation table (which relay pairs with which area
  at this hour). Relays page in the **paging windows** (5.7) on the calling lanes, each in its own slot
  from the plan (so pages do not clash). A page is a **beacon frame with a list of 3-byte hashes**
  (eight hashes per codeword **(est.)**). The first relay that hears a reply (the receiver's app answers on
  a calling lane in the next operator window: "here I am, heard relay B, SNR x") reports it, the server
  cancels the other pages, and delivery proceeds as in 6.2.
- **Not found** after **three paging cycles** (about 30 minutes): the sender is told **"receiver not
  reachable right now"**. The message **stays in the sender's own outbox on the sender's computer** (the
  owner's rule: nothing is stored in the network), and the app **retries automatically** on the
  receiver's likely listening pattern (the app learns it from the receiver's past replies; the server
  never sees message content) until the message's lifetime (default 24 h) ends. The sender sees
  "Waiting for receiver" with the next try time, and can cancel.
- **Both offline, receiver's computer off:** same result: "Waiting for receiver". When the receiver
  switches on and hears a beacon, it announces itself (`HERE`), the server tells the sender's relay (if
  still in play) or the next paging cycle finds it.

**What the receiver's app does so that it is findable:** it listens on C1 and C2 whenever it is on; in
every paging window it checks for its hash; whenever it hears a relay beacon it replies `HERE` on a
calling lane once per 10 minutes (so the server has fresh presence even without a message), unless the
operator disabled presence.

### 6.4 Choosing the delivering relay

`score(B) = P(deliver in 1 attempt) * freshness(B, dest) / (1 + queue(B)/capacity(B))`, where
`P(deliver)` is the learned success rate of B to dest's region on this lane at this hour (Bayesian
average with a prior from the sun model, 6.5), `freshness` decays with the time since B last heard dest.
The server picks the best, and the **second best is prepared** (warm) for failover.

### 6.5 Propagation: model plus measurements

- **Model (day one):** great-circle distance and the sun's elevation at the path's midpoint, plus the
  current solar flux and K index (NOAA, hourly) set a first guess of whether 17 m works over the path.
  Daytime paths of 1000 to 4000 km are "likely open", short paths "skip zone", night "closed" **(est.)**.
- **Measurements (from the first hour):** every decoded frame, every beacon heard, and every grant outcome is
  a data point `(relay or region A, relay or region B, lane, hour of day, SNR, success)`. The learner keeps an
  exponentially weighted average per pair and hour of day, so the model is replaced by what the network
  actually hears, pair by pair, with a slow forget (about 7 days).
- **Exploration.** For pairs with few samples, the planner sometimes schedules a test beacon (a short
  frame) so the table fills in without waiting for traffic. Test beacons are limited to 1 percent of a
  lane's air time.

---

## 7. No stored messages: how the network delivers without keeping them

The owner does not want messages stored. So:

### 7.1 Cut-through

The server and the relays **do not keep message bodies on disk, ever.** A message enters a relay's RAM
from the air, is pushed over the internet to the delivering relay's RAM, and leaves it on the air. If a
process restarts, in-flight messages are lost, and the **sender's retry** (7.2) repairs that.

### 7.2 Who keeps the copy

**The sender's own app**: it keeps the outbox on the sender's computer (it is the sender's own data, as
in any mail or chat app). The network only holds a message for the few seconds it needs.

- A relay holds a message **in RAM at most 60 s** while waiting for its downlink slot or the next relay's
  confirmation, then drops it and sends a negative acknowledgement back along the path, so the sender's
  app retries (another relay, another time).
- **Hand-over rule:** a relay drops its copy only after the next hop **confirms receipt of every frame**
  (ack bitmap complete). The server never "takes custody": it is a switch, not a mailbox.
- **Receipts** are the only thing that returns: `{id, delivered_at, relay}`; tiny and unlogged.
- **Sender states:** Waiting (no relay, or band closed), Sending, At relay, With the delivering relay,
  Delivered, Waiting for receiver (not reachable, will retry), Could not deliver (lifetime over, with the
  reason). These are the app's own states, stored on the sender's computer.

### 7.3 Duplicate control without storage

The server keeps `H(message id)` in RAM for 2 hours (a few bytes each) to drop doubles, and the
receiving app keeps the ids it has shown (its own data). Neither contains message text.

### 7.4 The consequences, stated honestly

- If a relay crashes with a message in RAM, that copy is gone. The sender retries within about a minute
  because "At relay" arrived but "Delivered" did not within a time that depends on the path.
- Messages to an operator who is offline for days wait **on the sender's computer**, not in the network.
  If the sender's computer is off too, nothing moves until it is on again. **That is the price of storing
  nothing.** An optional "delivery agent" (a relay that keeps a message for a day on behalf of a sender)
  can be added later as an explicit opt-in, per message. It is not part of this plan.
- Message ids and routing metadata (who talks to whom, when) are visible to the server, as they must be
  for routing. This is stated in the privacy text. Nothing is encrypted (amateur rules); every message is signed.

---

## 8. The air protocol

### 8.1 Frame header (new, applies to every frame)

Each codeword's payload (36 bytes) starts with a header that lets everyone who cannot use the frame
**ignore it after decoding the first bytes** (the "who should ignore what" rule, done on the air):

```
byte 0       type (0x30..)   bundle, request, grant, beacon, page, here, ack, data, receipt, ...
byte 1       flags | hop count
bytes 2..4   next-hop id   (24 bits: relay short id, or H24 of an operator, or 0 = anyone)
bytes 5..7   source id     (24 bits)
byte 8       frame index and count of this burst
byte 9..     body
```

Existing tags in `link.rs`: `0x01..0x06` link control, `0x10` data, `0x11` ack. New tags start at `0x30`.
A station that is not the next hop and not the destination **discards the frame after the header** and
does not answer.

### 8.2 Bundles (the owner's "collect and send together", without waiting)

**A relay never waits to fill a bundle. Every time it may transmit, it sends everything that is ready in
one burst** (up to 8 codewords share one preamble, which is already how bursts work). Items: short
messages (up to about 27 bytes of text per item), receipts (about 7 bytes), mail notices, acks, pages.
Each item names its recipient by hash; each codeword is self-contained, so a lost codeword loses only its
own items. Waiting to collect would slow the first item for nothing.

### 8.3 Ordered answers (no collisions on the way back)

After a bundle, recipients answer in **the order the bundle lists them**, each in its own mini-slot
(300 ms apart, starting 300 ms after the burst; `SIFS_MS` in `mac.rs`). Nobody's answer is left to
chance, and a missing answer is a missing item. Acks of several items travel in one codeword.

### 8.4 Two lanes at once

The operator app can receive **two lanes at the same time** from one sound card: both C1 and C2 sit in
the same 2.4 kHz of audio (700 Hz apart, centres at 1500 Hz and at 2200 Hz, not both at 1500 Hz as the
dial table assumes for one lane), so the receiver runs two demodulators on one capture. **The dial table
(2.2) places the lane being *transmitted* at 1500 Hz audio; for a two-lane receiver the dial sits
between them.** The app computes this itself from the band table. Transmitting stays on one lane at a
time (one radio).

### 8.5 Link layer additions

- **Early abort.** A codeword that fails to decode in the first 2 frames of a burst, *and* whose header
  was not for this station, is dropped without further decoding. Saves CPU, not air time.
- **Selective repeat with ack bitmaps** instead of per-frame acks, so a burst of 8 needs one ack.
- **HARQ with chase combining:** a repeated codeword is *combined* with the first copy (add the soft
  bits) before decoding, so a retry succeeds at a lower SNR than a fresh transmission. This is the
  cheapest large reliability gain on HF.
- **Speed from the measured SNR (BUILT for the old modes):** `select` the fastest mode the SNR supports.
  Extended with the lane mode ladder.

---

## 9. Hopping (designed now, built after the first release)

The owner likes it. The design leaves room so it can be added without changing the air format:

1. **Relay to relay over the air** uses the same frame header with `next-hop = a relay id`. The plan gives
   the sending relay a grant on a **relay lane** with purpose `HOP(relay_b)`. Only the named relay acts.
2. **When to hop by radio:** when a relay still has internet but the *server link to the target relay is
   down*, or for **reach-out**: the target operator's area has no relay, but a relay 3000 km away can
   reach it by sky wave; the plan books a long-distance grant for that hour. (Relays must still be
   connected to the server; the owner's rule.)
3. **Passive acknowledgement:** the sending relay hears the next relay forward the message (the hop
   counter increments) and treats that as the ack, so there is no separate ack frame.
4. **Hop limit 4**; loop drop by origin id and hop counter (5.4 rule 7).
5. **Soft-combining diversity** (advanced feature 10.1) works for hops too.

The sub-plan is: build direct and relay paths first (sections 5 to 8), then add `HOP` grants in the
planner and the forward rule in the relay.

---

## 10. The advanced features (why this is faster and more reliable than a plain relay)

1. **Distributed receive diversity (soft combining across relays).** When an operator transmits, **every relay that hears the
   burst, even faintly, forwards its soft bits (LLRs, quantised to 4 bits per bit, 324 bytes per codeword) to
   the server over the internet**. The server **adds them** (maximal-ratio combining) and decodes. A
   frame that **no single relay can decode** can often be decoded from the sum of three. The internet makes this
   almost free: there is no extra air time, and the gain is several dB. It is a large step in
   reliability for the weakest links, which are the slow ones, so it **also speeds them up** (a faster mode becomes usable). Limit: only for relays in the same domain and only for the codewords that fail locally.
2. **Learned propagation (6.5)** replaces guesses with data, per path, per hour, forgetting slowly.
3. **Demand-driven slots (5.2)**: idle relays take no time; busy relays get the lane.
4. **Conversation affinity.** When A sends to B, the planner **pre-books a reply window** around B's next
   beacon in the likely downlink slot, so a reply starts without a new request round.
5. **Predictive paging.** The server learns when each operator tends to listen and pages in that
   window, not at random.
6. **HARQ (8.5)** with chase combining, and **incremental redundancy** (a second transmission sends *new*
   parity, not a copy) for larger gains **(later)**.
7. **Lane bonding (2.5)** for strong links.
8. **Spectrum intelligence (5.5):** all relays report lane occupancy; the planner builds a live map of which
   lanes are clear in which region, automatically avoiding human QSOs, FT8 drift and contests.
9. **Interference-hopping.** If a lane is jammed or occupied, the plan moves the relay to a spare lane
   using a **pseudo-random sequence seeded by the server** (so a deliberate jammer cannot follow).
10. **Self-healing planner (5.2 rule 6):** any relay can run the same deterministic plan if the server
    vanishes, at reduced duty.
11. **Anomaly watchdog:** the server compares reported plans with what relays actually heard; a relay
    transmitting outside its grant is suspended within one epoch.
12. **Cheap time:** relays are time references; operators align to beacon epoch counters, so the whole
    network shares one clock with no GPS on the operator side.

---

## 11. Speed budget (normal path, short chat, both operators near their relay; **est.**)

17 m only, 5x slowed lane modes, a strong link (mode 1a). Moves to 1c when SNR allows; slows to 8 or 16
on weak links.

| Step | Time |
|---|---:|
| Wait for the next operator window | 0 to 6 s (windows follow every beacon; beacon every 30 s) |
| Request codeword + grant | about 3 to 4 s (one codeword each way at 1a in a lane) |
| Message on the air (100 bytes) | about 2.3 s at 1a, 0.8 s at 1c, 5.8 s at 8 |
| "At relay" ack | 0.3 s after the burst |
| Relay to server to relay | under 1 s (WebSocket push) |
| Wait for the delivering relay's slot | 0 to 4 s (demand-driven plan) |
| Downlink bundle | 2 to 6 s |
| Receiver's ack, in order | 0.3 to 2 s |
| **Delivered to the receiver** | **about 8 to 25 s** |
| Receipt back to the sender (bundled, tiny) | 3 to 10 s more |
| **Sender sees "Delivered"** | **about 12 to 35 s** |

On a weak link (mode 16 or lower) add the extra airtime from the table in 2.4. That is the honest cost
of a narrow lane. Soft combining (10.1), HARQ and bonding are the main ways to win it back.

---

## 12. Decisions for the owner (each blocks something)

**Answered 2026-11-18:** D1 = (a) autonomous 24/7, status screen and stop button only, no daily confirmation (the owner says it is allowed where they live; other countries still need their own check). D2 = 17 m only for the first release. D4 = widen to 800 Hz (10 lanes) if the leakage test needs it, and report the measured numbers. D3, D5 to D7 still open.

- **D1. Unattended relays and the law.** The IARU Region 1 plan asks societies to limit unmanned HF
  stations and recommends they be activated under operator control. Choose: (a) fully autonomous, 24/7,
  with the supervision status screen only; (b) autonomous plus a **daily operator confirmation** (5.9);
  (c) transmit only while the operator has confirmed this session. Also needed per country: whether
  automatic stations need a licence (the release notes already say a Finnish unattended relay does).
- **D2. 17 m only means daytime, long-haul.** Short links (under about 800 km) and night are mostly not
  served. Accept this for the first release, or add 40 m and 80 m now (only data rows in the band table;
  the lane logic is band-independent).
- **D3. Regions 2 and 3.** Add their 17 m rows (the US allows data only to 18.110), or run Region 1 only first.
- **D4. Lane pitch.** 700 Hz gives 12 lanes (the plan above). If measured adjacent-lane leakage needs it,
  800 Hz gives 10 lanes (still the owner's range).
- **D5. Narrow mode first step.** Accept the 5x slowed KAMI as the first lane mode (fast to build,
  estimated slow), then re-optimise.
- **D6. Message lifetime and retry.** 24 h default on the sender's computer proposed; "Read" receipts off by default.
- **D7. Optional delivery agent** (a relay holding a message for a day), contrary to the no-storage rule, only if the owner wants it later.

---

## 13. Risks (what can still go wrong)

| Risk | Effect | Answer |
|---|---|---|
| Narrow lanes leak into neighbours at 700 Hz pitch | clashes between our own lanes | measure leakage in `kahf-sim` first; widen the pitch if over -40 dBc |
| 17 m closed for days (low solar activity) | the network is silent | the app says so, keeps the outbox, listens for the opening; this is why D2 matters |
| Someone is on a lane when it is planned for KAHF | KAHF yields | sensing (5.5), quarantine, re-plan |
| The 2016 plan has changed | KAHF transmits where it should not | **verify before any transmission**, band table is signed data and can be changed by the server without an app update |
| Clock error on a relay | two relays overlap | clock gate (150 ms), clash report, suspension |
| Server outage | relays fall to reduced local plan, then silence | 48 s lookahead, deterministic local plan, I4 |
| Hash collision (24-bit) | a wrong app wakes | harmless: the follow-up names the full callsign |
| Hostile or buggy relay transmits off-plan | clashes | server watches reports; suspension within an epoch; signed plans |
| Unattended-station rules | a country may forbid it | decision D1; the supervision screen |
| All narrow modes are 5x slower | long waits on weak links | combining (10.1), HARQ, bonding |

---

## 14. How to build it, in order, with the test for each step

Rules for all steps: tests run on **virtual time** (step 1) so they take seconds; never run audio while
the owner's two BlackHole instances run (`pgrep -fl 'kahf-alpha|kahf-bravo'`); clippy clean; commit with the
repo's `adswill` noreply identity; branch `app-rig-relay`.

1. **Virtual time in `kahf-sim` and the daemon tests.** A `Clock` trait (real or virtual, one source of
   `now`) replacing the 46 `Instant::now()` calls in `kahf-daemon/src/lib.rs` and `Instant`-based pacing
   (`tx_advance`, `BusRadio`). Tests advance the clock instead of sleeping. Acceptance: the existing daemon
   test suite (about 45 s today) runs in under 10 s, with the same results.
2. **Finish the echo bug.** Root cause found in this session (the answer starts inside the 400 ms
   self-echo window, and the test bus put silence inside bursts); fixes are in the working tree, not yet
   verified. See the handover. Acceptance: `one_message_over_an_echoing_channel_arrives_once` and
   `several_messages_each_way_all_arrive_once_in_order_and_none_from_self` pass.
3. **Band table and lane module** (`kahf-core/src/lanes.rs`): table type, signature check, `lane(k)`, dial
   math (2.3), occupied-span check, `allowed()`. Acceptance: unit tests for every number in 2.2, the dial trap,
   excluded ranges, unsigned or older table refused.
4. **Narrow lane mode, first version**: the 5x slowed KAMI as a mode in `kahf-core` (`kami.rs`), with its
   own ladder entries, and a **leakage test** in `kahf-sim` (adjacent lanes at 700 Hz, SNR needed per
   mode with an interferer at +30 dB in the neighbouring lane). Acceptance: the lane signal stays inside 480 Hz
   (-26 dB), the neighbour test passes at the pitch, otherwise report the pitch that does.
5. **Sensing** (`sense.rs` extension): per-lane noise floor, +6 dB rule, signal classes, quarantine.
   Acceptance: simulated voice and FT8-like interferers are detected and avoided.
6. **Frame header and bundles** (`kahf-core/src/bundle.rs`, `frame.rs`): 8.1, 8.2, 8.3 with encode/decode and
   ignore rules. Acceptance: a bundle for 5 recipients is decoded by each, ignored by others, answers in order.
7. **Planner** (server `server/src/planner.js`, plus a Rust twin in `kahf-core` used by the relay's local plan):
   clash graph, colouring, demand-driven slots, signed epoch plans, lookahead, deterministic fallback.
   Acceptance (virtual time, `kahf-sim` many-station): 30 relays in one domain, 8 lanes, random queues: **no two relays
   ever share a (lane, slot)**; idle relays hold no slots; a busy relay gets over 80 percent of a lane;
   killing the server for 60 s degrades to local plan then recovers.
8. **Relay referee** (daemon): beacon with schedule, operator windows, request frames, grants, virtual carrier sense.
   Acceptance: 10 operators + 1 relay on one calling lane, each message delivered, no two operators overlap
   (they are the "hidden station" test).
9. **Server routing and presence** (`server/src/router.js`): `route(dest)`, presence with hashes, paging,
   warm second choice. Acceptance: the both-offline case (6.3) in the simulator: known, stale, unknown, and
   not-reachable, with the outbox retry.
10. **WebSocket push** (replaces polling in `backbone.rs`/`backbone.js`) with the HTTPS fallback.
    Acceptance: relay-to-relay latency over the fake server under 100 ms in the simulator.
11. **No-storage conformance test**: a test that scans the server's storage calls and the relay's disk
    writes during a message run and fails if a body is ever written.
12. **Receive diversity (10.1)**: LLR forwarding and combining. Acceptance: a frame undecodable at each of
    three relays at the same SNR is decoded by the server.
13. **HARQ chase combining** in `link.rs`, then lane bonding in the planner and relay.
14. **Propagation learner** and the solar inputs (server), then predictive paging.
15. **Hopping** (section 9): `HOP` grants and forward rule.
16. **Soak**: a simulated week with relay crashes, a closed band each night, interferers, floods. Acceptance: no
    message delivered twice, none lost that the rules say must be delivered, and the speed numbers in 11 hold
    on the good links.
17. **On the air**, after D1 to D3 are decided and the frequencies verified: two real relays and a few operators.

---

## 15. Glossary

- **Lane**: one 480 Hz-wide channel on a 700 Hz pitch. **Calling lane**: C1, C2. **Relay lane**: R1 to R8. **Spare lane**: X1, X2.
- **Epoch / slot**: 12 s / 2 s on UTC. **Grant**: a signed right to transmit in a lane and slot.
- **Clash domain**: relays that can interfere with each other on a lane at an hour.
- **Presence**: which relay last heard which operator. **Paging**: asking for an operator by 24-bit hash.
- **Cut-through**: forward immediately, store nothing. **Soft combining**: adding the likelihoods of the same frame heard at several relays.
- **ACDS**: automatically controlled data stations (unattended), a band-plan category that includes store-and-forward.
- **IBP**: the International Beacon Project, on 18.110 MHz (18.109 - 18.111 is for beacons only).

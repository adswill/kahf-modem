# KAHF on a busy channel: what can go wrong, and the design that answers it

Status: proposal for the owner. Nothing here is built yet except where it says "exists".
Written after the BlackHole test showed stations sending over each other, replies lost,
and speed collapsing. Those were symptoms. This is the whole list.

## 0. The facts everything rests on

- **Architecture (owner, latest): relays are online, operators may be offline.** Operators reach
  a relay by radio; relays reach each other and the server over the internet. Radio hops between
  relays are not part of the design. See the update at the top of NETWORK-SCENARIOS.md and its
  section H. Where this file describes forwarding between relays over the air (L2: suppression,
  hop limit), read it as the fallback, not the main path.

- One shared channel, about 2.4 kHz wide. Everyone in range hears everyone, **except** stations
  that cannot hear each other but both reach the same relay (the hidden station problem).
- A radio is half-duplex: while it sends, it is deaf. A station that is sending cannot know
  that someone else started.
- A burst is long: 0.5 s (fast mode) to 8 s or more (slowest mode). A collision wastes all of it.
- No shared clock. Laptop sound cards drift, wall clocks differ by seconds, and there is no
  internet or GPS guarantee. Anything timed must be timed from what was heard on the air.
- Many people will be on it at once, in the same city, on the same frequency (that is the
  point of the product). The design must assume a busy channel, not an empty one.
- Callsigns are claims, not proof. Anyone can send as anyone.

## 1. What exists today

| Thing | State |
|---|---|
| Energy carrier sense (10 dB over a 10 s floor) | exists, `access.rs`, `sense.rs` |
| Channel pools and a pick that avoids busy channels | exists, `select.rs` |
| One link at a time, ARQ window of 8, ACK per frame | exists, `link.rs` |
| Timers from the burst length | partly (fixed in this branch: start at TX end) |
| Relay role, slots, forwarding of chats and mail | exists, but one 24 h "may transmit" slot, no receive windows |
| Forward de-duplication on a relay | exists (hash of the message, 512 kept) |
| Advert: hop limit 4, de-duplicated | exists (new) |
| Hop limit and id on ordinary messages | **missing** |
| Listen-before-talk hold after hearing a burst | **missing** |
| Random back-off, doubling after each failure | **missing** |
| One acknowledgement per burst | **missing** (ACK per codeword, each its own burst) |
| Reply when the called station is busy | **missing** |
| Fair share of air time | **missing** |
| A test that puts many stations on one channel | partly (stress tests, mostly failing) |

The stress results already on file (ten stations: 3 of 10 delivered; seven in a line: fails)
are what this list predicts.

## 2. Everything that can go wrong

### A. Two stations send at the same time

1. **Both start together** (both saw a quiet channel). Both bursts are lost, or the louder one
   survives (capture). Seen in the test: both sides sending at 12.2 s.
2. **One starts during the other's burst.** The late one is deaf and cannot know. Seen: the
   receiver sent while the sender was still sending, because it answered every codeword the
   moment it arrived.
3. **Lock-step retries.** Both lost, both wait the same time, both send again, both lost again.
   Forever. Fixed timers make this certain.
4. **Hidden station.** A and B both reach relay R but not each other. Carrier sense on A says
   quiet while B is sending. R hears both at once. Listening cannot solve this; only R can.
5. **Capture hides unfairness.** The strong station always wins, the weak one never gets
   through, and nobody notices because the channel looks busy and productive.

### B. A station answers wrongly or too much

6. **Answer storm.** Eight codewords received cause eight separate answer bursts, each with its
   own preamble. Seen. A short exchange becomes ten seconds of talking.
7. **Answer on top of continuing data.** Window of 8 means the sender keeps sending. An answer
   per frame collides with the next frame every time.
8. **Everyone answers.** A relay and the real destination both hear a message and both reply.
9. **Duplicate delivery.** The answer is lost, the sender repeats, the receiver delivers the
   message twice, or a relay forwards it twice.
10. **Called station is busy.** It is already linked to someone else. The caller gets silence,
    retries five times, and each retry collides with the traffic it cannot hear. Many callers
    make this much worse.
11. **Two stations call each other at once** (simultaneous open): each waits for the other.
12. **Stale link after a crash or restart.** One side believes the link is up and keeps
    sending into nothing.

### C. The receiver is fooled

13. **False start.** Noise or a partial echo looks like a burst start and the receiver waits
    for up to the longest burst (about 8 s) while the real burst goes by. Seen: that was the
    "sender only shows Sending" bug.
14. **Own echo.** A virtual cable, a leaky cable or VOX feeds our own signal back in. We decode
    ourselves, or clear the buffer and cut the start off the reply. Seen.
15. **Torn bursts.** A burst partly overlapped by another decodes to garbage. The receiver
    must give up quickly and be ready for the next one.
16. **Non-KAHF energy** (a voice QSO, FT8, a noise source) keeps the channel "busy" for ever,
    so nobody sends; or it is more than 90 percent of the time busy, so the floor rises and
    busy reads quiet (a known limit in `access.rs`).
17. **Speed flapping.** Frames lost to collisions are counted as a weak signal, the speed is
    cut to the slowest mode, which makes every burst longer, which makes collisions more
    likely. A busy channel then locks itself into 40 bit/s. Seen at 7 dB.
18. **Fading.** On HF the signal level changes every few seconds. A speed chosen from an old
    report is wrong. The report itself needs a working link to arrive.
19. **Clock and rate drift** over a long burst: sound cards disagree by about 50 ppm, so the
    slowest modes (8 s and longer) walk off the symbol timing.

### D. The relay

20. **A relay is deaf while it sends**, so it misses what is sent to it exactly when the
    channel is busiest. Today it has a permanent "may send" slot and no listening windows.
21. **Several relays hear the same message and all forward it.** Duplicates, then loops:
    a message has no hop limit, so two relays can pass it back and forth.
22. **Flooding.** Everything heard is forwarded everywhere. In a dense area one message
    becomes dozens of transmissions. Adverts do the same unless they are rate limited.
23. **Which relay does a user use?** Several are in range. No load or health is known, so all
    users pick the strongest one and overload it.
24. **A relay dies** and users keep sending into it. Nothing tells them to choose another.
25. **Queue grows without limit**: a destination that never appears, a file that never
    finishes. The relay needs expiry, a size limit, priorities (urgent first), and to tell the
    sender when it gives up.
26. **One relay, many users, one link at a time.** A link layer that holds one connection means
    the twenty-first user waits for twenty others, and every one of them is retrying meanwhile.
27. **Relay and station disagree about time.** Any plan of "you send in the first ten
    seconds of each minute" fails on drift. Timing has to come from the relay's own beacon.

### E. Fairness and abuse

28. **A file hogs the channel.** Three minutes of continuous bursts and nobody else can say a
    word. Short messages and urgent ones must be able to cut in.
29. **A broken or hostile station** sends all the time, or floods adverts, or claims another
    callsign. Needs per-callsign rate limits at the relay, and signed adverts or at least
    "unverified" shown in the contact list.
30. **Location in adverts** is copied by every relay for ever. Precise mode is a real leak;
    it must be the person's clear choice (it is today: off, area, precise).
31. **Legal limits:** station identification, third-party traffic rules, and listen-before-talk
    are required in many places. We warn, we do not block (owner rule), but the defaults
    should be the lawful ones.

### F. The app tells the person the wrong thing

32. **"Sent" means "handed to the link", not "arrived".** The person sees Sent and the message
    never arrived; or sees Sending for ever while it did arrive (seen).
33. **Mail and its files are separate messages.** The mail can arrive without the files.
34. **No sign of a busy channel.** A message waits 40 s and the person thinks the app froze.

## 3. The design

Layers, bottom up. Each rule answers one or more numbered items above.

### L0. Etiquette every station follows (items 1, 2, 3, 5, 7, 13, 14, 28)

Taken from what works for 802.11 and amateur packet, adapted to long bursts.

- **R1 Never start while a burst is being received** (any lock, even a doubtful one for up to
  one short burst length).
- **R2 Two waiting times.** After a received burst ends:
  - the station the burst was **for** may answer after **0.3 s** (the short gap, "SIFS");
  - everyone else must wait **0.9 s plus a random back-off** (the long gap, "DIFS").
  An answer therefore always beats new traffic, and bystanders never talk over an answer.
- **R3 Random back-off, counted only while the channel is quiet.** Window of 4 slots of 0.25 s,
  doubling after each failure up to 64, reset on success. Frozen while the channel is busy.
  A different random number every time. This breaks lock-step retries.
- **R4 One burst train at most 20 s, then a gap.** In the gap anyone may speak. Urgent
  messages ignore the gap. When other stations are heard, a station's own air time is capped
  at about a quarter of the last two minutes (a file transfer takes turns).
- **R5 Own echo is ignored by time, not by clearing the buffer**: samples received during our
  own transmit and for a short tail after are marked and skipped, nothing before or after is
  thrown away.
- **R6 A false start is dropped after one short burst length** while the real burst is
  searched for in the audio that follows. (Done in this branch.)

### L1. Link (items 6, 7, 9, 10, 11, 12, 17, 32)

- **One answer per burst train**, carrying a bit map of which of the 8 codewords arrived.
  Eight codewords in, one short answer out. Replaces ACK per codeword.
- **Retry timer starts when our burst ends**, uses the length of the answer burst, then
  doubles with jitter. (Done in this branch.)
- **Frames carry a sequence number and a message id**, and the receiver delivers each id once
  and re-acknowledges repeats.
- **BUSY reply.** A station already linked answers a call with "busy, try after N s" and the
  caller waits that long instead of retrying five times.
- **Simultaneous calls:** the lower callsign wins and the other becomes the answering side.
- **Link idle time-out and a "who are you" re-sync** after a restart.
- **Speed choice is based on SNR first.** A loss with a good SNR is treated as a collision,
  and it raises the back-off, not the mode. Speed goes down only when SNR says so or losses
  continue after the back-off has grown. Speed goes up by one step at a time with a
  hold time (hysteresis). The first burst of a link is always sent in a safe mode.
- **Delivery states the person sees:** Waiting for a quiet channel, On air, Reached the other
  station (answer heard), and for relayed messages At relay, Delivered. Failed shows a reason.

### L2. Relay as the traffic controller (items 4, 8, 20 to 27)

Direct station to station traffic uses L0 and L1 only. When a relay is in range it becomes
the arbiter, which is the only fix for the hidden station.

- **Beacon** every 30 to 60 s, and after each exchange: relay name, its channel, how loaded it
  is (queue length, share of time busy), and when its next **request window** opens. The
  beacon is also the shared clock: stations time from it, not from their own clock.
- **Request window.** A station that wants to send to or through the relay sends a one
  codeword **request** at a random slot inside the window (slotted random access; collisions
  in a window cost one short frame, not a whole burst).
- **Grant.** The relay answers each request it decoded with a grant naming the station and how
  long it may send. Everyone who hears the grant stays quiet for that time (virtual carrier
  sense). The grant is heard around the relay, so a hidden station also keeps quiet. This is
  RTS/CTS with the relay as the one party everyone hears.
- **Relay listening windows.** The relay transmits only in its own gaps (after a grant ends,
  or between exchanges), not all the time.
- **Store and forward with rules:** message id, origin and hop count (limit 4) in every
  message; remember ids seen; **random delay before forwarding and cancel it if another relay
  forwards first** (suppression), so only one relay usually does. Directed forwarding: forward
  only when the destination has not been heard recently. Expire after a few hours, queue
  limit, urgent first, a "could not deliver" notice back to the sender.
- **Files are never forwarded** (done in this branch). They are too long for a shared relay.
- **Per-callsign rate limit** at the relay: messages per hour and bytes per hour, with a
  plain "relay is busy, try again at 14:20" reply.
- **Relay choice and fail-over:** the app lists relays heard, with signal and load, chooses
  the best, and moves to another if the beacon stops for three intervals.
- **Adverts:** limit one per station per 10 minutes and ignore repeats; the relay passes an
  advert on after a random delay and only if it was not heard again from another relay.
- **Trust:** contacts learned from an advert are marked "not verified" until the same
  station signs something (mail already signs) or the server lists it.

### L3. App (items 32, 33, 34)

- Show the channel state in the top strip: quiet, busy (with the wait), or a relay
  controlling it. A waiting message says why and for how long.
- Mail sends its text and its files as one item with one status; if a file cannot be sent the
  person is told which one.
- The file warning (limit, time estimate, direct only) is already in.

## 4. How we prove it before anyone trusts it

The real radio cannot test this, and the laptop pair only has two stations. So first a
**network simulator** that runs the real link and relay code on a virtual channel:

- virtual time, N stations, a graph of who hears whom (so hidden stations exist);
- collisions, capture (the louder survives), random loss, own echo, clock drift, delays in the
  sound card;
- stations can be told to send chats, mail, files, adverts, in random patterns.

Scenarios that must pass before a MAC change is merged:

1. Two stations talk at the same instant, 100 times: all messages arrive, none lost.
2. Ten stations send random chats for ten minutes on one channel: at least 95 percent
   delivered, no station below half the average share (fairness index above 0.8).
3. Hidden station triple (A, B hear only R): both deliver through R, with no loss from collisions.
4. A file transfer runs while five stations send chats: chats are delayed by less than 20 s.
5. Seven stations in a line with relays: delivered end to end, no duplicates, no loop.
6. Two relays hear the same message: forwarded once (or at most twice), never looped.
7. A relay restarts mid-forward: the message is delivered once, late, not lost, not doubled.
8. 30 percent of answers lost: no duplicate deliveries, no runaway retry.
9. Own echo on: no false locks, no lost replies.
10. Fading signal: speed follows, does not flap.
11. A station that sends constantly: the others still get at least their fair share.
12. Fifty adverts at once: all learned, at most a few forwarded each.
13. Called station busy, ten callers: all served in turn, no retry storm.

The stress-net branch already has two of these (ten stations, seven in a line). They fail
today, and they become the first tests the new rules must turn green.

## 5. Order of work

1. **Simulator and the scenarios above (they fail first).** This is the measuring stick.
2. **L0 and L1 fixes** (hold times, back-off, one answer per burst, busy reply, ids and
   de-duplication, speed from SNR). Re-run until the direct scenarios pass.
3. **Message ids, hop count, suppression, expiry, limits** (L2 store and forward).
4. **Relay beacon, request window, grant** (L2 controller). The largest piece, and the one
   that makes a busy city work.
5. **App channel state and delivery states** (L3).

Steps 1 and 2 are what stops the behaviour seen in the BlackHole test. Step 4 is what makes
it scale.

## 6. Decisions only the owner can make

- Whether relays become controllers (step 4), or whether we ship with listen-before-talk and
  back-off only first. Controllers are more work and make a relay a single point of control
  for its area; without them the hidden-station case stays unsolved.
- Air time cap while others are active (proposed: one quarter of two minutes) and the burst
  train limit (20 s). Both can be changed later; they are defaults.
- "Delivered" meaning: the other station's answer (cheap, available now), or a receipt from the
  recipient that travels back through relays (costs more air time, more honest).

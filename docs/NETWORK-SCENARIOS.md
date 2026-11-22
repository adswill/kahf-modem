> **Background only (2026-11-18).** KAHF now works on 17 m only; where this file mentions 20 m, 40 m or 80 m, channel pools or one 24 h relay slot, `docs/MESH-SPEC.md` replaces it.

# KAHF network: scenarios, what happens, what we do

Companion to NETWORK-DESIGN.md. That file lists the layers. This one walks through concrete
situations one at a time, says what goes wrong today, and gives the rule that fixes it and the
test that proves it. Each scenario is numbered so the simulator can name its tests after them.

"Today" means the code in branch `app-rig-relay`.

## Architecture update (owner, latest): relays are online, operators are offline

A **relay always has internet**. An ordinary **operator may have none**: only a radio and a
sound card. So the network is two parts:

- **The radio side**: operators talk to the relay in their area (or directly to each other).
- **The internet side** (the backbone): relays talk to each other, and to the kahf.dev
  server, over the internet. They do not need to pass messages from relay to relay over the air.

What this changes:

- Radio **multi-hop between relays is dropped** from the main design. A message goes
  operator, relay (radio), internet, relay (radio), operator. That removes most of the hidden
  relay, loop and suppression problems (S3, S4, S30 below) and the flood of adverts on air.
  Radio hops stay only as an emergency fallback (section H, H5), off by default.
- **The server is the referee for duplicates.** When two relays hear the same message, the first
  to register its id with the backbone wins; the other drops it. No radio race needed (S1).
- A relay that loses the internet is **not a relay any more**: it becomes a local hub (H3) until
  the connection returns, and says so in its beacon.
- Because relays are online, they can give offline operators things they cannot get themselves:
  **the correct time, who else is around, the signed release notice, which relays exist**
  (section H).

Section H covers the offline-operator scenarios. Sections A to G apply where they still make
sense: direct operator to operator traffic (no relay in range) and the radio side of a relay.

## Principles that fall out of the scenarios

These came out of working through the cases below. Each one answers several scenarios.

1. **Every message has an id**: origin callsign plus a 32-bit counter, and a hop count. Everything
   else (duplicates, loops, receipts, cancel) hangs on that id.
2. **A frame names its next hop.** "For relay R" or "for the destination" or "for any relay".
   Only the named station takes responsibility for it. This is what stops two relays both taking
   the same message.
3. **"Any relay" is a race with cancel.** Each relay waits a random time (shorter if it is
   closer to the destination, less loaded, or heard the destination recently), and the first to
   finish sends a short "I have it" answer. The others hear it and drop their copy.
4. **Responders speak in a fixed order.** The destination first (short gap), then the named relay,
   then any-relays by random delay. Nobody talks at the same time by design.
5. **Passive acknowledgement.** A station that hears the next hop pass its message on knows it was
   taken. No separate answer is needed on a chain.
6. **Custody is not delivery.** "A relay has it" and "the person has it" are different states,
   shown differently, and the sender keeps a copy until the second one arrives or the message
   expires.
7. **Mailbox, not push.** A relay holding a message for someone it cannot hear does not keep
   calling them. It lists who it holds mail for in its beacon, and that person, on hearing their
   name, asks for it.
8. **Time-based memory.** "Seen this id" is remembered by time (longer than the message lifetime),
   not by count, and survives a restart. A fixed list of 512 forgets under load and loops return.
9. **No wall clock.** Timing comes from what was heard on the air (the beacon, the end of a
   burst), never from the computer's clock.
10. **A silent network is a normal network.** Every wait has a limit, every retry has a growing
    back-off with random jitter, every queue has a size and an expiry.

---

## A. The same transmission is heard by several stations

### S1. Two relays both hear the same message from one station

Alice sends to Dave. Relay R1 and relay R2 are both in range of Alice. Both decode it.

- Today: both forward it. Dave receives it twice (maybe three times through other paths), and the
  two forwards may collide with each other on the air. There is no id, so Dave cannot tell it is
  the same message.
- Rule: Alice names no relay (she does not know them), so it goes out as "for any relay" (principle
  3). R1 and R2 each start a random timer. The first to expire, say R1, transmits "R1 has message
  42 from Alice". R2 hears it and drops its copy. If R2 cannot hear R1 (hidden from each other),
  Alice gets two answers, which is harmless, and both forward; Dave delivers the first and ignores
  the second by id (S6), and sends one receipt (S34) that both relays hear and use to delete their
  copy.
- Test: two relays and one origin, hidden and not hidden, the message arrives exactly once at the
  destination in both.

### S2. A relay and the destination both hear the message

Alice sends to Dave. Dave is in range, and so is relay R.

- Today: Dave answers; R also forwards the message to Dave, and Dave gets it a second time.
- Rule: Dave answers in the short gap (SIFS). R waits at least one long gap plus its random
  delay, which is always later. R hears Dave's answer and cancels. R also remembers "Dave is
  heard directly" for some minutes, so for Dave it will not take custody at all.
- Test: R never transmits when the destination answered.

### S3. A chain: Alice, R1, R2, R3, Dave

- Today: each relay forwards what it hears, with no hop limit and no memory beyond 512 items.
- Rule: R1 takes custody and forwards, naming R2 as next hop if known (else any-relay). R1 hears
  R2 transmit the same id: passive acknowledgement, done. R2 hears R3, and so on. The hop count
  goes up at each relay and is dropped at the limit (4 for now; the number must be larger than
  the longest sensible chain in an area and is a setting).
- A relay that hears the message from the other direction (R2 hears R1's forward and also the
  original from Alice at weaker strength) sees the same id and does nothing.
- Test: chain of seven, one delivery, hop counts right, no loops (this is the stress-net seven
  line that fails today).

### S4. A message goes round in a circle

Two relays that can hear each other, a destination nobody can reach.

- Today: no hop limit, so R1 and R2 pass it back and forth until the 512-item memory pushes it
  out, then it starts again.
- Rule: hop limit, id memory by time that outlives the message (principle 8), and the message
  expires (S27).
- Test: unreachable destination, the traffic stops after the hop limit and the message expires.

### S5. The origin hears its own message forwarded

- Rule: it sees its own id and uses it as passive acknowledgement of custody. It does not treat
  it as new, and does not forward.

### S6. The answer was lost, so the sender repeats

- Today: the receiver may deliver the message twice, because nothing identifies repeats.
- Rule: receivers keep the ids of messages already delivered. A repeat is not delivered again but
  is answered again (the sender needs the answer). Relays do the same: a repeat of a message they
  hold gets a new "I have it", not a second forward.

### S7. A station is in range of one relay only at some times

- Rule: relay choice is by recent beacon (S22). A station that hears no beacon for three
  intervals considers the relay gone and chooses another or goes direct.

---

## B. Several stations speak at once

### S8. Two stations start together

- Rule: random back-off with slots, counted only while quiet (L0, R3). After a collision both
  draw new random numbers from a window that doubled. Lock-step is impossible.
- Test: two stations, queue a message at the same instant, 100 runs, all delivered.

### S9. A station starts in the middle of another's burst

- Rule: nobody starts while a burst is being received (R1), and nobody starts until the long gap
  after it ends (R2). The only exception is the named destination, which answers in the short
  gap.
- Test: the receiver never transmits before the sender's burst ends.

### S10. Hidden stations: A and B both reach relay R, not each other

- Today: both send, R hears a mix, both retry on fixed timers and collide again.
- Rule: R sends a beacon with a request window. A and B each send a short request in a random
  slot (a collision costs one short frame, not a whole burst). R grants them one at a time, and
  the grant says how long. A grant is heard by everyone near R, so a station that did not hear the
  other still hears the grant and stays quiet (virtual carrier sense).
- Without a relay: they cannot hear each other and nothing can fix that, only retries with
  growing back-off; the app says so ("channel may be busy with stations you cannot hear").
- Test: hidden triple, both deliver, no collision between bursts.

### S11. The loud station always wins

- Today: capture means the strongest is decoded and the weaker one never gets through, and its
  retries make the net busier.
- Rule: under a relay, grants are given in order of waiting time, so the weak station is served
  too. Without a relay, back-off grows for the loser until it wins a quiet slot. The relay counts
  per-station service and the simulator reports a fairness number.

### S12. A thundering herd

A relay comes back after an outage, or a beacon ends a long silence, and twenty stations that
have been waiting all transmit at once.

- Rule: the request window has slots, the number of slots is sized from the load the relay
  estimates (and announced), each station picks a random slot, and a station that does not get a
  grant doubles its slot range. Stations also add a random delay after a beacon that ends a quiet
  period.

### S13. One station never stops

A stuck PTT, a program bug, a loop.

- Rule: a hard limit on one transmission (60 s), a limit on a train of bursts (20 s then a gap),
  an air-time share cap while others are heard. The relay refuses a station that exceeds its
  rate and tells it when to try again.

### S14. A file transfer shares the channel with chats

- Rule: the train limit and the gap let chats in. Urgent messages ignore the gap. Files are
  direct only and a relay never holds them (so a file never blocks a relay).
- Test: a file runs while five stations chat, chats wait less than 20 s.

---

## C. Calls and links

### S15. The called station is busy

- Today: the caller retries five times into the busy station; with several callers this is a storm.
- Rule: the busy station answers once, "busy, try in N s", and the caller waits that long (plus
  jitter). The busy station keeps a short list of callers in order and calls the first when free.
- Better for relays: use connectionless message delivery (one frame, one answer) for chats and
  small mail, not a link. A link is for files and long mail between two stations.

### S16. Two stations call each other at once

- Rule: the call carries both callsigns; the lower callsign wins and becomes the answerer.

### S17. The link breaks halfway

- Rule: delivery of a message is resumable: the receiver reports which chunks it has, the sender
  sends only the rest. If the link cannot be re-established, the message stays in the outbox with a
  retry when the station is next heard.

### S18. One side restarts

- Rule: link has an idle time-out on both sides and an "are you still there" step before data after
  a long silence. A restarted station answers an old session with "unknown session", and the
  other side starts a fresh one. The receiver's partial message is discarded, the sender's copy
  is still in the outbox.

### S19. A station leaves range during a transfer

- Rule: the retries stop after the limit, the message returns to the outbox, and it is sent again
  automatically when the station is heard again (an advert, a beacon, any frame counts).

### S20. The called station is in listen-only mode

- Rule: a listen-only station says so in its adverts ("receive only"). Callers see it in the
  contact list and are told before they send. Without that, the caller retries into silence.

---

## D. Addressing and identity

### S21. The destination is not heard by anyone

- Today: the relay keeps trying to call a station that is not there.
- Rule (principle 7): the relay holds the message, and its beacon lists a short code for each
  station it holds mail for. When that station hears its own code it asks for the mail. The relay
  never transmits unattended to an absent station.

### S22. Which relay should a station use?

- Rule: beacons carry load (queue size, share of time busy) and the channel. The app ranks relays
  by signal and load, uses the best, and shows the list with the reason. If the best stops
  beaconing, it moves on.

### S23. The destination is in another area, reachable only through other relays

- Rule: relays remember "heard X through relay R, N hops" from adverts and forwards. A relay
  forwards toward the best known next hop and names it. If it knows nothing, it forwards as
  any-relay with the hop limit.

### S24. Two stations use the same callsign

A typing mistake, a second device, or someone pretending.

- Rule: each station has a key. If two keys appear for one callsign, the contact list shows a
  warning and the app asks which one is meant. A second device can add a suffix (KA1ALF-2). A
  message is shown as "unverified" unless it was signed with the key we know.

### S25. Someone forges an advert or a message from another callsign

- Rule: adverts and messages are accepted but marked "not verified" until signed by the known key.
  Signed mail exists today. An advert is too long to sign inside one frame, so a signed follow-up
  (one extra frame) is sent after the first advert, and the contact list shows the check mark when
  it arrives. A forged advert can still appear in the list, labelled unverified.

### S26. The location in an advert is copied by every relay

- Rule: the person chooses off, area or precise (already so). Relays pass the position on
  unchanged but never record it beyond the contact list. The app states plainly that anyone who
  hears the advert can read it.

---

## E. Relay life

### S27. A message that cannot be delivered

- Rule: every message has a lifetime (default 6 hours for chats, 24 hours for mail), shown in the
  app. When it expires, the holder deletes it and, if it can, tells the origin through the same
  route ("could not deliver to Dave"). The relay's queue has a size limit (default 200 messages
  or 1 MB) and the lowest priority and oldest go first.

### S28. A relay crashes holding messages

- Rule: the queue is saved to disk after each change. After a restart the relay still has them.
  If a relay is lost for good, the sender (principle 6) still has its copy and, with no receipt
  after a set time, sends again by another route. A duplicate is possible and harmless (S6).

### S29. A relay is deaf when it sends

- Today: permanent "may send" slot, no listening windows.
- Rule: a relay sends only inside the gaps it announces. The beacon says when its next request
  window opens, and between grants it listens. A grant for a long transmission is given only when
  nothing is waiting to be received.

### S30. Two relays share a channel

- Rule: a new relay listens for a beacon on its channel before sending its own. If it hears one,
  it picks another channel from the pool or shares by alternating beacon phases (the lower
  callsign keeps phase 0). Relays list each other as neighbours and forward between them
  (S23).

### S31. A relay is saturated

- Rule: it says so in the beacon ("busy, closing the window for N minutes") and refuses new
  mail with a reason. Urgent messages are still taken. Files are never taken.

### S32. A huge mail

- Rule: relays accept text up to 4 kB; larger mail with files goes direct only, with the same
  warning the app shows for files.

### S33. Adverts multiply

- Rule: one advert per station per 10 minutes is passed on; repeats are ignored by id. A relay
  forwards an advert after a random delay and cancels if it hears another relay do it first.
  The hop limit is 4 and the app shows how far it went.

### S34. Delivered, or only handed over?

- Rule: states shown to the person: Waiting for a quiet channel, On air, Reached the next station
  (answer heard), At a relay (custody answer), Delivered (receipt from the recipient), Failed
  (with the reason). The receipt is one short frame that travels back the same way and is also the
  signal for relays to delete their copy. A receipt costs air time, so for chats it is on by
  default and for adverts it is off.

---

## F. The receiver and the air

### S35. A false start while the real burst is arriving

- Rule: a lock is held no longer than one short burst length without a decoded codeword, and the
  search continues in the audio after it. (Done in this branch.)

### S36. The station hears itself

Virtual audio cable, rig sidetone, VOX leakage.

- Rule: samples during our own transmit and its tail are marked and skipped. The rest of the
  buffer is kept, so a reply that starts early is not cut.

### S37. Someone who is not KAHF is on the channel

A voice QSO, a digital mode, noise.

- Rule: carrier sense reads energy, so it says busy. The sender waits, with a limit, then tries
  another channel in the pool and tells the person: "the channel is in use by something that is
  not KAHF". The rule that busy must be above the floor also fails when the channel is busy more
  than 90 percent of the time; the app detects a long constant busy and shows it.

### S38. A burst is torn by an overlap

- Rule: after the first codeword fails to decode with enough audio present, the receiver gives up
  that burst immediately and looks again.

### S39. Speed flaps on a busy channel

- Rule: losses with a good SNR are counted as collisions (they raise the back-off, not slow the
  mode). Speed down needs a bad SNR or losses that continue after the back-off has grown. Speed
  up is one step at a time with a hold time. A new link starts in a safe mode. Each direction has
  its own speed.

### S40. Fading on HF

- Rule: the SNR report is smoothed and has a time stamp. A report older than two minutes is not
  trusted for going faster, only for going slower.

### S41. The two sound cards disagree on rate

- Rule: the receiver re-estimates symbol timing through long bursts; the slow modes are limited to
  a length the clock error does not break (about 10 s at 50 ppm; to be measured, not assumed).

### S42. A transmission is cut short by the sound device

An underrun, a stalled output, PTT dropped early.

- Rule: the transmit loop checks the device's real progress, not only the clock, and sends PTT
  off only when the buffer has drained. A failed burst is a lost frame like any other and is
  repeated.

---

## G. What the person does

### S43. Changing frequency or callsign during a link

- Rule: the app warns and closes the link first; the other side times out.

### S44. Two copies of the app on one computer and one sound card

This is the test setup, and it happens to real people too.

- Rule: the app warns when another KAHF is running on the same audio device and explains that the
  two will hear each other.

### S45. The app is closed during a transfer

- Rule: the outbox is on disk. The partial receive is discarded. On the next start the app says
  what is still waiting.

### S46. The person sends to someone who has never been heard

- Rule: the app tells them: "nobody has heard KD9XYZ yet; the message will wait at a relay for
  72 hours" (or direct only, if no relay is known), and offers Announce myself.

---

---

## H. Offline operators and online relays

"Operator" means a station with no internet. "Relay" means a station that has it.
H-numbers are scenarios; each ends with the rule and the test.

### H1. Operator to operator through two relays

Alice (offline) is near relay R1. Dave (offline) is near relay R2, 800 km away.

1. Alice's app sends the message to R1 over the radio (one request, a grant, one data burst, one
   answer "R1 has it").
2. R1 submits it to the backbone with its id and a lifetime.
3. The backbone knows Dave was last heard by R2 (R2 reports every station it hears, with time
   and signal). The message goes to R2.
4. R2 puts "mail for Dave" in its beacon (S21) and delivers when Dave answers or is heard, using
   a short exchange. Dave's app sends the receipt, R2 sends it back over the backbone, R1 tells
   Alice (next time Alice is heard, or in R1's beacon list "receipts for Alice").
- States Alice sees: Waiting, On air, At relay (R1 answered), In the network (the backbone
  accepted it), At Dave's relay, Delivered.
- Test: simulator with two radio areas and a fake backbone; delivered once, receipt returns.

### H2. The destination was never heard anywhere

- Rule: the backbone holds the message for that callsign for its lifetime and gives it to any
  relay that later hears the callsign (a "wanted" list sent to all relays). The sender's app says:
  "Nobody has heard KD9XYZ yet. It will wait up to 72 hours."
- When the callsign appears (an advert, any frame), the holding relay is told, puts the callsign
  in its beacon, and delivery runs as in H1.

### H3. A relay loses its internet

A power cut at the ISP, a cable cut, a flat phone hotspot.

- The relay detects it (no answer from the backbone for 30 s) and changes its beacon flag to
  "local only". Stations that hear it:
  - may still send to other stations **in this relay's area** (it delivers locally, S21 mailbox);
  - are told, before sending to anyone else: "this relay has no internet; the message will wait
    here until it is back, or choose another relay";
  - prefer another relay with internet if one is heard.
- Messages the relay already held for the backbone stay in its queue (saved to disk) and are sent
  when it reconnects. Messages for its area that arrive over the backbone cannot arrive
  meanwhile; they wait on the server.
- Test: kill the backbone connection, local delivery works, remote ones wait, nothing lost,
  everything arrives after reconnect, exactly once.

### H4. The server is down but relays still have internet

- Rule: relays keep a cached list of other relays (from the server, refreshed hourly) and talk to
  each other directly. They exchange the same "who heard whom" lists and queues, and use their
  own clocks plus NTP. Message ids are kept by the relays; a duplicate is possible and is dropped
  at the destination (S6). The directory is out of date until the server returns, which only
  affects where to send first.
- If a relay has never fetched the list (new install, server down): it works as a local hub (H3)
  and keeps trying.

### H5. No relay in range at all (and then not even one hop)

A remote valley, a boat, a power cut that took the relays down too.

- Operators talk **direct** (CSMA, back-off, receipts; sections A to F). The app says plainly
  "No relay heard. Messages go straight to the other station."
- The outbox keeps messages for stations that are not heard yet and sends them when they are heard
  (S19), or when a relay appears (H6).
- **Radio relaying between operators** (an operator forwards for another): off. It brings back
  every loop, duplicate and hidden problem in sections A to D for a case that is rare. If the
  owner wants it as an emergency feature, it is switched on by hand, with the hop limit and the
  suppression rules, and tested separately.

### H6. An operator walks or drives into relay range

- Rule: the first beacon heard triggers (once, rate limited): the app sends Announce myself (so
  the relay learns the callsign, with location if shared), then sends what is waiting in the outbox
  that is for the network. Mail waiting at the relay for this callsign is delivered.
- Test: a station is out of range, queues five messages, moves in, all five go and the waiting
  mail arrives, in order.

### H7. The operator can hear the relay, but the relay cannot hear the operator

The relay has a big antenna and power; the operator has a handheld.

- Today: the operator sees beacons, sends requests, hears nothing back and keeps trying.
- Rule: the operator's app counts "requests sent and no grant". After three, it says: "You can hear
  this relay but it probably cannot hear you. Try a better antenna, more power, or a higher
  place." Back-off grows so it does not flood. The operator's speed for a request is the slowest
  mode, because the relay's report of how well it hears them cannot arrive.
- The relay's beacon includes its receive floor and a "heard you" list (callsigns and SNR for the
  last few minutes). A station sees its own callsign with a good SNR there, or does not.

### H8. The operator does not know what time it is

No internet, no GPS, a laptop clock that is wrong. Messages have times and lifetimes.

- Rule: the relay's beacon carries the time (UTC, to a second, from NTP), and the app keeps the
  offset to its own clock. Message timestamps use the corrected time. Without a beacon, the app
  uses the computer's clock and shows "time unverified". Expiry uses the corrected time
  (principle 9): no schedule depends on the two clocks agreeing.

### H9. The operator is offline and needs to learn who is on the network

- Rule: relays broadcast a **directory frame** at a low rate (one frame every few minutes): a short
  list of stations they know in the area, with the signal and the time last heard. An offline
  operator builds a contact list from it without any internet. Location is included only for those
  who chose to share. Adverts from stations in other areas arrive the same way: the backbone
  carries them, and each relay puts them in its directory frames, no more than a few per hour.
- Advert reaches everyone: a station's advert is heard by its relay, the relay posts it on the
  backbone, each other relay includes it in its directory frames for a day. Radio is used
  once per area, not once per hop.

### H10. The operator needs to find a relay

First use, no internet, no list.

- Rule: the app ships with the bandplan and the default meeting channel and listens for beacons
  on it, then scans the other channels in the pool slowly in turn. A cached relay list from the
  last time the operator was online (or from a file on a USB stick) shows where to look.
- Once a beacon is heard the app saves the relay (name, channel, last time).
- If nothing is heard, see H5.

### H11. Many offline operators, few relays

A city of 500 radios and one relay.

- Rule: the beacon announces load and a second channel when load is high; the relay asks new
  stations to move there (a "QSY" in the grant). Requests are slotted (S12). Chats are small and
  mail text is limited; files are never taken. Priorities: urgent, chat, mail. The relay shows
  its limits in the beacon so the app can say "busy, expect 3 minutes".
- More relays in the same area use different channels (S30) and share the backbone.

### H12. An offline operator needs to be sure who they are talking to

- Rule: the first time a station is seen, its key is stored (trust on first use). A change of key
  for the same callsign is a loud warning. The relay verifies messages and adverts against the
  server registry when it can, and signs a "verified" statement that goes into directory frames
  (so an offline operator can see the check mark). Without any of that, the contact shows
  "unverified".

### H13. The operator's own message vanishes after "At relay"

R1 answered "I have it", then crashed before putting it on the backbone.

- Rule (principle 6): the sender keeps its copy until it gets the receipt or the lifetime ends.
  If no "In the network" state arrives after 5 minutes, the app resends through any relay heard
  (the id makes a duplicate harmless). The relay writes a message to disk before it answers
  "I have it".

### H14. The relay's internet is slow, not gone

- Rule: the backbone connection has a queue with retry and back-off, and a limit; the relay's
  beacon load figure includes it. States show "At relay" until the backbone accepts it, so the
  sender knows where it is stuck.

### H15. The destination moves between relay areas

- Rule: the backbone follows the latest "heard" report, with its time. A message goes to the
  newest relay. If it does not arrive (the person left), the backbone re-sends to the next
  newest after a set time. The first delivery wins and the receipt cancels the rest.

### H16. Software updates reach operators who are never online

- Rule: the signed release notice from the server travels in the relay's beacons (small, signed,
  with the version and where to get it). An offline operator sees "KAHF 4.2 is available" and gets
  the file by other means. The app verifies the signature, so a forged notice is ignored.

### H17. Who can read the messages

Relays and the server carry the messages. That means the people who run them can read them.

- Rule: this is stated plainly in the app and in the relay's terms. Radio law in many places
  forbids hiding the content of messages sent on air, so KAHF does not encrypt on air. Messages
  marked private are only for the addressee's inbox (existing rule), not secret. If an operator
  needs secrecy, the answer today is: do not send it. (Open question for the owner: end-to-end
  encryption of the part that travels on the internet only is possible, but a message would then
  have to be decryptable only at the destination relay, which breaks the radio side. We do not
  attempt it.)

### H18. Law and licences when a relay joins radio to the internet

- Rule: the relay operator confirms they hold a licence and accept the terms (exists). The app
  warns operators that a message handed to a relay may leave the radio network (third-party
  traffic rules differ by country). We warn, we do not block (owner rule).

### H19. A relay operator misbehaves

Reads mail, drops messages, injects fakes, floods the backbone.

- Rule: messages are signed by their origin where keys are known, so injected fakes show
  "unverified" and edited ones fail the check. A relay that drops mail is noticed by the sender
  (no "In the network" state, no receipts) and the app prefers another relay. The server
  rate-limits each relay and can remove one from the list. Honest limit: a relay that merely
  drops or reads cannot be stopped technically, only avoided.

### H20. The operator wants to reach someone who is on the internet only

A friend with no radio.

- Out of scope for now (the stub `relay_via_gateway` is the place). If wanted later it needs its
  own design: addresses, spam, replies, law. Asked of the owner, not assumed.

### H21. One person, two devices

- Rule: a suffix (KA1ALF-2) per device; mail for the plain callsign goes to the one heard most
  recently, and to both if the person says so. Each has its own key.

### H22. Two offline operators and a relay, all three in range

- Rule: the destination answers directly in the short gap (S2); the relay hears it and does not
  take custody. If the destination does not answer (it is off), the relay takes custody and delivers
  later.

---

## Open questions the scenarios raise

1. **Lifetime and size defaults** (S27, S31, S32): 6 h chat, 24 h mail, 200 messages, 4 kB mail
   text, 100 kB direct files. All are settings. Want different numbers?
2. **Who may be a relay** (S22, S24): anyone can claim to be one today. Should a relay be listed
   only when the server (or other relays) have seen it work, so that a new relay starts "trial"?
3. **Receipts** (S34): on for chats and mail by default, with the cost accepted?
4. **Mailbox privacy** (S21): the beacon lists short codes of callsigns waiting. That tells an
   eavesdropper that someone has mail. Is that acceptable, or should codes rotate each beacon?
5. **Third-party traffic rules** differ by country. We warn and do not block; should the first-run
   screen ask the country and set the lawful defaults?

## What is covered, and how we know

Every scenario above becomes a named test in the network simulator (S1 to S46 and H1 to H22). A change to the
rules is merged only when all pass. The scenarios that fail today and prove the problem:
S1, S2, S4, S8, S9, S10, S12, S14, S15, S21, S29, S39, and all of H (none of it exists yet).

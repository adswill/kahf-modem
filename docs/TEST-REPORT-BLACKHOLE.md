# Test report: two to five stations over BlackHole

Run on one Mac, every station an engine in the same sound device (BlackHole 2ch in and out),
so each station hears everything, including its own sound played back about 0.4 s later. No
rig control and no PTT. The tools are the examples in `crates/kahf-daemon/examples/`:
`bh_connect` (connect two stations), `hangup`, `soak` (messages both ways for minutes) and
`mesh` (five stations, two hops). Frames go out as ladder bursts (#28) unless said otherwise.

## What was wrong, and what is fixed

| Finding | Root cause | Fix | Regression test |
|---|---|---|---|
| A station heard the call but never answered, over any real sound path | The preamble repeats every 128 samples; the correlation is equal at each repeat. A little noise or the filter of a sound path made the receiver pick a later repeat, so the frame was read 128 samples late | The first peak within 5 percent of the strongest wins | `preamble_start_is_the_first_repeat_even_with_noise` (40 noise seeds) |
| A false preamble hit blinded the receiver | After a failed decode the receiver waited for the 8 copies of a weak-mode burst, about 25 s, and then threw away what it had heard | Skip the hit when no second copy follows; take a real frame that follows the hit | covered by the shared-bus soak |
| A station received its own message | BlackHole plays our own burst back; the link layer took it for the peer's frames | Frames of our own bursts are remembered and dropped when they come back | `our_own_frames_played_back_are_recognised_and_other_frames_are_not` |
| The called station could not send back | Unattended sends need an operator link; only the caller had one | An operator send counts as the operator being present | `an_operator_who_sends_is_attending_whichever_side_started_the_link` |
| Transmitting while receiving | No listen-before-talk | `lbt.rs`: quiet wait, random backoff, own sound not counted; the end of a burst triggers a scan at once (the idle scan period, 4 s, was longer than the sender's retry) | `lbt::tests` (7) |
| A relay stuck on a call whose hang-up was lost | A hang-up was sent once; the relay never timed the call out | Hang-ups are sent twice, and a relay closes a call quiet for 20 s when it has something to deliver | `a_hang_up_reaches_the_other_station` |
| Debug builds decoded too slowly for real time | No optimisation of the modem crates in the dev profile | `profile.dev.package.kahf-core` opt 3, `kahf-daemon` opt 2 | all real-time tests |

## Measured

| Test | Result |
|---|---|
| Connect, two stations, ladder bursts | link up in 2.8 s (before the fixes: never) |
| Hang-up | the other station sees it 1.0 s later; repeated 2.5 s later |
| Soak, 150 s, messages both ways every 20 to 27 s | 12 of 14 delivered, about 5 s each; 5 overlapping sends, all from both stations choosing to send in the same second |
| Mesh, five stations, A to D by the relay R1 (relay path set) | 3 of 3 delivered, 39 to 54 s each (average 45 s) |
| `cargo test -p kahf-daemon` | 87 pass, 2 real-time bus tests ignored (run with `-- --ignored`; they pass on a quiet machine, not under load) |

## Matrix run (`examples/matrix.rs`), second pass

Two stations, three connect / chat / hang-up cycles, files both ways in two of them:

| Step | Result |
|---|---|
| connect (6 runs) | up in 2.4 to 5 s each; once 10.9 s |
| A to B message, B to A message (9 steps) | all delivered, 3.8 to 9.5 s each |
| hang-up seen by both | 7 of 8 (the one miss was seen later, after the next connect) |
| 1.5 kB file A to B, content compared byte for byte | 3 of 3 passed, 32 to 65 s |
| 0.9 kB file B to A | 3 of 6 passed, 54 to 83 s; the 3 failures all came right after an A to B file in the same session and never finished in 150 to 240 s |
| messages A to D through one relay, with a bystander | 3 of 3 delivered in 43.7 s each; the bystander never showed the message (3 of 3) |
| the same with two relays configured | 3 of 3 in 43.5 s each, but see below |

What the failing file transfer shows (trace of both sides): after the first file both stations
keep sending data frames at each other, so the other side's acknowledgements and data collide;
each side then sees loss, moves to a slower ladder rung, and its bursts get longer (0.8 s up to
8.4 s), which collides more. The link layer is stop-and-wait over a half-duplex channel with no
turn-taking, and carrier sense cannot break the loop on a sound path whose delay it cannot see.
The sound path delay is now measured (first energy of our own playback after a burst) and
carrier sense waits longer than it, which helped but did not remove the failure. A real fix
is turn-taking in the link layer (the sender of a burst train yields the channel to the peer's
acknowledgement), which belongs with the modem and link work.

Two relays in a row were not truly exercised on BlackHole: every station hears every other, so
R1 delivers to D directly and never needs R2. The multi-hop path with stations that cannot hear
each other is covered by the simulated air in `message_crosses_two_radio_relays`.

## Third pass: turn-taking

Cause of the file stall: no rule said whose turn it was. Both stations sent data at each other,
the acknowledgements collided with data, loss reports pushed both down the ladder to slow, long
bursts, and the loop fed itself. The link also mistook late echoes of its own frames for the
peer's, which made the first version of the rule stall.

What changed in the daemon:
- A station that hears the peer's DATA gives the peer the turn for 2.5 s and does not start its
  own data meanwhile (acknowledgements always go).
- The link's retransmit clock runs only on an idle channel: not while we transmit, not while the
  peer's signal is on the air, not while the peer has the turn.
- Our own frames are remembered for 8 s (was 1.5 s) so their late playback is never read as the
  peer's.

| Test over BlackHole | Before | After |
|---|---|---|
| one session, 10 pairs: 1.2 kB A to B then 0.9 kB B to A, byte for byte | B to A failed about half the time | 20 of 20 pass, 91 to 131 s each |
| `two_stations_*` bus tests | 1 pass in 6 | 12 of 12 pass, now un-ignored |
| `cargo test -p kahf-daemon` | 87 pass, 2 ignored | 93 pass, none ignored, three runs in a row |

Files are reliable but slow, about 10 bytes a second, because every frame is its own burst with a
channel check, a preamble and an acknowledgement. Packing several frames into one burst was
tried and made transfers fail, so it was reverted; it needs work in the burst and link code.

## Known limits

- The mesh time is long because five engines and two app windows each scan every burst on one
  Mac (about 0.2 s of CPU per scan), and because a relay waits for the hang-up before it calls
  the next station. Not measured on separate machines.
- Two stations that choose to send in the same second can still start together (the link then recovers). The
  backoff only applies after the channel was busy. Over BlackHole the other station's sound
  arrives 0.4 s late, so carrier sense cannot see it in time; on a radio it can.
- Not tested: voice notes (the microphone permission prompt needs a click) and a run longer than
  about 10 minutes.
- BlackHole is a clean path. HF fading and noise are covered only by the simulator benchmarks.

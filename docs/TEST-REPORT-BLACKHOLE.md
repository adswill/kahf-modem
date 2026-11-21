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

## Known limits

- The mesh time is long because five engines and two app windows each scan every burst on one
  Mac (about 0.2 s of CPU per scan), and because a relay waits for the hang-up before it calls
  the next station. Not measured on separate machines.
- Two stations that choose to send in the same second can still start together. The
  backoff only applies after the channel was busy. Over BlackHole the other station's sound
  arrives 0.4 s late, so carrier sense cannot see it in time; on a radio it can.
- Not tested: voice notes (the microphone permission prompt needs a click), files, a
  disconnect and reconnect cycle in the soak, two relays in a row, and a run longer than 150 s.
- BlackHole is a clean path. HF fading and noise are covered only by the simulator benchmarks.

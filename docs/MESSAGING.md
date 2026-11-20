# KAHF messaging layer

Pure-Rust state machines in `kahf_core::msg`, driven by the daemon on its worker clock. Nothing
here does I/O or encrypts content: signatures only prove the sender, every message is readable
on air, and the daemon stays responsible for callsign ID. Every method takes `now` (unix
seconds). State saves as versioned text (`to_text` / `from_text`) so files stay inspectable.

## Facade: `msg::messenger::Messenger`

| Call | Use |
| --- | --- |
| `send(now, to, class, chunks)` | Plan one outgoing message (`to` is a callsign or `#GROUP`). Returns `SendPlan { id, transmit, direct, held, rejected }`. Queue the chunks on air when `transmit`; `held` recipients wait in the mailbox. |
| `mark_sent(now, id, to)` | Link or relay accepted it. |
| `on_heard(now, call, Observation)` | Call for every frame heard. Returns `Release`s: held messages for a station that just (re)appeared. Send them, then `acked`. |
| `acked(now, id, to)` | A released message reached its station; leaves the mailbox. |
| `on_chat(now, from, to, text, chunks)` | `Incoming { show, duplicate, group, receipt, control_reply }`. Show only when `show`; send `receipt` bytes back (direct messages); send `control_reply` (a group roster) to its group. |
| `on_receipt(now, from, bytes)` | Delivered/Read receipt (`extras` 3-byte format). |
| `on_beacon(payload)` | Position or grid beacon. Updates the heard list and returns `(call, grid)`; the daemon then calls `kahf_log::contacts::fill_grid` so contacts get a locator automatically. |
| `tick(now, pos)` | Housekeeping. Returns `Action::{Expired, Resend, Beacon(Due), Identify}`. |
| `snapshot()` / `restore()` | Persist heard list and mailbox. |

UI reads: `heard()` (heard-station list), `mailbox().waiting()`, `tracker().get(id)` (per
recipient status: Queued, Held, Sent, Delivered, Read, Failed, Expired; `overall()` for groups),
`groups().joined()/members()`.

## Features

- **Direct and group chat.** A group is `#NAME` (A-Z, 0-9, `-`, up to 16 chars) in the chat's `to`
  field; no new on-air format. `groups_mut().join("net")` returns a control line to send as chat
  (`KAHFGRP1 JOIN #NET A1AA`); members answer new joiners with a roster. Join/leave must come from
  the callsign they name. Duplicates arriving by two paths are shown once.
- **Offline mailbox.** Messages for stations not heard within 30 min are held (emergency 7 days,
  normal 3, bulk 1; 50 per station, 8 MB total; emergency is never evicted by lower classes) and
  released when the station is heard again after 15 min of silence.
- **Receipts.** Delivered/Read use the existing 16-bit id; the full id is `Messenger::wire_id`
  over the on-air chunks, so both ends compute it. Priority and emergency traffic resends if no
  receipt arrives in 10 min.
- **Files and images.** `msg::airtime::plan` / `label` give chunks and "4 min 20 s" estimates;
  `encode_image_for` picks the best image quality for a time budget (zstd and quantising in
  `session` / `image`). Files are bulk class.
- **Beacons.** `beacon::Scheduler` backs off from 15 to 60 min while stationary, beacons sooner
  after moving 2 km, rate-limits status changes, and reports when a callsign ID is due (10 min).
- **Grid locators.** `grid::beacon_payload(call, gps, grid)` sends a GPS position (receivers derive
  the 6-char grid) or, with no GPS, a grid beacon (`0xA5`/`0xA7` + grid + callsign). The heard
  list stores position and grid either way, so the 17 m route planner can place stations.
  `contacts::fill_grid` fills a contact's QTH only when it is empty.
- **Traffic classes.** `traffic::Class` Emergency > Priority > Routine > Bulk maps to the outbox
  priority; emergency floods all relay paths, preempts lower classes between frames and is exempt
  from relay bulk limits. Emergency text still needs the two-step confirmation in
  `payload_urgent`.

## Daemon API (`kahf_daemon::Engine`)

- `heard_stations(limit)`: stations heard on air, most recent first (`kahf_core::msg::heard::Heard`:
  call, last heard, count, SNR, position, grid, status). Link peers and senders of received
  messages are recorded automatically and saved as `heard.txt` in the data folder.
- `note_beacon(payload)`: feed any location beacon payload heard on air (position or grid beacon).
  Updates the heard list and fills the contact's QTH with the grid when it is empty, so the 17 m
  route planner knows where stations are. Returns `(callsign, grid)`.

- Group chat: `join_group(name, invite)` (invite lists callsigns to tell, for example picked from
  `heard_stations`), `leave_group`, `send_group(name, text)` (one chat per known member through
  the outbox, so members that are off air get it when reached) and `groups()`. A group message
  shows up as an `Event::Message` from the sender with text `#GROUP text`; control lines never
  show. Replies to a join (the member list) are queued and, like all unattended sends, go out only
  while an operator link or relay mode is active. Saved as `groups.txt`.

- Location beacons: `set_beacon(enabled, position, grid)`. Off by default; when on, the worker sends
  a short unconnected burst (frame kind Beacon, rung Q12) at most every 15 minutes, backing off to
  an hour while the position does not change. It carries a GPS position, or a grid locator when
  there is no GPS. Never in listen-only mode, never during a link, needs HF burst mode. Heard
  beacons update `heard_stations()` and fill empty contact QTHs (log line "heard a beacon").
- Delivery: `delivery(public_id)` gives one (callsign, status) per recipient for chats and mail,
  status queued, held, sent, delivered, read, failed or expired. Delivered comes from the link ACK
  when the addressee was the link peer, or from an on-air receipt (frame kind Receipt, 3-byte
  receipt plus the callsign of the sender). A station sends that receipt when a message for it
  arrives from a station it is not linked with (so it came through relays), and only while an
  operator link or relay mode allows unattended sending.

## Not in the daemon (by design)

The offline mailbox stays in `kahf_core::msg` for now: the daemon's outbox already keeps retrying
messages for stations that are off air and survives restarts. Group fan-out uses it too. Status
(presence) beacons and read receipts have encoders but no on-air sender yet.

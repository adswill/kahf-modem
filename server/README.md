# KAHF station map (Cloudflare Worker)

A small Worker for the station map. An operator opts in, and a relay station
that has heard them sends a signed report. The map shows the relay and the
heard station, each as a precise point or as a circle, depending on what the
operator chose to share.

## Privacy rules (enforced in code)

- A location is either a **precise** point (latitude and longitude, at most
  4 decimal places; the client rounds it to about 200 m) or an **area**: a
  centre point plus a whole-number radius of 1 to 100 km. The client picks
  the centre at random inside the radius, so the true position lies within
  the circle.
- Coordinates must be in range (latitude -90 to 90, longitude -180 to 180).
  Other values, unknown fields and any top-level `latitude`/`longitude` field
  are refused.
- A report is accepted only with a valid Ed25519 signature from the reporter's key.
- A callsign belongs to the first key that reports it. Another key gets 409
  until the first key has been silent for 48 hours.
- A station removes its own record with a signed request.
- Records expire 48 hours after their last refresh. KV drops them on its own,
  and the code also filters them out when reading.

Note: this design replaces the earlier grid-square-only rule. Precise points
are now accepted, as the owner asked.

## Endpoints

| Method and path | Purpose |
| --- | --- |
| `GET /` | Home page (`renderHome`). No script. |
| `GET /map` | Live map (`renderMap`). Plain CSS and SVG, one inline script (hashed in the CSP), no map tiles. |
| `GET /stations` | JSON list of non-expired records (calls, locations, times). No keys. |
| `GET /zones` | Busy channels per one-degree area (`busyZones`) and the channel pool split (`poolPlan`). |
| `GET /map-data` | JSON for the map: live relays (area circles) and live station records. No keys. |
| `GET /release` | The current release announcement (`release`, or `null` before the first one). |
| `POST /release` | Store a release announcement. Signed by the owner's release key (see Deploy). |
| `POST /report` | Store or refresh a report. Signed by the reporter's key. |
| `POST /remove` | Delete one of your own records. Signed by the key that made it. |
| `POST /register` | Register a relay: callsign, area and key. Signed by that key. |
| `POST /schedule` | Ask for slots. Signed by a registered relay; returns `makeSchedule` output for that relay. |

Every route answers with JSON or HTML and the matching `Content-Type`. Errors
are JSON (`{"error": "..."}`) with no stack trace. An unknown path gets 404; a
known path with the wrong method gets 405 and an `Allow` header. Request
bodies are capped at 4096 bytes.

The HTML pages send a `Content-Security-Policy` with `default-src 'none'`. The
map page allows exactly one script, by its SHA-256 hash (`mapScriptSha256` in
`src/page.ts`), and nothing else external.

### POST /register

```json
{
  "area": "N51W1",
  "callsign": "K1ABC",
  "public_key": "<base64, 32 bytes>",
  "signature": "<base64, 64 bytes>"
}
```

`area` is a one-degree square named by its south-west corner (`areaKey`).
Responses: `200 {"ok":true,...}`, `400` (bad callsign or area), `401` (bad
signature), `409` (callsign registered with another key). A registration lasts
48 hours unless the relay registers again.

### POST /schedule

```json
{
  "public_key": "<base64, 32 bytes>",
  "relay": "K1ABC",
  "requests": [
    { "channel": 4, "duration": 60, "direction": "tx" },
    { "channel": 4, "duration": 30, "direction": "rx", "urgent": true }
  ],
  "signature": "<base64, 64 bytes>"
}
```

The signed fields are `relay` and `requests`. Up to 20 requests. The response is
`{"relay": "K1ABC", "generated_at": "...", "slots": [...], "unplaced": [...]}`, where `slots`
is the planner output for this relay. Urgent requests (`"urgent": true`) are planned first.
A request may set `latency_budget_s` (whole seconds, 0 to 3600): the latest start it accepts,
counted from the server time. A request that cannot start inside its budget is not booked and
is listed in `unplaced` with a reason. Responses: `200`, `400` (bad request, with
the reason), `401` (bad signature), `403` (another key), `404` (relay not
registered).

Limit: each call plans only the requests it carries, against the other live
relays in the area as they are registered. Two relays that ask in separate
calls are not checked against each other's earlier slots.

### Band

All channel numbers come from one constant, `BAND_SEGMENT_KHZ` in
`src/core.js` (lower edge included, upper edge excluded), and the channel width
`CHANNEL_WIDTH_KHZ` (2.7 kHz). `BAND_CHANNELS` is derived from them. The segment
values are a working choice and must be confirmed before deployment.

### POST /report

```json
{
  "frequency_khz": 14074,
  "heard_at": "2026-11-17T11:59:00Z",
  "heard_call": "DL1XYZ",
  "heard_location": { "mode": "area", "latitude": 48.8566, "longitude": 2.3522, "radius_km": 50 },
  "mode": "FT8",
  "public_key": "<base64, 32 bytes>",
  "reporter_call": "K1ABC",
  "reporter_location": { "mode": "precise", "latitude": 51.5074, "longitude": -0.1278 },
  "signature": "<base64, 64 bytes>"
}
```

- `heard_at` is a real ISO 8601 UTC time ending in `Z`: not more than 10 minutes old and not more than 5 minutes in the future.
- `public_key` must not be a small-order point (all zeros, for example). Callsigns are ASCII only.
- `frequency_khz` is a positive integer.
- `mode` (the radio mode, not the location mode) is 1 to 24 characters: letters, digits, space, `+ . _ / -`, no leading or trailing space.
- A precise location has `mode`, `latitude`, `longitude` only. An area location adds `radius_km` (whole number, 1 to 100).
- Responses: `200 {"ok":true,"expires_after_s":172800}`, `400` (bad input), `401` (bad signature), `409` (callsign held by another key), `413` (too large).

### POST /remove

```json
{
  "heard_call": "DL1XYZ",
  "public_key": "<base64, 32 bytes>",
  "reporter_call": "K1ABC",
  "signature": "<base64, 64 bytes>",
  "signed_at": "2026-11-17T12:00:00Z"
}
```

`signed_at` is covered by the signature and must be within 5 minutes of the server time (past or future). A removal can be used once; replaying it returns `409`.

Responses: `200 {"ok":true,"removed":true}`, `400`, `401`, `409` (replayed), `403` (the record belongs to another key), `404` (no such record).

## Signing

1. Build the object of the fields you send, **without** `public_key` and `signature`.
2. Serialise it as canonical JSON: object keys sorted alphabetically at every
   level (including inside the location objects), no spaces, UTF-8. Write
   numbers in plain form, as JavaScript `JSON.stringify` does.
3. Sign those bytes with Ed25519 (raw 32-byte public key, 64-byte signature).
4. Send the fields as you signed them, plus `public_key` and `signature`, both base64.

For a removal, the signed object holds `heard_call` and `reporter_call` only.

## Storage

Workers KV, binding `STATIONS` (see `src/store.ts`). Keys:

- `rec:<REPORTER>><HEARD>` holds a station record, with a 48 hour TTL.
- `owner:<REPORTER>` holds the key that owns a reporter callsign, with the same TTL.
- `relay:<CALLSIGN>` holds a registered relay (area and key), with the same TTL.

KV was chosen over a Durable Object because the data is small, written rarely,
read often, and expires on its own. Reads can lag writes by about a minute,
which is fine for a map. To change the backend, replace `kvStore()` in
`src/store.ts`; nothing else touches storage.

## Layout

```
src/core.js     pure logic: callsigns, locations, canonical JSON, expiry, validation,
                Ed25519 check, band, busy zones, pools, schedule planning
src/routes.js   request routing for every endpoint; takes the store as an argument
src/store.ts    storage interface and the KV implementation
src/page.ts     HTML/SVG home and map pages, and the map script hash
src/index.ts    Worker entry point (binds the KV store to routes.js)
src/types.ts    shared types
test/core.test.mjs    reports, removals, locations, signatures
test/zones.test.mjs   busy zones, pools, schedules
test/band.test.mjs    band segment, channels, areas, register and schedule validation
test/page.test.mjs    home and map pages
test/routes.test.mjs  every route, against a fake store and a fixed clock
test/section5.test.mjs  latency budgets, release announcements, relay index and map data
```

`src/core.js` is plain JavaScript so that `node --test` runs it on Node 20
without a compiler. Wrangler bundles it into the Worker. There are no runtime
dependencies.

## Tests

```
cd server
npm test
```

The tests need only Node 20 and make no network calls or cloud calls. They cover
callsigns, location validation, circle geometry, canonical JSON, expiry, base64
sizes, Ed25519 signing, report and removal validation, busy zones, pools,
schedules, the band constant, registration and schedule validation, the pages,
and every route through a fake store (including the CSP hash of the map script,
the 404/405/413/500 answers, and that a 500 leaks no detail).

## Deploy

Deployment creates a cloud resource, so run these by hand:

```
cd server
npx wrangler kv namespace create STATIONS
# paste the printed id into wrangler.toml in place of REPLACE_WITH_KV_NAMESPACE_ID
npx wrangler deploy
```

The release key is the public half of the owner's Ed25519 key that signs release
announcements. Set it as a secret, not in the code:

```
cd server
npx wrangler secret put RELEASE_PUBLIC_KEY
```

Until it is set, `POST /release` answers `503`. The owner runs these steps by hand.

Local development: `npx wrangler dev`.

## HF backbone for relays

Relays are always online and operators are radio-only, so relays carry messages for each
other through this server (docs/NETWORK-SCENARIOS.md, section H). Code: `src/backbone.js`.

| Endpoint | Purpose |
| --- | --- |
| `POST /relay/heard` | `{relay, heard:[{callsign, snr?, at}], ts}`: records which relay last heard each callsign (kept 2 h from `at`; 1 to 200 entries). |
| `POST /msg/submit` | `{relay, id, origin, dest, kind:"chat"\|"mail", subject?, body, created, ttl_s, hops, urgent?}`: the first submit of an `id` (`ORIGIN-<counter>`) wins and is queued for `dest`; a repeat returns `{ok:true, duplicate:true}` and stores nothing. `body` is base64, at most 1024 bytes (chat) or 4096 (mail). `ttl_s` is clamped to 30 min .. 48 h. |
| `POST /msg/pull` | `{relay, callsigns:[...up to 100], ts}`: queued messages for the callsigns the relay hears now (at most 20, urgent first, then oldest) plus pending receipts for those callsigns as origin. Handed messages not receipted within 10 minutes can be pulled by any relay again. |
| `POST /msg/receipt` | `{relay, id, status:"delivered"\|"failed", reason?}`: closes the message, removes the payload, stores a short receipt (24 h) for the origin. Only the relay holding the message may send it. |
| `GET /relay/wanted` | Public: `{wanted:[callsigns]}` with queued messages (max 500), for beacons. |

Signing is the same as `/register` and `/schedule`: the JSON body carries every field above
plus `public_key` (base64, 32 bytes) and `signature` (base64, 64 bytes). The signature is
Ed25519 over the canonical JSON (keys sorted, no whitespace) of the body without
`public_key` and `signature`. The relay must be registered, and `public_key` must be the key
it registered. `ts` is unix seconds and must be within 10 minutes of the server clock.
Backbone requests may be up to 16 KB; a relay may make 600 backbone calls a minute (429 after).
An expired message gives its origin a `failed` receipt with reason `expired`.

`/schedule` plans transmit slots for the whole world: two relays never get overlapping
transmit slots on one channel, whatever their area (HF skywave is global). A request that
would overlap is shifted later, or left in `unplaced` when its latency budget is too short.

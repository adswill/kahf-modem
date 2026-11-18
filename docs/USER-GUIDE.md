# KAHF user guide

KAHF sends chat, mail with attachments, files and voice notes between computers over HF amateur radio on the
**17 metre band**, with no internet. This guide describes what the current build does. **It has never been used on a real
radio channel**, and the app will not transmit until the 17 m table has been checked against the current band plan
(`docs/ON-AIR-PLAN.md`). Anyone can listen; only a licensed amateur may transmit.

## First start

A short wizard asks for your callsign (or "listen only" if you have none), your radio connection (a sound card, or an
SDR such as HackRF or RTL-SDR) and your place. It then starts on lane **C1**, the calling lane. KAHF uses 17 metres
only, in 12 narrow lanes between 18.111 and 18.120 MHz; you do not choose a band.

## Setting up the radio

Set the transceiver to **USB** and to the **dial frequency shown on the Radio screen** (for C1: 18.11344 MHz). That
dial puts the lane at 1500 Hz of audio. KAHF refuses to transmit on any other dial, because a wrong dial would put the
signal on top of other lanes. With a rig control program the app can set it for you.

Set the transmit level with the test tone on a dummy load first: the radio's ALC must not move. Over-driving a radio
spreads the signal into the neighbouring lanes, which are only 220 Hz away.

## Sending

- **Chat.** Type a callsign and a message and press Send. Up to 400 bytes goes out without a connection, as small coded
  frames. The other station answers once; the app shows waiting, sending, delivered, or failed with the reason, and
  tries again by itself.
- **Mail.** Subject, text and attachments, signed by you so the receiver can check who wrote it.
- **Files.** Up to 100 kB, direct only. The app shows how long it will take before you send.
- **Voice notes.** Short clips, squeezed by the Codec2 voice codec.

Nothing is encrypted. Messages marked private are only shown to the station they are addressed to, but are readable on
the air.

## What "slow" means

Each lane is the KAMI modem slowed five times to fit 480 Hz. At the fastest mode a lane carries about 1000 bit/s;
at the most robust about 8 bit/s. A short chat takes seconds on a good path and minutes on a poor one. These are figures
from the modem, not measurements on a real radio.

## When 17 metres is closed

17 m is a daytime, long-distance band: roughly 1000 to 4000 km by day; the first several hundred to a thousand
kilometres are often silent, and at night the band usually closes. The app keeps your message and listens.

## Never transmitting by accident

- **Listen only** turns every transmit path off.
- Nothing is sent without your action (or, for a relay, without relay mode being on).
- Before every transmission the app checks the lane, and both neighbours, for other signals and waits if they are busy.
- It yields to anyone: voice, CW, digital modes or other KAHF stations.

## Relay mode

See `docs/RELAY-GUIDE.md`.

## Your responsibility

KAHF is for licensed amateur radio operators. You are responsible for staying inside your licence: bands, modes, power,
identification and rules on third-party traffic differ from country to country. KAHF asks you to confirm each
transmission. It does not decide for you.

Location sharing on the map is off by default. If you turn it on you share a point accurate to about 200 m, or a
circle of 1 to 100 km with your real position somewhere inside.

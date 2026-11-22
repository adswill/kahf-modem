# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users
Anyone with a radio can listen: KAHF decodes what it hears without a licence. Only people with an amateur radio licence may transmit or run a relay station. The app is used at a computer connected to an HF radio or SDR (sound-card cable or receiver), often with no phone signal or internet. Users are not assumed to know modem or radio-engineering terms.

## Product Purpose
KAHF is a desktop app that sends chat messages, mail with attachments, files and voice notes between computers by amateur radio on HF, with no internet needed. Success is a message that arrives once, in order, and a person who always knows what the radio is doing and never transmits by accident.

## Positioning
Its own modem (KAMI, Walsh signalling, slowed to a 480 Hz lane on 17 metres, twelve lanes in 18.111 to 18.120 MHz) plus a relay network: licensed relay stations and a backbone server (kahf.dev) pass messages on to stations that cannot hear each other. One app covers chat, mail, files and voice. Radio-only operation is complete without the server.

## Operating Context
Two-way over HF with real conditions: both sides sending at once, fading, the link closing mid-conversation, a station hearing its own transmissions. The owner tests with two app instances over a BlackHole virtual audio cable. Relay mode requires a licence and a shared location to register.

## Capabilities and Constraints
- Desktop app: a Tauri window (Svelte front end), not a phone app or website.
- Works fully offline; the server, map and relay backbone are optional extras.
- Never transmit by accident: listen-only mode, licence reminder, transmit level setting and test tone stay prominent.
- Files go direct only, 100 kB limit, with a time estimate shown.
- Built and tested only on a bench or in simulation: lane mode, lane sensing, chat on bundle frames, the relay as referee of one lane. Not built or not joined (do not present as working): server-planned slots, routing and paging through the server, hopping, wider signals over several lanes, any test on a real HF channel. 17 m only: never mention 80, 40 or 20 m.
- Wording to the owner: plain, short, honest about what is not verified.

## Brand Commitments
Name KAHF; modem KAMI. Callsigns and frequencies are shown in a fixed-width face. The owner dislikes the dark, "hacker" look and the orange scheme, and a generic shop-template look: the website is a printed plotting-sheet world (pale chart blues, navy ink, one chart magenta) with developer detail (commands, test output, frame layout).

## Evidence on Hand
Measured results are in docs/NETWORK-STATUS.md and docs/KAMI-MODES.md. No user testimonials or field reports from real HF exist; do not invent any.

## Product Principles
1. A person can always tell what the radio is doing (listening, receiving, sending).
2. Transmitting is deliberate; nothing sends by accident.
3. Plain words over radio jargon; say what is not verified.
4. Everything essential works with no internet.
5. Each message arrives once, in order, and shows a clear status.

## Accessibility & Inclusion
Plain language for non-technical users. No product-specific standard was set.

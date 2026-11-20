# KAHF User Guide

This guide is for radio amateurs who have not used KAHF before. It describes only what the
current code does. Anything that is not connected yet is marked **coming later**.

## 1. What KAHF is for

KAHF is an HF data modem. It sends and receives digital data over a radio link. You use it to:

- connect to another station and hold a link with it,
- send short messages,
- send mail,
- send a file.

KAHF is in early development. The modem is not yet ready for on-air use. Test it with a
dummy load or a station you have agreed to test with, and only transmit when your licence allows
it on the frequency you use.

## 2. Before you start

- You need your own amateur licence and callsign.
- You need a radio, a sound-card interface between the radio and your computer, and a way to
  set the radio to a data mode.
- Everything KAHF sends goes out under your callsign. Check the frequency is clear before you
  connect.

## 3. Install and start KAHF

Download the archive for your system from the Releases page of the repository, together with
`SHA256SUMS.txt`. Check the download with `sha256sum -c SHA256SUMS.txt` (macOS: `shasum -a 256 -c`).

- **macOS (Apple silicon):** unzip, move `KAHF.app` to Applications. The app is not signed yet.
  Open it with right-click, Open.
- **Windows x64:** unzip and run `KAHF.exe`. If Windows warns, choose More info, then Run anyway.
- **Linux x64:** unpack the archive and run `./kahf`.

Each archive also holds `kahf-cli`, a command-line tool. KAHF does not update itself. Download a
new release when you want one.

To build from source instead, see `docs/BUILD.md`. The desktop app is the `kahf-ui` crate:
`cargo run --release -p kahf-ui`.

Settings are saved in `%APPDATA%\kahf` on Windows and `~/.config/kahf` elsewhere.

## 4. First-run setup

The first time KAHF starts, and whenever no callsign or no receiving path is set, a **Set up KAHF**
screen opens. Work through it from the top. You can change every choice later in **Settings**,
which also has **Run the full setup**.

- **Callsign.** Your own callsign. It is sent with every transmission, and setup will not finish
  without it.
- **Rig control.** None (you tune by hand), Serial CAT through a rig control program (rigctld),
  or VOX. For CAT, enter the rigctld address as `host:port`, for example `127.0.0.1:4532`.
- **Receiver.** A sound card, or an SDR such as a HackRF or RTL-SDR.
- **Audio input** and **Audio output.** Pick your sound-card devices from the lists. KAHF lists the
  devices it finds.

Setup shows each step as **Done** or **Needs attention**. KAHF starts in **Listen only**, so nothing
is transmitted until you turn that off in Settings or the top bar.

**Show sample data** in Settings is off by default. Leave it off for real use; turn it on only to
look at the screens with example stations and messages.

## 5. The main screen

The window has these parts.

- **Top bar.** Your callsign, the link status, the frequency in large numbers, the band and
  mode, the transmit and receive state, a **Listen only** button, and the UTC clock.
- **Left sidebar.** The sections: Operate, Stations, Messages, Mail, Files, Relay, Logbook, Map
  and Settings.
- **Centre.** The spectrum and the waterfall, with the messages under them. The spectrum shows
  signal strength across the frequencies. The waterfall shows the same signals over time.
- **Right column.** The link state, the transmit state (queue, last transmit, speed and frame
  error rate), the channel (frequency and source), the audio level, the hops for the last
  message, and the event log.
- **Status bar.** Plain-language messages about what KAHF has just done or refused to do.

The older `kahf-gui` window is still in the code but is no longer released.

> **Note.** Sections 6 to 13 were written for `kahf-gui` and are being revised for the current
> app. Button names and where things sit can differ. The rules they describe (confirm before
> transmitting, Listen only, private is not encryption) still hold.

## 6. Connect to a station

1. Type the other station's callsign in the **Connect** box. The box checks the format.
2. Click **Connect**. A window called **Transmit and connect?** opens. It shows:
   - the callsign that will be sent (yours),
   - the station you are calling,
   - the frequency,
   - the mode (QPSK).
3. Read it. Click **Transmit and connect** to send the connection request, or **Cancel** to
   stop. If you cancel, the status bar says **Connection cancelled. Nothing was sent.**

After you confirm, the link line moves through these steps:

- **Connecting to XX1XX. Waiting for the other station.** Your radio has sent the request and
  KAHF is waiting for a reply.
- **Connected to XX1XX.** The other station has answered and the link is open.
- **Disconnecting.** The link is closing.
- **Not connected.** The link is closed.

If there is no reply, KAHF gives up after about 6 seconds. The event log then shows
**No reply to the connection request**.

To close the link, click **Disconnect**. A window asks **Disconnect from XX1XX now?**. Click
**Disconnect** again to close it, or **Stay connected** to keep it open.

## 7. Send a message, mail and a file

First choose a station in the **Stations** list, or type its callsign and connect. The
**Messages** area then shows the station you are talking to.

### Message

1. Make sure **Message** is selected above the text box.
2. Type your text. Press **Ctrl+Enter** or click **Send**.
3. The message is added to the outbox. The outbox count shows how many are still waiting.

### Mail

1. Click **Mail**.
2. Enter a **Subject**. The send button stays off until there is a subject.
3. Type the mail text.
4. To add a file, click **Attach**, type its full path, and click **Add to mail**.
5. Click **Send mail**.

### File

1. Click **Send file** (in Message mode).
2. Type the full path of the file.
3. Click **Send file**. If the path does not point to a file, KAHF says
   **No file at that path. Check the path and try again.**

Progress appears in the file list. Each item shows whether it is waiting, active, paused,
done or cancelled.

### The confirmation before transmitting

Connecting shows the **Transmit and connect?** window described in section 6. Nothing is sent
until you click its **Transmit and connect** button.

In this build, messages, mail and files have no separate confirmation window. Clicking **Send**
queues them for sending. Before you click, check the frequency, that your radio is in the right
mode, and that **Listen only** is off if you do want to transmit.

## 8. Listen only

**Listen only** means KAHF never transmits. When it is on:

- **Connect** is disabled.
- **Send** is disabled.
- Nothing leaves your radio, even if a message is waiting.

Turn it on with the **Listen only** button in the top bar, or with the tick box in Settings
under **Transmit**. The status bar confirms the change. Turn it off again to transmit.

Use Listen only when you only want to hear what is on the channel.

## 9. The relay role

A relay passes messages between stations that cannot hear each other. A relay station works
to a schedule. Only the relay-role parts of the code are written; the desktop app does not yet
let you set the role or see a schedule. **Coming later.**

When it is available, a relay station asks the operator to:

- keep the radio on for the times in the schedule,
- accept or decline a schedule before it runs,
- follow the send and receive slots. A radio that cannot receive while it transmits must take
  turns.

Do not take on the relay role unless you can keep the station running as agreed.

How radio relay works in the engine:

- A message waits two minutes for its station. If the station is not reached and you have named
  relays, the log says **Could not reach XX1XX on air. It goes by radio relay through ... once
  connected.** Connect to one of those relays and the message goes to it.
- A relay in relay mode, with **Relay other stations' messages** on, holds a message for another
  station. In its next transmit slot it calls that station, passes the message on exactly as
  heard (the signature and the private mark stay), and hangs up when the station has
  acknowledged it. A relay does not show these messages in its own inbox.
- Each message is passed on once. Copies heard again and messages that loop back are dropped.
  A relay holds at most 64 messages for other stations.
- A relay that cannot reach the station either passes the message to its own relays, so a
  message can cross several relays (up to four hops are planned).
- Relays work on 17 m by default, 18.111 to 18.120 MHz, in three 2.7 kHz channels. IARU Region 1
  marks this part of 17 m for digital modes and automatically controlled data stations. It sits
  above the 18.110 MHz beacon and clear of FT8 and FT4. Check your own country's rules.
- Which relays to name can come from the route planner, which estimates when 17 m is open
  between each pair of stations: ground wave up to about 30 km, sky wave from the MUF and LUF.
  Between those two there is a skip zone, where two nearby stations cannot hear each other on
  17 m and need a distant relay. The planner is an estimate and has not been checked against
  real contacts yet.

## 10. Location sharing

Location sharing decides how much of your position other stations and a map may learn. There are
three choices:

- **Off.** Your position is not shared. This is the default.
- **Precise.** Your position is shared to about 200 m. Anyone who can see the map can see it.
- **Area.** The map shows a circle with a radius you choose, from 1 to 100 km (50 km by default).
  Your position is somewhere inside the circle.

In this build the choice is saved in the settings file, but nothing uses it yet. No position is
sent. The map is not in the desktop app. **Coming later.**

## 11. Internet relay

Internet relay would let a message travel over the internet when a station cannot be reached on
air. It is **off by default**. The engine has the switch for it, but the internet gateway is not
available yet, so it cannot be turned on in this build. **Coming later.**

When a message cannot reach its station on air, the status and event log say:

> Could not reach XX1XX on air. The station did not respond on any relay path. Try again later,
> or enable internet relay in Settings.

This means KAHF waited about 2 minutes for the station and got no answer on any path. It is not
an error in your setup. Try again later, or check that the other station is on frequency and
listening.

If internet relay were on and failed, the message would read
**Could not reach ... and internet relay failed**. The message then stays in the outbox.

## 12. Signed messages

A signed message carries a signature made with the sender's key. The signature proves:

- the content was not changed after it was signed,
- it was signed by the key of the station named as the sender.

The signature does not:

- hide anything. The text is still readable.
- prove who is at the keyboard.
- prove that the sender is honest or that the message is true.

The desktop app does not sign or check messages yet. **Coming later.**

## 13. Private messages are not encryption

A **private** message is a routing rule. Only the addressee's station keeps it in its inbox. It
is **not encryption**. The words are readable on air, and anyone with a receiver and the same
mode can read them. Do not send anything on air that you would not say in plain speech.

The engine can send a message marked private. The desktop app does not yet show the choice.
**Coming later.**

## 14. Updates

KAHF checks for updates only when you ask. It does not update itself without asking.

There are two kinds of update prompt in the code:

- **Optional update.** You can install it now or skip it. KAHF keeps working.
- **Required update.** KAHF cannot transmit or receive until the update is installed. If you
  choose to skip a required update, the **Accept** button stays off for 10 seconds. After you
  accept the skip, KAHF will not transmit or receive until the update is installed.

A message shown while a required update is pending says:
**A required update is pending. Install it before you transmit.**

The update prompt is not yet connected to the desktop window. The Settings line says
**Update checks are not connected yet.** **Coming later.**

## 15. Troubleshooting

### No audio device

The status bar says **Audio device not found. Choose one in Settings.**

- Check that the sound card is plugged in and that the radio's audio cable is connected.
- In Settings, set **Audio input** and **Audio output** to **System default**, then click **Save**.
- Check the level of the signal from your radio. The level must not be too high or too low.

### No reply

The event log says **No reply to the connection request** after about 6 seconds.

- Check the callsign you typed is correct.
- Check the frequency and mode on your radio match the other station's.
- Check the other station is on, listening, and using KAHF.
- Check the audio levels in both directions.
- Try again. Wait a few minutes between attempts so you do not keep sending into a busy channel.

If messages are queued but never leave, the **Could not reach** message in section 11 explains
what KAHF did.

### A busy channel

KAHF sends only after you confirm, and it waits if it hears another signal on the channel. If the
channel is busy, nothing is sent yet. The outbox count stays up until the channel is clear.

Check the spectrum and waterfall. If the channel is busy with other signals, choose another
frequency or wait for it to clear.

### Other messages

- **Cannot reach the rig control program.** Check the rig control address in Settings, and that
  the rig control program is running.
- **The data folder cannot be used.** Choose another folder in Settings.
- **The radio engine has stopped.** Restart KAHF to use the radio again.
- **Could not save the settings.** Check that the KAHF folder can be written.

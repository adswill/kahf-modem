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

## 6. Connect to a station

You do not need a separate connect step to send a message. Open **Operate**, type the other
station's callsign in **To**, write the message and press **Send**. The app connects for you.

To connect without sending, type the callsign in **Station** in the right column and choose
**Connect...**.

Either way, a **Transmit now?** window opens first. It shows the action, the station, your
callsign and the frequency. Read it. Choose **Transmit** to go ahead, or **Cancel** (or press
Escape). Nothing is sent until you choose **Transmit**.

The **Link** panel shows the state: **No link** or **Linked**. If the other station does not
answer, the event log says so and nothing is left open.

To close a link, choose **Disconnect**. A **Disconnect?** window warns that anything still being
sent will stop. Choose **Disconnect** again, or **Keep link** to stay connected.

**Send beacon** sends a short call so other stations can hear you. It asks for the same
confirmation.

## 7. Send a message, mail and a file

### Message

1. Open **Operate**. In **To**, type the callsign.
2. Write the text. The limit is 500 characters.
3. Choose a **Kind**: Chat, Urgent, Signed or Private (see sections 12 and 13).
4. Press **Send** or **Ctrl+Enter**, then confirm in the **Transmit now?** window.

**Messages** lists your conversations. A message that cannot go out yet stays in the outbox.

### Mail

1. Open **Mail** and choose **Write mail**.
2. Fill in **To**, **Subject** and the text. Tick **Sign this mail** to sign it.
3. To add a file, choose **Attach file** and enter its path.
4. Choose **Queue for sending**. The mail goes out when you confirm the transmission.

Received mail appears in the inbox. Choose a message to read it.

### File

1. On **Operate**, choose **Attach file** (or **Send a file** on the Files page).
2. Type the full path of the file and choose **Send file**.
3. Confirm in the **Transmit now?** window.

**Files** lists each transfer with its size, progress and state: Waiting, Active, Paused, Done or
Cancelled. You can **Start**, **Pause** or **Cancel** a transfer there. Every file carries a
checksum, so a damaged file is detected.

### Stations, logbook and the map

- **Stations** lists stations heard, with last heard time and how often. Use the filter box to
  find one.
- **Logbook** records each completed link. **Export ADIF** writes `logbook.adi` to the data
  folder, ready to import into logging software.
- **Map** opens the live map at https://kahf.dev/map in your browser. The app does not draw a
  map itself.

## 8. Listen only

**Listen only** means KAHF never transmits. KAHF starts with it on. While it is on, nothing leaves
your computer, even if a message is waiting, and the right column says **Transmit is off because
Listen only is on**.

Turn it off or on with the **Listen only** control in the top bar, **Ctrl+L**, the command palette
(**Ctrl+K**), the **Turn off Listen only** button in the right column, or **Settings**.

## 9. The relay role

A relay passes messages between stations that cannot hear each other. It works to a schedule.

1. Open **Settings** and set **Role** to **Relay** or **Station and relay**.
2. A **Relay mode** window explains the legal position. In some countries an automatic station
   needs a licence. In Finland it needs a permit from Traficom. Tick both boxes (your licence
   and the terms) and choose **Start relay**. KAHF asks again every time relay mode starts.
3. Open **Relay**. It shows the **Schedule**, **Statistics**, **Waiting to send** and the
   **Hop log** of messages forwarded.
4. A schedule offered by the network must be accepted before it runs. Choose **Accept** or
   **Decline** on each one. A relay sends only inside an accepted slot.

Do not take on the relay role unless you can keep the station running as agreed. The relay
works on the 17 m relay segment, and the radio must take turns between receiving and sending.

## 10. Location sharing

Location sharing decides how much of your position other stations and the map may learn. Set it in
**Settings**, **Location sharing**:

- **Off.** Nobody sees your location. This is the default.
- **Precise.** Your exact position is shared.
- **Area.** Only a circle around you is shared. Set **Area radius** from 1 to 100 km.

The choice is saved with your other settings.

## 11. Internet relay

Internet relay lets a message travel over the internet when a station cannot be reached on air. It
is **off by default**. Turn it on with **Use internet relay** in **Settings**.

When a message cannot reach its station on air, the status says so and the message stays in the
outbox. Try again later, or check that the other station is on frequency and listening.

## 12. Signed messages

A signed message carries a signature made with your station key. The signature proves:

- the content was not changed after it was signed,
- it was signed by the key of the station named as the sender.

The signature does not:

- hide anything. The text is still readable.
- prove who is at the keyboard.
- prove that the sender is honest or that the message is true.

Choose **Signed** as the message **Kind**, or tick **Sign this mail**. Your key fingerprint is
in **Settings**, **Station**. Other stations use it to check your signed messages. The label on a
received message says who signed it.

## 13. Private messages are not encryption

A **Private** message is a routing rule. Only the addressee's station keeps it in its inbox. It
is **not encryption**. The words are readable on air, and anyone with a receiver and the same
mode can read them. Do not send anything on air that you would not say in plain speech.

## 14. Updates

KAHF checks for updates only when you ask. It never updates itself.

Open **Settings**, **Updates**, and choose **Check for updates**. The line above the button shows
the result: **Checking...**, **You have the newest version.**, or a message that the check failed.
If a newer version exists, KAHF says so and opens its download page in your browser. Download and
install it yourself, and check the file with `SHA256SUMS.txt`.

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

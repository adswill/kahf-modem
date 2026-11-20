# KAHF 1.0

KAHF is an HF and VHF messaging and file-transfer program for licensed amateur radio operators. It works with ordinary radios through rig control and with software-defined radios for receiving.

## What is in 1.0

- Messages, mail with attachments, and file transfer with a checksum on every file.
- Signed messages, with a label that says who signed them.
- A modem with forward error correction that decodes two-path fading at 1 dB with under 1 percent frame loss.
- Receive from HackRF, RTL-SDR, and the other radios listed in the interface, with the library loaded when it is needed.
- Transmit through a HackRF or a rig, with a confirmation before every transmission.
- Listen only, on by default in the interface.
- A relay mode that starts only after the operator confirms the licence and accepts the terms, each time it starts.
- A live map at https://kahf.dev/map that updates every second.

## Before you use it

- The program warns when a frequency is outside the amateur segments of the European band plan. It never blocks transmitting.
- Some countries require a licence or permit for automatic stations. In Finland, an unattended relay needs one. Relay mode is your responsibility.
- The download is not yet signed with a certificate from Apple or Microsoft. Your computer will show a security warning. On macOS, open the app with right-click, Open. On Windows, choose More info, then Run anyway.

## Known limits in 1.0

- Official builds are for macOS on Apple silicon and Linux on x86-64. Windows, Intel Macs and ARM64 Linux follow later.
- Only the RTL-SDR v4 and HackRF Pro have been tested on real hardware. The other radio drivers are tested against fake servers.
- Transmit is through HackRF or a rig. The other radios receive only.

## Licence

GNU General Public License, version 3 or later. KAHF is free software: you can use, study, change and share it, and changed versions must be shared under the same licence with their source. See LICENSE.

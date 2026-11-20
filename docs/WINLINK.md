# Winlink bridge

kahf mail and Winlink mail are the same mail. A kahf callsign is a Winlink callsign, so
`K1ABC` on kahf is `K1ABC@winlink.org` on Winlink: nobody changes address, nothing is
re-sent, and a Winlink user can move to kahf without losing a message or an attachment.

The `kahf-winlink` crate speaks the real protocols (B2F, LZHUF, secure login), checked against
`wl2k-go`, the library Pat is built on (`scripts/winlink-interop.sh`).

## What you get

- **Same address.** Mail to `CALL@winlink.org` reaches your kahf mailbox; kahf mail to a
  callsign or an email address (`bob@example.com`) goes out through Winlink.
- **Keep your history.** The mailbox on disk is laid out exactly like Pat's
  (`<root>/<CALL>/{in,out,sent,archive}/<MID>.b2f`). Point `--root` at an existing Pat mailbox
  (or a Winlink Express export folder) and every message is already there.
- **Attachments and Cc** travel intact, with non-ASCII text (UTF-8).
- **Pat and Winlink Express keep working.** A kahf station answers telnet peer-to-peer calls
  (port 8774) from both, and can call them.
- **A gateway for a whole kahf network.** One station with internet syncs with the Winlink CMS
  for every kahf callsign it serves, so stations on the 17 m relay network send and receive
  Winlink mail over radio, with no internet of their own.

## Commands

```text
kahf-winlink gateway --call K1ABC --serve W1AW,N0CALL --root ~/kahf/mail --interval 300
kahf-winlink listen  --call K1ABC --root ~/kahf/mail            # Pat / Winlink Express P2P
kahf-winlink call    host:8774 --call K1ABC --target N0CALL --root ~/kahf/mail
kahf-winlink send    --root ~/kahf/mail --from K1ABC --to N0CALL,bob@example.com \
                     --subject "Hello" --body note.txt --attach photo.jpg
kahf-winlink list    --root ~/kahf/mail --call K1ABC --folder in
```

The Winlink account password is read from `WINLINK_PASSWORD` or `--password-file`, never from
the command line. `--serve` lists extra kahf callsigns the gateway receives mail for; they are
sent to the CMS as auxiliary addresses in the same session.

## From the kahf side

`kahf_winlink::bridge` converts between kahf's mail model and Winlink: `join` and `split` fold
kahf's separate mail and file messages into one message with attachments and back,
`to_winlink` builds the Winlink message (and removes the kahf signature block, which Winlink
recipients cannot check), and `from_winlink` produces one kahf mail per served callsign the
message is addressed to. `Spool` is the on-disk mailbox a gateway reads and writes; a daemon
hands it kahf mail for Winlink addresses (`queue`) and takes delivered mail from `in`.

## Rules and limits

- **No encryption.** Nothing is encrypted or obscured. Secure login proves the account
  password with a challenge response; the password is never sent. Callsigns identify stations.
- **CMS terms.** Using the CMS needs a Winlink account and its password. Run a gateway only
  with your own account, and only forward mail for stations whose operators have asked for it.
  Third-party traffic rules differ by country; the operator is responsible for them.
- **Size.** A message (body plus attachments) is limited to about 120 kB.
- **Not covered.** Messages are not forwarded as a transparent internet link for other radio
  traffic; only mail is bridged. Offset resume of partial messages and the gzip experiment
  are not supported (the peer then resends or defers).
- **Real CMS.** The CMS login path is written to the published protocol and tested against a
  stand-in plus `wl2k-go`; it has not been run against the live CMS from this repository's CI.

## Credits

The LZHUF port and the secure-login salt follow the MIT-licensed `wl2k-go` by Martin Hebnes
Pedersen (LA5NTA), the library behind Pat. The protocol itself is Winlink's B2F/FBB.

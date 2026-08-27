# Security

## Reporting something

**Please report privately rather than opening an issue.** Use
[Report a vulnerability](https://github.com/scottarius/TinePlayer/security/advisories/new),
which is GitHub's private channel and reaches me without the report being
public first.

Useful to include, roughly in order:

- What an attacker can do, and what they need in order to do it - being on the
  same network, having an account on the machine, persuading somebody to open
  a file
- The platform and the TinePlayer version, from **Settings → About**
- The log, if it is relevant: see
  [Log file](https://tineplayer.app/docs/settings/where-data-is-saved/#log). Tokens, your account
  name, the folders videos sit in and any server address outside your own
  network are removed before it is written; file names are kept

I am one person and this is not my full-time work, so I will not promise a
response time. I will confirm I have read it, say whether I think it is a real
problem, and tell you what I intend to do. If you would like credit in the
release notes, say so and how you would like to be named.

## What is supported

The most recent release. TinePlayer is small enough that there are no
maintenance branches: a fix goes into the next release rather than being
backported.

## What is already known, and why

Three things look like vulnerabilities, get reported as such, and are
deliberate. Reporting them anyway is welcome if you think the reasoning is
wrong - but the reasoning exists.

**The Jellyfin access token is stored in a file your account can read.**
`jellyfin.json` in the user data folder, `0600` where the platform can say so.
It is not encrypted and not in the system keyring, and obfuscating it would be
theatre: whatever TinePlayer can read unattended, so can anything else running
as you. The keyring was considered and rejected for a specific reason rather
than for convenience - TinePlayer often runs on a Raspberry Pi wired to a
television with automatic login, where a keyring that wants unlocking after a
reboot means the machine comes up and is not reachable from anybody's phone
until somebody finds a keyboard. A headless Linux box may have no Secret
Service running at all. See the comment at the top of `src/jellyfin.rs`.

A portable copy on a memory stick has no permissions to set at all. A lost
stick is a leaked credential, and that is the trade a portable install makes.

**TinePlayer will talk to a Jellyfin server over plain HTTP.** That is what a
server on a home network answers to, and requiring HTTPS would mean most
people could not connect at all. Anybody who types `https://` gets it, and the
WebSocket follows the scheme rather than quietly downgrading. On a network
where you do not trust the other devices, use HTTPS.

**The log file contains file names and item names.** What is removed - the
token, the home directory and so the account name, the folder a video sits in,
and any server address outside the machine's own network - is enforced at the
write path rather than at each call site, so a line added later cannot forget.
File names are kept on purpose: they are often what a fault turns on. Nothing
is sent anywhere.

## What is out of scope

- Anything requiring an attacker to already be running code as your user
  account. At that point they can read the same files TinePlayer can
- Vulnerabilities in GStreamer, GTK or the codecs, which should go to those
  projects - though tell me too if TinePlayer is what exposes them
- A Jellyfin or Kodi server you have deliberately made reachable from the
  internet without a proxy in front of it

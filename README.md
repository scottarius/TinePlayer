<h1 align="center">
  <img src="data/ui/tineplayer.png" width="96" alt=""><br>
  TinePlayer
</h1>

<p align="center">
  Watch together, in different languages.
</p>

<p align="center">
  <a href="https://github.com/scottarius/TinePlayer/actions/workflows/ci.yml"><img src="https://github.com/scottarius/TinePlayer/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-informational" alt="Windows, Linux and macOS">
  <img src="https://img.shields.io/badge/built%20with-Rust%20%2B%20GStreamer-orange" alt="Rust and GStreamer">
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

Play one video with two audio tracks at the same time, each sent to a different
audio device, so people who speak different languages, or who need audio
description, can watch it together. The room hears one track through the
speakers; whoever is on headphones hears the other. Any two connected devices
will do.

That matters because people experience a film differently - in different
languages, described, subtitled, louder, quieter - and watching the way that
works for you means the performances land, the jokes time correctly, and you
don't have to work at it. Usually those differences mean watching apart, or one
person making a concession. This is an attempt to let everyone have the film
the way it works for them, at the same time, on the same screen, while still
sharing the experience together.

## Why another video player

Plenty of people have tried to rig this up themselves: the film on the
television, the same film started again on a laptop or a phone with headphones
in, and a count of three to start them together. It works for about a minute.
Then one drifts, someone nudges it back, it drifts again, and every pause for
the door or the kettle means lining both up by hand all over again. You end up
watching the sync instead of the film.

The other way around is worse. One person just goes without: reads subtitles
they would rather not need, or misses the description, or listens in a
language they have to work at. That is usually the option people settle on,
because it is the one that does not need managing.

Ordinary players cannot fix this, because choosing a track and choosing a
speaker are two separate settings rather than a pair, and one copy of a player
only ever plays one of them. This does it in one place: one film, playing
once, with both soundtracks kept together because they never came apart.

## How it works

TinePlayer is built on [GStreamer](https://gstreamer.freedesktop.org/). A video
file's audio tracks are already separate streams inside the container, so the
file is demuxed once and each chosen track is piped to its own output device
from a single pipeline, staying in sync because it is all one clock.

## Features

- Plays a video and two simultaneous audio tracks to separate output devices in
  sync
- Targets HTPC and TV use, with a large interface and full gamepad support
- Works with standard video containers: MKV, MP4, MPEG-TS (anything GStreamer
  supports)
- Resumes videos with remembered playback time and language/track selections
- Displays subtitles with support for both embedded and external files
- Selects tracks automatically from your preferred languages
- Splits any pair of tracks a file carries, including audio description
  alongside the ordinary soundtrack
- Integrates with Kodi and reports playback progress, including libraries from
  add-ons like Jellyfin and Plex
- Launches straight into playback from command-line arguments, for custom
  integrations

## Requirements

- A display and two or more connected audio output devices. Any combination
  works: speakers and headphones, a USB headset, an external DAC, and so on.
- A video file containing two or more audio tracks.

> [!NOTE]
> Bluetooth audio devices may add 100-200ms of latency and result in the audio
> being slightly out of sync.

## Documentation

- **[Building from source](docs/building.md)** - setup scripts and dependencies
  for each platform
- **[Using TinePlayer](docs/usage.md)** - controls, keyboard and gamepad,
  command-line options
- **[Configuration](docs/configuration.md)** - `config.yaml`, language
  preferences, saved playback resume data
- **[Integrations](docs/integrations.md)** - integrating with other media
  players and libraries

## Quick start

```sh
./setup-linux.sh            # Linux; .\setup-windows.ps1 on Windows, ./setup-mac.sh on macOS
cargo build --release
./target/release/TinePlayer
```

See [Building from source](docs/building.md) for what those scripts install,
and for the Windows caveat about installing GTK separately.

## Accessibility

The interface is built to be driven without a mouse or a screen: everything is
reachable by keyboard or gamepad, the type is large and scalable, and the selection mark is
meant to be read from across a room. Audio description is a first-class
feature rather than an afterthought, and can be sent to one output while the
room hears the ordinary soundtrack.

Subtitles are treated as an access feature rather than a translation one.
Embedded tracks and files sitting beside the video appear in a single list, so
an SDH or hard-of-hearing track is chosen the same way as any other, and a
preference can pick one automatically for every video. Size and font are both
configurable, sized against the video rather than the screen so they stay
legible on a television.

Screen readers are a work in progress. Some effort has gone into it and parts
of the interface are usable, but it is not where it should be, and it has not
been tested by anyone who relies on one. If that is you, [I would love to get
feedback on how it can be better](https://github.com/scottarius/TinePlayer/issues).

## Feedback

If TinePlayer doesn't let everyone in your room watch the way each of them
needs to, I want to hear about it: what gets in the way, whose experience it
falls short for, what's missing or simply wrong.
[Open an issue](https://github.com/scottarius/TinePlayer/issues) or
[start a discussion](https://github.com/scottarius/TinePlayer/discussions).
This only works if it suits everyone watching at once, and I genuinely want
your feedback in order to make it better.

## Compatibility

TinePlayer targets a deliberately conservative baseline of **GTK 4.6** and
**GStreamer 1.18**, so it builds on the distributions people actually run
rather than only current ones. GTK 4.x and GStreamer 1.x are both API- and
ABI-stable within their major version, so building against that baseline still
runs correctly on much newer releases.

| System | GTK 4 | Status |
| --- | --- | --- |
| Raspberry Pi OS / Debian 12 (Bookworm) | 4.8 | Tested |
| Windows 10 / 11 | 4.20 (bundled with GStreamer) | Tested |
| macOS 26 (Apple Silicon) | 4.22 (Homebrew) | Tested |
| Ubuntu 22.04 LTS | 4.6 | Meets baseline, untested |
| Ubuntu 24.04 LTS | 4.14 | Meets baseline, untested |
| Debian 13 (Trixie) | 4.18 | Meets baseline, untested |

Both Wayland and X11 sessions are supported; the backend is chosen at runtime.
"Meets baseline" means the system satisfies the version requirements and is
expected to work, but has not been run there. Reports welcome.

One consequence of that baseline: the GTK 4.14+ `dmabuf` zero-copy rendering
path is not used, so newer systems do not gain that particular optimization.
The OpenGL path used instead handles 1080p comfortably, including on a Pi 5.

## How this was built

Written collaboratively with an AI assistant (Claude). While Claude wrote the
bulk of the code, every architectural and design decision, and all testing and
verification, was done by hand.

## License

TinePlayer's own code is MIT. See [LICENSE](./LICENSE).

It builds on the following, none of which are vendored into this repository:

- **GStreamer** ([LGPL
  2.1](https://gstreamer.freedesktop.org/documentation/frequently-asked-questions/licensing.html))
  and **GTK 4** (LGPL 2.1) are runtime dependencies. A build from source uses
  the copies already installed on the machine; a packaged build ships them
  beside the executable, unmodified, with their license texts. They are loaded
  as separate shared libraries in both cases, so they can be replaced with
  your own build of the same version, which is what the LGPL asks for.
- **`gst-plugin-gtk4`** (MPL-2.0) provides the `gtk4paintablesink` element and
  *is* statically linked into the binary, because it is not packaged by Debian
  or shipped with the GStreamer Windows installer. Depending on it as an
  ordinary crate avoids making every user build and install a plugin by hand.
  MPL-2.0 is file-scoped copyleft, so it combines with MIT code without
  affecting this project's own licensing; the plugin's source remains under
  MPL-2.0 and is available
  [upstream](https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs).

Every Rust library compiled into TinePlayer, direct and transitive, is listed
with its license in **[THIRD-PARTY.md](./THIRD-PARTY.md)**. Nearly all are MIT
or Apache-2.0, and their notices travel with the application as those licenses
ask.

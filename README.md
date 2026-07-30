<h1 align="center">
  <img src="data/tineplayer.png" width="96" alt=""><br>
  TinePlayer
</h1>

<p align="center">
  Watch together, in different languages.
</p>

<p align="center">
  <a href="https://github.com/scottarius/TinePlayer/actions/workflows/ci.yml"><img src="https://github.com/scottarius/TinePlayer/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux-informational" alt="Windows and Linux">
  <img src="https://img.shields.io/badge/built%20with-Rust%20%2B%20GStreamer-orange" alt="Rust and GStreamer">
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

Play one video with two audio tracks at the same time, each sent to a different
audio device, so people who speak different languages can watch it together.
The room hears one language through the speakers; whoever is on headphones
hears the other. Any two connected audio devices will do.

That matters because there is something particular about hearing a film in your
own language. The performances land, the jokes time correctly, and you follow
the story without having to work at it. TinePlayer lets everyone watching have
that experience together on the same screen.

## Why another video player

Most video players decode exactly one audio track per playback session, and
none of them can route two different tracks to two different output devices at
the same time.

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
- Integrates with Kodi and reports playback progress, including libraries from
  add-ons like Jellyfin and Plex
- Launches straight into playback from command-line arguments, for custom
  integrations

## Requirements

- A display and two or more connected audio output devices. Any combination
  works: speakers and headphones, a USB headset, an external DAC, and so on.
- A video file containing two or more audio tracks.

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
./install.sh            # Linux, or .\install.ps1 on Windows
cargo build --release
./target/release/TinePlayer
```

See [Building from source](docs/building.md) for what those scripts install,
and for the Windows caveat about installing GTK separately.

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
  and **GTK 4** (LGPL 2.1) are runtime dependencies, installed separately and
  linked dynamically.
- **`gst-plugin-gtk4`** (MPL-2.0) provides the `gtk4paintablesink` element and
  *is* statically linked into the binary, because it is not packaged by Debian
  or shipped with the GStreamer Windows installer. Depending on it as an
  ordinary crate avoids making every user build and install a plugin by hand.
  MPL-2.0 is file-scoped copyleft, so it combines with MIT code without
  affecting this project's own licensing; the plugin's source remains under
  MPL-2.0 and is available
  [upstream](https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs).

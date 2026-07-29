# TinePlayer

Play one video with two audio tracks at the same time, each sent to a different
audio device, so people who speak different languages can watch it together.
The room hears one language through the speakers; whoever is on headphones
hears the other. Any two connected audio devices will do.

There is something particular about hearing a film in your own language. The
performances land, the jokes time correctly, and you follow the story without
having to work at it. TinePlayer lets everyone watching have that at the same
time, on the same screen.

## Why

Most video players decode exactly one audio track per playback session, and
none of them can route two different tracks to two different output devices at
the same time.

TinePlayer is built on [GStreamer](https://gstreamer.freedesktop.org/). A
video file's audio tracks are already separate streams inside the container, so
the file is demuxed once and each chosen track is piped to its own output
device from a single pipeline, staying in sync because it is all one clock.

## Features

- Plays two audio tracks at once, each to its own output device, in sync with
  the video
- Reads standard video containers: MKV, MP4, MPEG-TS, and anything else
  GStreamer supports
- Standard playback controls, shown over the video
- Remembers the playback position for each video and resumes there
- Interface large enough to use from the couch, with gamepad support
- Subtitles, either embedded in the video or from a subtitle file beside it,
  chosen separately from the two audio tracks
- Preferred languages per output and for subtitles, used to pick tracks
  automatically for a video you have not watched before
- A settings screen for theme, interface size, devices, languages and
  subtitle appearance
- Remembers your output devices
- Command-line arguments to launch straight into a video

## Known issues

- On Linux, seeking can leave one or both audio outputs silent for the rest of
  playback. Returning to the menu and playing again clears it. Windows is
  unaffected.
- Blu-ray PGS subtitles cannot be shown. No decoder for them ships with
  GStreamer, so they are left out of the subtitle list rather than offered and
  silently drawn as nothing. Subtitle files beside the video are unaffected.
- No packaged downloads yet, so it has to be built from source.

## Requirements

- A display, and two or more connected audio output devices. Any combination
  works: speakers and headphones, a USB headset, an external DAC, and so on.
- A video file containing two or more audio tracks.

## Building from source

Setup scripts are provided for both platforms. Each is idempotent, skipping
anything already present, so they are safe to re-run.

**Linux**

```sh
./install.sh
cargo build --release
```

Installs the Rust toolchain, GTK 4 development headers, and the GStreamer
runtime, development headers and plugins (including those for common
Blu-ray-rip codecs like AC3 and DTS).

**Windows**

```powershell
.\install.ps1
cargo build --release
```

Installs Rust, the Visual Studio 2022 C++ build tools and GStreamer, then sets
the environment variables needed to build. Open a new terminal afterwards so
those take effect.

GStreamer's Windows distribution bundles GTK 4 and glib alongside GStreamer, so
it supplies every native dependency in one place. **Don't install GTK
separately** (with gvsbuild, for instance): a second GTK brings a second copy
of glib, and mixing one library's headers with the other's build tools fails to
build. Everything must be an MSVC build to match Rust's MSVC toolchain, because
MSYS2/MinGW builds use a different ABI and will not link.

## Using it

Run it with no arguments, or by double-clicking the executable:

```
./target/release/TinePlayer
```

Everything is chosen from one menu: the video, which output devices to use, and
which audio track goes to each. Output devices are remembered; the video and
track choices are per-session.

Settings live in the per-user config directory (`~/.config/tineplayer/` on
Linux, `%LOCALAPPDATA%\tineplayer\` on Windows), so they apply however the
application is launched.

### Controls

| Key | Gamepad | Action                                |
| --- | --- |---------------------------------------|
| Arrow keys | D-pad or left stick | Navigate the menus                    |
| <kbd>Enter</kbd> | A / Cross | Select                                |
| <kbd>Esc</kbd> | B / Circle | Back one menu; stop video playback    |
| <kbd>Space</kbd> | A / Cross, or Start | Pause / resume playback               |
| <kbd>←</kbd> <kbd>→</kbd> | D-pad or left stick | Tap to skip 10 seconds; hold to scrub |
| <kbd>F</kbd> | Y / Triangle | Toggle fullscreen                     |

Controllers are picked up whenever they are connected, including part-way
through a session, and no configuration is needed.

### Command line

Any of the menu choices can be given up front, which skips straight to
playback:

```sh
TinePlayer --list-tracks video.mkv          # show track numbers
TinePlayer video.mkv --primary 5 --secondary 1
TinePlayer video.mkv --fullscreen --restart
```

Track numbers match what `--list-tracks` prints; `0` means no audio on that
output.

### Appearance

The interface is sized to be read from across a room, and scales itself to the
display it opens on: a 4K screen gets twice the size a 1080p one does, so the
menu stays the same size to the eye rather than shrinking as resolution grows.
Displays a compositor is already scaling are left alone, since the scaling has
happened once already.

Set `ui_scale` in the config file to pin the size instead, and the automatic
sizing stops. Navigation sounds can be turned off with `sounds: false`.

`theme` chooses `auto`, `light` or `dark`. Auto follows the desktop, and uses
dark when the desktop has no preference or cannot be asked, which is common on
the minimal desktops a media machine tends to run.

All of this is also reachable from the gear at the bottom of the menu, along
with preferred languages, subtitle size and font, and a way to forget every
saved playback position. `subtitle_size` is in points against the video's own
resolution rather than the screen's, so one value holds on any display.

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
path is not used, so newer systems do not gain that particular optimisation.
The OpenGL path used instead handles 1080p comfortably, including on a Pi 5.

## How this was built

Written collaboratively with an AI assistant (Claude). Every design
decision, and all testing and verification, was done by hand: much of what
this application does can only be checked by watching and listening to it.

## License

TinePlayer's own code is MIT. See [LICENSE](./LICENSE).

It builds on the following, none of which are vendored into this repository:

- **GStreamer** ([LGPL 2.1](https://gstreamer.freedesktop.org/documentation/frequently-asked-questions/licensing.html))
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

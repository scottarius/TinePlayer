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
- A built-in file browser for choosing videos with a controller, or drop a
  file onto the window
- Remembers your output devices
- Command-line arguments to launch straight into a video

## Known issues

- On Linux, seeking can silence an audio output for the rest of playback.
  Returning to the menu and playing again clears it.
- Blu-ray PGS subtitles cannot be shown: GStreamer ships no decoder for them,
  so they are left out of the list rather than drawn as nothing.
- On Windows, switching from dark back to light needs a restart, which the
  application offers to do. GTK there changes to dark but not back.
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

Everything is chosen from one menu: the video, the output devices, the audio
track for each, and the subtitles. Choosing a video opens a built-in browser;
a button in its top corner opens the system dialog instead.

### Controls

Everything is reachable with a keyboard or a gamepad. Nothing needs a mouse.

| Key | Gamepad | Action |
| --- | --- | --- |
| Arrow keys | D-pad or left stick | Navigate the menus |
| <kbd>Page Up</kbd> <kbd>Page Down</kbd> | Shoulder buttons | Jump a screenful, for long folders |
| <kbd>Enter</kbd> | A / Cross | Select |
| <kbd>Esc</kbd> | B / Circle | Back one menu; stop video playback |
| <kbd>Space</kbd> | A / Cross, or Start | Pause / resume playback |
| <kbd>←</kbd> <kbd>→</kbd> | D-pad or left stick | Tap to skip 10 seconds; hold to scrub |
| <kbd>F</kbd> | Y / Triangle | Toggle fullscreen |

### Command line

Any of the menu choices can be given up front, which skips straight to
playback. Track numbers are those `--list-tracks` prints.

| Option            | Meaning                                                        |
|-------------------|----------------------------------------------------------------|
| `FILE`            | Path to the video to play. Omit it to choose one in the window |
| `--primary <N>`   | Audio track for the primary output. `0` for no audio there     |
| `--secondary <N>` | Audio track for the secondary output. `0` for no audio there   |
| `--list-tracks`   | Print the file's audio tracks with their numbers, then exit    |
| `--restart`       | Start video from the beginning, ignoring any saved position    |
| `--fullscreen`    | Start fullscreen                                               |
| `-V`, `--version` | Print the version                                              |
| `-h`, `--help`    | Print help                                                     |

```sh
TinePlayer --list-tracks video.mkv
TinePlayer video.mkv --primary 5 --secondary 1 --fullscreen
```

### Configuration

Everything in the settings menu is stored in `config.yaml`, which can also be
edited directly. It lives in the per-user config directory
(`~/.config/tineplayer/` on Linux, `%LOCALAPPDATA%\tineplayer\` on Windows).

| Setting                       | Key                  | Default     | Meaning                                                                                            |
|-------------------------------|----------------------|-------------|----------------------------------------------------------------------------------------------------|
| Theme                         | `theme`              | `auto`      | `auto`, `light` or `dark`                                                                          |
| Interface Size                | `ui_scale`           | Unset       | Interface scale, such as `1.5` <br/>Unset scales automatically to the display resolution           |
| Navigation Sounds             | `sounds`             | `true`      | Navigation clicks, `true` or `false`                                                               |
| Primary Audio Device          | `primary_sink`       | Unset       | Primary output device name. Required                                                               |
| Primary Language Preference   | `primary_language`   | Unset       | Preferred primary language, such as `en` <br/>Unset defaults to the first track, see list below    |
| Secondary Audio Device        | `secondary_sink`     | Unset       | Second output device name <br/>`null` to play through primary only                                 |
| Secondary Language Preference | `secondary_language` | Unset       | Preferred secondary language, such as `en` <br/>Unset defaults to the second track, see list below |
| Subtitle Language             | `subtitle_language`  | Unset       | Preferred subtitle language, such as `en` <br/>Unset shows no subtitles, see list below            |
| Subtitle Size                 | `subtitle_size`      | `12`        | Point size against the video's resolution, not the screen's                                        |
| Subtitle Font                 | `subtitle_font`      | `Sans Bold` | Font Family and style name                                                                         |

Languages are `en`, `ru`, `es`, `fr`, `de`, `it`, `pt`, `nl`, `pl`, `uk`,
`cs`, `sv`, `no`, `da`, `fi`, `hu`, `tr`, `el`, `he`, `ar`, `hi`, `ja`, `ko`
and `zh`.

Only the leading letters are compared, so `en` matches a track tagged `eng` or
`en-US`, and a subtitle file named `film.en.hi.srt`.

Each video's position, track and subtitle choices are kept separately, in
`positions.json`.

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

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

- Built primarily for use on an HTPC and TV, but works just as well on a computer
- Plays a video and two audio tracks at once, each to its own output device, all in sync
- Works with standard video containers: MKV, MP4, MPEG-TS, and anything else
  GStreamer supports
- Remembers the playback position, output devices, track selection, and subtitle choice for each video when resuming
- Interface large enough to use from the couch, with full gamepad support
- Supports subtitles, either embedded in the video or from an external subtitle file beside it
- Preferred language settings per audio track and for subtitles, used to pick tracks
  automatically from what's available
- A built-in gamepad enabled file browser for choosing videos
- Command-line arguments to bypass the UI and launch straight into video playback

## Known issues

- Blu-ray PGS subtitles cannot be shown: GStreamer ships no decoder for them,
  so they are left out of the list rather than drawn as nothing.
- No packaged downloads yet, so it has to be built from source.

## Requirements

- A display and two or more connected audio output devices. Any combination
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

Run it from the command line with or without arguments, or by double-clicking the executable:

```
./target/release/TinePlayer
```

Everything required for video playback is chosen from the main screen: the video, the output devices, the audio
track for each, and the subtitles. The settings menu contains more advanced settings and preferences. See Configuration section below.

### Controls

Everything is reachable with a keyboard or a gamepad. Nothing needs a mouse, though everything is also mouse interactable.

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

Any of the menu choices can be given up front, and will skip straight to playback if required options are provided.

Track numbers are those `--list-tracks` prints.

| Option            | Meaning                                                                   |
|-------------------|---------------------------------------------------------------------------|
| `FILE`            | Path to the video to play. Omit it to choose one in the window            |
| `--primary <N>`   | Audio track for the primary output. `0` for no audio there                |
| `--secondary <N>` | Audio track for the secondary output. `0` for no audio there              |
| `--subtitle <N>`  | Subtitles to show. `0` for none                                           |
| `--list-tracks`   | Print the file's audio tracks and subtitles with their numbers, then exit |
| `--restart`       | Start video from the beginning, ignoring any saved position               |
| `--fullscreen`    | Start fullscreen                                                          |
| `--windowed`      | Start windowed, overriding a remembered fullscreen preference             |
| `-V`, `--version` | Print the version                                                         |
| `-h`, `--help`    | Print help                                                                |

```sh
TinePlayer --list-tracks video.mkv
TinePlayer video.mkv --primary 5 --secondary 1 --fullscreen
```

### Configuration

Everything in the settings menu is stored in `config.yaml`, which can also be
edited directly. It lives in the per-user config directory
(`~/.config/tineplayer/` on Linux, `%LOCALAPPDATA%\tineplayer\` on Windows).

| Setting                       | Key                  | Default     | Meaning                                                        |
|-------------------------------|----------------------|-------------|----------------------------------------------------------------|
| Theme                         | `theme`              | `auto`      | `auto`, `light` or `dark`                                      |
| Interface Size                | `ui_scale`           | Unset       | Interface scale, such as `1.5` <br/>If unset scales automatically to the display resolution |
| Navigation Sounds             | `sounds`             | `true`      | Navigation clicks, `true` or `false`                           |
| Primary Audio Device          | `primary_sink`       | Unset       | Primary output device name. Required                           |
| Primary Language Preference   | `primary_language`   | Unset       | Preferred primary language, see list below <br/>If unset defaults to the first track |
| Secondary Audio Device        | `secondary_sink`     | Unset       | Second output device name <br/>`null` to play through primary only |
| Secondary Language Preference | `secondary_language` | Unset       | Preferred secondary language, see list below <br/>If unset defaults to the second track |
| Subtitle Language             | `subtitle_language`  | Unset       | Preferred subtitle language, see list below <br/>If unset shows no subtitles |
| Subtitle Size                 | `subtitle_size`      | `12`        | Point size against the video's resolution, not the screen's    |
| Subtitle Font                 | `subtitle_font`      | `Sans Bold` | Font Family and style name                                     |

Supported languages: `en`, `ru`, `es`, `fr`, `de`, `it`, `pt`, `nl`, `pl`, `uk`,
`cs`, `sv`, `no`, `da`, `fi`, `hu`, `tr`, `el`, `he`, `ar`, `hi`, `ja`, `ko`,
 `zh`

Only the leading letters are compared, so `en` matches a track tagged `eng` or
`en-US`, and a subtitle file named `film.en.hi.srt`.

Each video's position, track and subtitle choices are kept separately, in
`positions.json`.

### Kodi

[Kodi](https://kodi.tv) is a media center application: it catalogues your films
and TV files with artwork and plays them on a television. It can hand playback to
TinePlayer rather than playing video itself so that you get the benefits of Kodi's
library browser plus the benefits of TinePlayer's dual audio output.

There are two ways to set it up depending on your preference:

* **As a choice per video:** By default Kodi will play videos itself, and 
TinePlayer becomes an extra option under **Play using...** in a video's context menu.
* **As the default player:** Kodi opens every video in TinePlayer.

Caveat about Kodi versions: 
* On Kodi 21 and later, the **Play using...** option appears throughout.
* On Kodi 20 and earlier, the **Play using...** only appears under the **Videos → Files** section, not in the Libraries.

**Manual Installation**

Kodi has no interface for this, so it means editing `playercorefactory.xml` in Kodi's userdata directory — `~/.kodi/userdata/` on
Linux, `%APPDATA%\Kodi\userdata\` on Windows. If it doesn't exist already, you can create it yourself. 

Paste in the following snippit, replacing `<filename>` with the TinePlayer executable path.

```xml
<playercorefactory>
  <players>
    <player name="TinePlayer" type="ExternalPlayer" audio="false" video="true">
      <filename>/path/to/TinePlayer</filename>
      <args>"{1}" --fullscreen</args>
      <hidexbmc>true</hidexbmc>
      <hideconsole>true</hideconsole>
    </player>
  </players>
  <!-- Rules here -->
</playercorefactory>
```

This will add an option to the **Play Using...** menu.<br/> 
To force TinePlayer to act as the default player, insert the following under `<!-- Rules here -->`:

```xml
  <rules action="prepend">
    <rule video="true" player="TinePlayer" />
  </rules>
```

On Windows, you must point `<filename>` at `launch-tineplayer.cmd` rather than straight at the
executable. It will start the player from the right working directory so it can launch without conflicts.

**Automated Installation**

If you don't want to do the above manual installation, the following scripts will do it for you:

```sh
./install-kodi.sh             # Linux
./install-kodi.sh --default   # Linux, as default player

.\install-kodi.ps1            # Windows
.\install-kodi.ps1 -Default   # Windows, as default player
```

They find Kodi's userdata directory themselves and write the correct configuration file.
Any existing `playercorefactory.xml` is backed up rather than replaced.

Restart Kodi after either installation method.

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

Written collaboratively with an AI assistant (Claude). While Claude wrote
the bulk of the code, every architectural and design decision, and all 
testing and verification, was done by hand.

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

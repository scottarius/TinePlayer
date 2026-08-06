# Using TinePlayer

[Install TinePlayer](../README.md#install) by downloading the latest version from the
[releases page](https://github.com/scottarius/TinePlayer/releases). Start it
from your applications menu, or from a terminal with or without arguments.

## Starting TinePlayer

### Windows

Start TinePlayer from the Start menu, or from a terminal with arguments. Installed, it is on
your `PATH`; from the portable ZIP, run it from wherever you unpacked it:

```powershell
TinePlayer.exe
TinePlayer.exe "C:\Videos\film.mkv" --fullscreen
```

### macOS

Start TinePlayer from Applications, or from the terminal with arguments:

```sh
/Applications/TinePlayer.app/Contents/MacOS/tineplayer ~/Movies/film.mkv --fullscreen
```

### Linux

Start TinePlayer from your applications menu, or from a terminal. It installs
onto your `PATH` as `tineplayer`:

```sh
tineplayer
tineplayer ~/Videos/film.mkv --fullscreen
```

### Built from source

The binary is at `./target/release/tineplayer` on every platform, and takes the
same arguments as above. See [Building from source](building.md).

On Windows, a build from source may need starting through
`launch-tineplayer-windows.cmd` when something else launches it. Windows looks
for libraries in the working directory before anywhere else, so a program that
starts TinePlayer from its own folder, and happens to ship its own copies of
libraries GStreamer also uses, can have those found first. TinePlayer then
fails to start, usually without saying why. The script sets the working
directory before handing over:

```powershell
.\launch-tineplayer-windows.cmd "H:\Videos\film.mkv" --fullscreen
```

An installed build never needs this, because its libraries sit beside the
executable and are found first.

---

Everything required for video playback is chosen from the main screen: the
video, the output devices, the audio track for each, and the subtitles. The
settings menu contains more advanced settings and preferences. See
[Configuration](configuration.md).

## Video sources

A video can be given as a path or as a URL.

| Given                   | Example                                                     | Notes                                                                                        |
|-------------------------|-------------------------------------------------------------|----------------------------------------------------------------------------------------------|
| An absolute path        | `C:\Videos\film.mkv` <br/>`~/Videos/film.mkv`                                 | An absolute path on a local or mapped drive                                                  |
| A network share         | `\\server\media\film.mkv` <br/>`/mnt/server/media/film.mkv` | Anything the machine already has access to: a UNC path on windows or a mounted path on Linux |
| `http://` or `https://` | `http://server/media/film.mkv`                              | A direct link to the file, including one from a media server                                 |
| `smb://`                | `smb://server/media/film.mkv`                               | Linux only, and only where the share is already reachable                                    |

A network share opened by path is an ordinary file as far as TinePlayer is concerned,
which makes it the most dependable option: the machine has already handled
reaching the share and any credentials it needed.

Note that [external subtitle files](#subtitles) are only found for videos opened
by direct path, not `http://` or `smb://`

## Controls

Everything is reachable with a keyboard or a gamepad. A mouse is not required, but can be used.

### In the menus

| Key | Gamepad | Action                                |
| --- | --- |---------------------------------------|
| Arrow keys | <kbd>D-pad</kbd> or <kbd>Left stick</kbd> | Navigate the UI                       |
| <kbd>Page Up</kbd> <kbd>Page Down</kbd> | <kbd>LT</kbd> <kbd>RT</kbd> / <kbd>L2</kbd> <kbd>R2</kbd> | Page up or down a scrollable list     |
| <kbd>Tab</kbd> <kbd>Shift</kbd>+<kbd>Tab</kbd> | <kbd>RB</kbd> <kbd>LB</kbd> / <kbd>R1</kbd> <kbd>L1</kbd> | Move to next / previous UI element    |
| <kbd>Enter</kbd> | <kbd>A</kbd> / <kbd>Cross</kbd> | Select                                |
| <kbd>Esc</kbd> | <kbd>B</kbd> / <kbd>Circle</kbd> | Cancel / Back                         |
| <kbd>Ctrl</kbd>+<kbd>O</kbd> | - | Open the file browser, from main menu |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> | - | Open a video by URL, from main menu   |

### During playback

| Key | Gamepad                                                    | Action                                                                                            |
| --- |------------------------------------------------------------|---------------------------------------------------------------------------------------------------|
| <kbd>Space</kbd> | <kbd>A</kbd> / <kbd>Cross</kbd>, or <kbd>Start</kbd>       | Pause or resume                                                                                   |
| <kbd>Esc</kbd> | <kbd>B</kbd> / <kbd>Circle</kbd>                           | Stop and return to the menu. (Gamepad will hide the playback controls first)                      |
| <kbd>←</kbd> <kbd>→</kbd> | Left or Right on <kbd>D-pad</kbd> or <kbd>Left stick</kbd> | Tap to skip 10 seconds, hold to scrub. Moves between the playback controls while they are showing |
| <kbd>↑</kbd> | Up on <kbd>D-pad</kbd> or <kbd>Left stick</kbd>            | Show the playback controls, then move up through them                                             |
| <kbd>↓</kbd> | Down on <kbd>D-pad</kbd> or <kbd>Left stick</kbd>          | Move down through the playback controls, then hide them                                           |
| <kbd>F</kbd> | <kbd>Y</kbd> / <kbd>Triangle</kbd>                         | Toggle fullscreen                                                                                 |
| <kbd>C</kbd> | <kbd>X</kbd> / <kbd>Square</kbd>                           | Show or hide subtitles                                                                            |
| <kbd>M</kbd> | Hold <kbd>X</kbd> / <kbd>Square</kbd>                      | Mute or restore both outputs at once. Or long-press the volume button                             |
| <kbd>T</kbd> | Click <kbd>Right stick</kbd>                               | Swap the right-hand readout between the video length and the time remaining                       |


The playback controls hide themselves after a few seconds of stillness, and
come back on any interaction.

![The playback control bar: the position along the top, then settings, stop,
skip back, pause, skip forward, subtitles, volume and
fullscreen.](screenshots/control-bar.png)

### Volume and sync

The volume button opens a panel above the controls, holding audio settings
for each output. When open, the controls will not auto-hide until closed.

![The volume panel, with a volume and a sync bar under each output
device.](screenshots/volume-panel.png)

**Volume** levels can be set per output. The speaker icons next to the
volume sliders will mute each separately. <kbd>M</kbd>, or holding the main
volume button, mutes and restores both outputs at once.

**Sync** adjusts the audio delay so it lines up with the picture. Bluetooth
headphones and other speakers can add a delay of their own and feel out of sync.
It can be adjusted and fine-tuned here while the video is playing, per output.
A positive value holds that output back, for sound arriving ahead of the picture;
a negative one moves it earlier. The sync icon puts that output back to `0ms`.

Both are saved and persist between runs. See
[Audio Sync](configuration.md#audio-sync).

## Subtitles

Subtitles come from two places: tracks embedded in the video, and subtitle
files sitting beside it. Both appear in one list on the main screen, chosen
separately from the audio so subtitles can be a third language, or the same as
one of the two being heard.

External files are matched by name. Anything that starts with the video's name
and ends in `.srt`, `.ass`, `.ssa` or `.vtt` is offered, and whatever sits
between the two becomes its label:

| File beside `Film (2019).mkv` | Listed as  |
|-------------------------------|------------|
| `Film (2019).en.srt`          | `en`       |
| `Film (2019).en.hi.srt`       | `en.hi`    |
| `Film (2019).srt`             | `External` |

The [Subtitle Preference](configuration.md#choosing-subtitles-automatically)
will attempt to automatically choose subtitles and defaults to Forced subtitles
matching the primary output language, but any manual choices are saved.
To override either for a single playback, see [Choosing subtitles on the
command line](#choosing-subtitles-on-the-command-line).

> [!NOTE]
> * Blu-ray PGS subtitles are not offered. They are images rather than text
>   and GStreamer ships no decoder for them, so a track that could only draw
>   nothing is left out of the list entirely.
> * External subtitle files are only found for videos opened by path: local, a
>   UNC path, or a mounted share. A video opened by URL, such as `http://` or
>   `smb://`, offers only its embedded subtitles.

## Command line

Any of the menu choices can be given up front, and will skip straight to
playback if required options are provided.

Track numbers are those `--list-tracks` prints.

| Option            | Meaning                                                                                                     |
|-------------------|-------------------------------------------------------------------------------------------------------------|
| `FILE`            | The video to play, see [Video sources](#video-sources)                                                      |
| `--primary <T>`   | Audio for the primary output, see [Choosing Audio on the Command Line](#choosing-audio-on-the-command-line) |
| `--secondary <T>` | Audio for the secondary output, same as above                                                               |
| `--subtitle <S>`  | Subtitles to show, see [Choosing Subtitles on the Command Line](#choosing-subtitles-on-the-command-line)    |
| `--list-devices`  | Print this machine's audio output device names, as [`config.yaml`](configuration.md) wants them             |
| `--play`          | Skip the menu and start playback immediately. See [Playing a Video Directly](#playing-a-video-directly)     |
| `--list-tracks`   | Print the file's audio tracks and subtitles with their numbers                                              |
| `--restart`       | Start video from the beginning, ignoring any saved position                                                 |
| `--forget`        | Forget the saved positions and track choices. Pass a FILE to limit to a single video                        |
| `--fullscreen`    | Start fullscreen. With `--external` it's fixed, see [Fixed Fullscreen](#fixed-fullscreen)                   |
| `--windowed`      | Start windowed, overriding a remembered fullscreen preference                                               |
| `--external`      | Used for launching from another application, see [Integrations](integrations.md)                            |
| `--kodi`          | Used by [Kodi Integration](integrations.md#kodi), Implies `--external`                                      |
| `-V`, `--version` | Print the version                                                                                           |
| `-h`, `--help`    | Print help                                                                                                  |

```sh
TinePlayer --list-tracks video.mkv
TinePlayer video.mkv --primary 5 --secondary 1 --fullscreen
```

### Playing a Video Directly

Normally TinePlayer opens to the playback options menu, with track
and subtitle choices. Passing `--play` skips the menu and starts playback immediately.

```sh
tineplayer film.mkv --play
```

By default it uses whatever settings was saved for that video, or your
language preferences if it has not been played before.

Additionally, you can pass any combination of arguments to override the various settings.

```sh
# Straight to playback with specific audio tracks
tineplayer film.mkv --play --primary 5 --secondary 1

# The menu, with those tracks pre-chosen
tineplayer film.mkv --primary 5 --secondary 1
```

The only argument required with `--play` is a valid video path, and the primary output device setup.

### Fixed Fullscreen

`--fullscreen` together with `--external` (or `--kodi`, which implies it) starts
in fullscreen mode and disables toggling. Most integrations that ask for fullscreen
are providing a fullscreen experience themselves and breaking out of that is a bad
experience.

### Choosing Audio on the Command Line

`--primary` and `--secondary` each take any of these:

| Given     | Means                                                                  |
|-----------|------------------------------------------------------------------------|
| `3`       | the third entry `--list-tracks` prints                                 |
| `en`      | the first track in that [language](configuration.md#languages)         |
| `ad`      | the first [described](configuration.md#audio-description) track        |
| `en:ad`   | the first described track in that language                             |
| `0`       | no audio on this output                                                |

A language on its own never selects a described track, matching what the
setting does: description is only ever played by asking for it.

### Choosing Subtitles on the Command Line

Override the subtitle preference by passing in the `--subtitle` argument can override this behavior with the below
options.

| Given                              | Means                                                                                            |
|------------------------------------|--------------------------------------------------------------------------------------------------|
| `primary_forced`, `primary`, `secondary_forced`, `secondary` | the same as the [Subtitle Preference](configuration.md#choosing-subtitles-automatically) setting |
| `3`                                | the third entry `--list-tracks` prints                                                           |
| `Film (2019).ru.hi.srt`            | that file, beside the video                                                                      |
| `en.hi`                            | the entry with that label                                                                        |
| `ru`                               | the first subtitle matching the [language code](configuration.md#languages)                      |
| `0` or `none`                      | no subtitles                                                                                     |

> [!NOTE]
> Numbers from `--list-tracks` can change. Adding or removing a subtitle file
> beside the video renumbers everything after it.
>
> A subtitle file has to sit beside the video. Any path given is ignored and
> only the file name is used.

The font size and style can be configured in the settings, see
[Configuration](configuration.md).

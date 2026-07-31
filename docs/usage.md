# Using TinePlayer

Run it from the command line with or without arguments, or by double-clicking
the executable.

### Linux

```sh
./target/release/TinePlayer
./target/release/TinePlayer ~/Videos/film.mkv --fullscreen
```

### Windows

```powershell
.\target\release\TinePlayer.exe
.\target\release\TinePlayer.exe "H:\Videos\film.mkv" --fullscreen
```

#### Launch script
Useful for launching from another app or integration that has a different
working directory if library conflicts cause errors.
```powershell
.\launch-tineplayer-windows.cmd "H:\Videos\film.mkv" --fullscreen
```

Everything required for video playback is chosen from the main screen: the
video, the output devices, the audio track for each, and the subtitles. The
settings menu contains more advanced settings and preferences. See
[Configuration](configuration.md).

## Video sources

A video can be given as a path or as a URL.

| Given                       | Example                        | Notes                                              |
|-----------------------------|--------------------------------|----------------------------------------------------|
| A file on this machine      | `~/Videos/film.mkv`            |                                                    |
| A network share, by path    | `\\server\media\film.mkv`      | Whatever this machine can already open: a UNC path or mapped drive on Windows, a mounted path on Linux |
| `http://` or `https://`     | `http://server/media/film.mkv` | A direct link to the file, including one from a media server |
| `smb://`                    | `smb://server/media/film.mkv`  | Linux only, and only where the share is already reachable. Windows should use the UNC path instead |

A share opened by path is an ordinary file as far as TinePlayer is concerned,
which makes it the most dependable option: the machine has already handled
reaching the share and any credentials it needed.

Note that [external subtitle files](#subtitles) are only found for videos opened
by path.

## Controls

Everything is reachable with a keyboard or a gamepad. Nothing needs a mouse,
though everything is also mouse interactable.

> [!NOTE]
> Screen reader support is minimal: the menus are named, the playback controls
> are not reachable at all. See [Accessibility](../README.md#accessibility).

| Key | Gamepad | Action                                                                                  |
| --- | --- |-----------------------------------------------------------------------------------------|
| Arrow keys | D-pad or left stick | Navigate the menus                                                                      |
| <kbd>Page Up</kbd> <kbd>Page Down</kbd> | Shoulder buttons | Jump a screenful, for long folders                                                      |
| <kbd>Enter</kbd> | A / Cross | Select                                                                                  |
| <kbd>Esc</kbd> | B / Circle | Back one menu; stop video playback                                                      |
| <kbd>Space</kbd> | A / Cross, or Start | Pause / resume playback                                                                 |
| <kbd>←</kbd> <kbd>→</kbd> | D-pad or left stick | In Video: Tap to skip 10 seconds; hold to scrub, navigate playback controls if visible. |
| <kbd>↑</kbd> | D-pad or left stick | In Video: Show or navigate the playback controls                                        |
| <kbd>↓</kbd> | D-pad or left stick | In Video: Hide or navigate the playback controls                                        |
| <kbd>F</kbd> | Y / Triangle | Toggle fullscreen                                                                       |
| <kbd>C</kbd> | X / Square | Show or hide subtitles, during playback                                                 |
| <kbd>M</kbd> | Hold X / Square | Mute or restore both outputs at once                                                    |
| <kbd>T</kbd> | Right stick click | Swap the right-hand readout between the length and the time left                        |
| Hold <kbd>Enter</kbd> | Hold A / Cross | On the volume button: Mute or restore both outputs at once.                             |
| <kbd>Ctrl</kbd>+<kbd>O</kbd> | - | Open the file browser                                                                   |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> | - | Open a video by URL                                                                     |

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

Your choice is remembered per video. For one you have not played before, the
[Subtitle Preference](configuration.md#choosing-subtitles-automatically)
setting chooses. By default that is forced subtitles in the language the
primary output is playing.

> [!NOTE]
> * Blu-ray PGS subtitles will not be shown as GStreamer ships no decoder for
>   them natively.
> * External subtitle files are only found for videos opened by path: local, a
>   UNC path, or a mounted share. A video opened by URL, such as `http://` or
>   `smb://`, offers only its embedded subtitles.

### Choosing audio on the command line

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

### Choosing subtitles on the command line

By default the [Subtitle
Preference](configuration.md#choosing-subtitles-automatically) setting will
auto-select subtitles or a previous saved choice will be used.

Passing in the `--subtitle` argument can override this behavior with the below
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

## Command line

Any of the menu choices can be given up front, and will skip straight to
playback if required options are provided.

Track numbers are those `--list-tracks` prints.

| Option            | Meaning                                                                                                  |
|-------------------|----------------------------------------------------------------------------------------------------------|
| `FILE`            | The video to play, see [Video sources](#video-sources)                                                   |
| `--primary <T>`   | Audio for the primary output, see [Choosing audio on the command line](#choosing-audio-on-the-command-line) |
| `--secondary <T>` | Audio for the secondary output, same as above                                                            |
| `--subtitle <S>`  | Subtitles to show, see [Choosing subtitles on the command line](#choosing-subtitles-on-the-command-line) |
| `--list-tracks`   | Print the file's audio tracks and subtitles with their numbers                                           |
| `--restart`       | Start video from the beginning, ignoring any saved position                                              |
| `--forget`        | Forget the saved positions and track choices. Pass a FILE to limit to a single video                     |
| `--fullscreen`    | Start fullscreen. With `--external` it's fixed, see [below](#fixed-fullscreen)                           |
| `--windowed`      | Start windowed, overriding a remembered fullscreen preference                                            |
| `--external`      | Used for launching from another application, see [Integrations](integrations.md)                         |
| `--kodi`          | Used by [Kodi Integration](integrations.md#kodi), Implies `--external`                                   |
| `-V`, `--version` | Print the version                                                                                        |
| `-h`, `--help`    | Print help                                                                                               |

```sh
TinePlayer --list-tracks video.mkv
TinePlayer video.mkv --primary 5 --secondary 1 --fullscreen
```

### Fixed fullscreen

`--fullscreen` together with `--external` (or `--kodi`, which implies it) starts
in fullscreen mode and disables toggling. Most integrations that ask for fullscreen
are providing a fullscreen experience themselves and breaking out of that is a bad
experience.


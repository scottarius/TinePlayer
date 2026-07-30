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
.\launch-tineplayer.cmd "H:\Videos\film.mkv" --fullscreen
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

| Key | Gamepad | Action |
| --- | --- | --- |
| Arrow keys | D-pad or left stick | Navigate the menus |
| <kbd>Page Up</kbd> <kbd>Page Down</kbd> | Shoulder buttons | Jump a screenful, for long folders |
| <kbd>Enter</kbd> | A / Cross | Select |
| <kbd>Esc</kbd> | B / Circle | Back one menu; stop video playback |
| <kbd>Space</kbd> | A / Cross, or Start | Pause / resume playback |
| <kbd>←</kbd> <kbd>→</kbd> | D-pad or left stick | Tap to skip 10 seconds; hold to scrub |
| <kbd>F</kbd> | Y / Triangle | Toggle fullscreen |
| <kbd>C</kbd> | X / Square | Show or hide subtitles, during playback |
| <kbd>Ctrl</kbd>+<kbd>O</kbd> | - | Open the file browser |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> | - | Open a video by URL |

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

| Option            | Meaning                                                                                                                                  |
|-------------------|------------------------------------------------------------------------------------------------------------------------------------------|
| `FILE`            | The video to play, see [Video sources](#video-sources)                                                                                   |
| `--primary <N>`   | Audio track for the primary output. `0` for no audio there                                                                               |
| `--secondary <N>` | Audio track for the secondary output. `0` for no audio there                                                                             |
| `--subtitle <S>`  | Subtitles to show, see [Choosing subtitles on the command line](#choosing-subtitles-on-the-command-line)                                 |
| `--list-tracks`   | Print the file's audio tracks and subtitles with their numbers, then exit                                                                |
| `--restart`       | Start video from the beginning, ignoring any saved position                                                                              |
| `--fullscreen`    | Start fullscreen                                                                                                                         |
| `--windowed`      | Start windowed, overriding a remembered fullscreen preference                                                                            |
| `--external`      | Used for launching from another application, see [Integrations](integrations.md)                                                         |
| `--kodi`          | Launched by Kodi: Sync the resume position with its library. Implies `--external`
| `-V`, `--version` | Print the version                                                                                                                        |
| `-h`, `--help`    | Print help                                                                                                                               |

```sh
TinePlayer --list-tracks video.mkv
TinePlayer video.mkv --primary 5 --secondary 1 --fullscreen
```

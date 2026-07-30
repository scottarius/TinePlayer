# Using TinePlayer

Run it from the command line with or without arguments, or by double-clicking the executable.

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
Useful for launching from another app or integration that has a different working directory if library conflicts cause errors. 
```powershell
.\launch-tineplayer.cmd "H:\Videos\film.mkv" --fullscreen
```

Everything required for video playback is chosen from the main screen: the video, the output devices, the audio
track for each, and the subtitles. The settings menu contains more advanced settings and preferences. See [Configuration](configuration.md).

## Controls

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

## Subtitles

Subtitles come from two places: tracks inside the video, and subtitle files
sitting beside it. Both appear in one list on the main screen, chosen
separately from the audio - so subtitles can be a third language, or the
same as one of the two being heard.

External files are matched by name. Anything that starts with the video's name
and ends in `.srt`, `.ass`, `.ssa` or `.vtt` is offered, and whatever sits
between the two becomes its label:

| File beside `Film (2019).mkv` | Listed as  |
|-------------------------------|------------|
| `Film (2019).en.srt`          | `en`       |
| `Film (2019).en.hi.srt`       | `en.hi`    |
| `Film (2019).srt`             | `External` |

Your choice is remembered per video. For a video you have not played before,
`subtitle_language` picks one automatically, and `subtitle_size` and
`subtitle_font` control how they look - see
[Configuration](configuration.md).

> [!NOTE]
> * Blu-ray PGS subtitles will not be shown as GStreamer ships no decoder for them natively.
> * A video played from a media server integration offers only the embedded subtitles.

## Command line

Any of the menu choices can be given up front, and will skip straight to playback if required options are provided.

Track numbers are those `--list-tracks` prints.

| Option            | Meaning                                                                   |
|-------------------|---------------------------------------------------------------------------|
| `FILE`            | The video to play: a path, or a URL such as `http://…` or `smb://…`. Omit it to choose one in the window |
| `--primary <N>`   | Audio track for the primary output. `0` for no audio there                |
| `--secondary <N>` | Audio track for the secondary output. `0` for no audio there              |
| `--subtitle <N>`  | Subtitles to show. `0` for none                                           |
| `--list-tracks`   | Print the file's audio tracks and subtitles with their numbers, then exit |
| `--restart`       | Start video from the beginning, ignoring any saved position               |
| `--fullscreen`    | Start fullscreen                                                          |
| `--windowed`      | Start windowed, overriding a remembered fullscreen preference             |
| `--kodi`          | Launched by Kodi: sync the resume position with its library, and leave choosing the video to Kodi. Set for you by [the Kodi setup](integrations.md#kodi) |
| `-V`, `--version` | Print the version                                                         |
| `-h`, `--help`    | Print help                                                                |

```sh
TinePlayer --list-tracks video.mkv
TinePlayer video.mkv --primary 5 --secondary 1 --fullscreen
```

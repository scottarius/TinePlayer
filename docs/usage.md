# Using TinePlayer

Run it from the command line with or without arguments, or by double-clicking the executable.

**Linux**

```sh
./target/release/TinePlayer
./target/release/TinePlayer ~/Videos/film.mkv --fullscreen
```

**Windows**

```powershell
.\target\release\TinePlayer.exe
.\target\release\TinePlayer.exe "H:\Videos\film.mkv" --fullscreen
```

**Windows, via launch script**

```powershell
.\launch-tineplayer.cmd "H:\Videos\film.mkv" --fullscreen
```

Point anything that starts TinePlayer on your behalf at `launch-tineplayer.cmd`
rather than at the executable - a media center, a shortcut, a script. Started
from another program's folder, TinePlayer can pick up the wrong libraries and
fail silently; the script starts it from its own folder, passing arguments
through unchanged.

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

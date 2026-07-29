# Configuration

Everything in the settings menu is stored in `config.yaml`, which can also be
edited directly. It lives in the per-user config directory:
* Linux: `~/.config/tineplayer/config.yaml`
* Windows: `%LOCALAPPDATA%\tineplayer\config.yaml`

| Setting                       | Key                  | Default     | Meaning                                                        |
|-------------------------------|----------------------|-------------|----------------------------------------------------------------|
| Theme                         | `theme`              | `auto`      | `auto`, `light` or `dark`                                      |
| Interface Size                | `ui_scale`           | Unset       | Interface scale, such as `1.5` <br/>If unset scales automatically to the display resolution |
| Navigation Sounds             | `sounds`             | `true`      | Navigation clicks, `true` or `false`                           |
| Primary Audio Device          | `primary_sink`       | Unset       | Primary output device name. Required                           |
| Primary Language Preference   | `primary_language`   | Unset       | Preferred primary language, [see list below](#languages) <br/>If unset defaults to the first track |
| Secondary Audio Device        | `secondary_sink`     | Unset       | Second output device name <br/>`null` to play through primary only |
| Secondary Language Preference | `secondary_language` | Unset       | Preferred secondary language, [see list below](#languages) <br/>If unset defaults to the second track |
| Subtitle Language             | `subtitle_language`  | Unset       | Preferred subtitle language, [see list below](#languages) <br/>If unset shows no subtitles |
| Subtitle Size                 | `subtitle_size`      | `12`        | Point size against the video's resolution, not the screen's    |
| Subtitle Font                 | `subtitle_font`      | `Sans Bold` | Font Family and style name, [see below](#subtitle-fonts)       |

## Example

Everything is optional except `primary_sink`. Leave a setting out and its
default applies.

A fully commented version of the whole file is at
**[examples/config.yaml](../examples/config.yaml)**, if you would rather start
from that than build one up.

```yaml
theme: dark
ui_scale: 1.5
sounds: true

primary_sink: Built-in Audio Analogue Stereo
primary_language: en

secondary_sink: Sennheiser USB Headset Analogue Stereo
secondary_language: ru

subtitle_language: ru
subtitle_size: 12
subtitle_font: Sans Bold
```

Device names are the ones the settings menu lists, which are also what
`primary_sink` and `secondary_sink` are matched against. They differ by
platform: `Speakers (Realtek High Definition Audio)` on Windows looks nothing
like the example above.

TinePlayer writes a few keys of its own to the same file — `last_video`,
`last_folder` and `fullscreen` — so it reopens where you left it. There is no
need to set those by hand.

## Subtitle fonts

`subtitle_font` is a font family followed by optional style words. It carries no
size; that is `subtitle_size`.

The generic families resolve on any system, whatever is actually installed:

| Value              | Result                                     |
|--------------------|--------------------------------------------|
| `Sans`             | The system sans-serif face, plain          |
| `Sans Bold`        | The default, and the most legible in motion |
| `Serif`            | The system serif face                      |
| `Serif Italic`     | Serif, slanted                             |
| `Monospace Bold`   | Fixed width, bold                          |

Naming a family directly also works, where it is installed:

| Value                  | Where                     |
|------------------------|---------------------------|
| `DejaVu Sans Bold`     | Most Linux distributions  |
| `Liberation Sans Bold` | Most Linux distributions  |
| `Noto Sans Bold`       | Broad script coverage     |
| `Arial Bold`           | Windows and macOS         |
| `Verdana Bold`         | Windows and macOS         |
| `Segoe UI Semibold`    | Windows                   |

Style words are `Light`, `Medium`, `Semibold`, `Bold`, `Black`, `Italic`,
`Oblique` and `Condensed`, and they combine — `Noto Sans Condensed Bold Italic`
is valid.

A family that isn't installed is quietly substituted rather than reported, so
that is the first thing to check if subtitles come out looking wrong. Subtitles
in a non-Latin script need a face that covers it, which the generic families do
on most systems while something like `Arial` may not.

## Languages

| Code   | Language   | Native name |
|--------|------------|-------------|
| `en`   | English    | English     |
| `ru`   | Russian    | Русский     |
| `es`   | Spanish    | Español     |
| `fr`   | French     | Français    |
| `de`   | German     | Deutsch     |
| `it`   | Italian    | Italiano    |
| `pt`   | Portuguese | Português   |
| `nl`   | Dutch      | Nederlands  |
| `pl`   | Polish     | Polski      |
| `uk`   | Ukrainian  | Українська  |
| `cs`   | Czech      | Čeština     |
| `sv`   | Swedish    | Svenska     |
| `no`   | Norwegian  | Norsk       |
| `da`   | Danish     | Dansk       |
| `fi`   | Finnish    | Suomi       |
| `hu`   | Hungarian  | Magyar      |
| `tr`   | Turkish    | Türkçe      |
| `el`   | Greek      | Ελληνικά    |
| `he`   | Hebrew     | עברית       |
| `ar`   | Arabic     | العربية     |
| `hi`   | Hindi      | हिन्दी         |
| `ja`   | Japanese   | 日本語        |
| `ko`   | Korean     | 한국어        |
| `zh`   | Chinese    | 中文         |

Only the leading letters are compared, so `en` matches a track tagged `eng` or
`en-US`, and a subtitle file named `film.en.hi.srt`.

## Saved Playback Resume Data

Details about resuming playback of a video that didn't finish are kept in
`positions.json`, also in the per-user config directory:

* Linux: `~/.local/share/tineplayer/positions.json`
* Windows: `%LOCALAPPDATA%\tineplayer\positions.json`

Entries are keyed by the video's full path:

```json
{
  "H:\\Videos\\Films\\Example (2019).mkv": {
    "position_ns": 2297815000000,
    "tracks": {
      "primary": 0,
      "secondary": 1,
      "subtitle": { "Embedded": 2 }
    }
  }
}
```

| Field                  | Meaning                                                                                                                    |
|------------------------|----------------------------------------------------------------------------------------------------------------------------|
| `position_ns`          | Where playback stopped, in nanoseconds                                                                                     |
| `tracks`               | The choices made for this video, or `null` if none have ever been made                                                     |
| `primary`, `secondary` | Audio track for each output, counted from `0` <br/>`null` for no audio on that output                                      |
| `subtitle`             | `{"Embedded": N}` for a track inside the video, `{"External": "path"}` for a subtitle file beside it, `null` for no subtitles |

Track choices are kept per video rather than globally, because they are a
property of the file rather than of the machine: returning to a film you were
halfway through restores the languages you picked for it, not the ones you last
used on something else.

Note that tracks are counted from `0` here, one lower than the numbering
`--list-tracks` prints and `--primary` takes, where `0` means no audio.

A saved position under ten seconds is treated as no position at all - stopping
a few seconds in is a false start rather than a place you left off.

The path is the key, so moving or renaming a video loses its entry. Deleting
`positions.json` is harmless: it forgets every position and track choice, and
is rebuilt as you play things. This data can be cleared at any time from the
Settings menu.

# Configuration

Everything in the settings menu is stored in `config.yaml`, which can also be
edited directly. It lives in the per-user config directory:
* Linux: `~/.config/tineplayer/config.yaml`
* Windows: `%LOCALAPPDATA%\tineplayer\config.yaml`

| Setting                                                  | Key                  | Default     | Meaning                                                                                                                                        |
|----------------------------------------------------------|----------------------|-------------|------------------------------------------------------------------------------------------------------------------------------------------------|
| Theme                                                    | `theme`              | `auto`      | `auto`, `light` or `dark`                                                                                                                      |
| Interface Size                                           | `ui_scale`           | Unset       | Interface scale, such as `1.5` <br/>If unset scales automatically to the display resolution                                                    |
| Navigation Sounds                                        | `sounds`             | `true`      | Navigation clicks, `true` or `false`                                                                                                           |
| Primary Audio Device                                     | `primary_sink`       | Unset       | Primary output device name. Required                                                                                                           |
| Primary Language Preference                              | `primary_language`   | Unset       | Preferred primary [language code](#languages) <br/>If unset defaults to the first track                                                        |
| Secondary Audio Device                                   | `secondary_sink`     | Unset       | Second output device name <br/>`null` to play through primary only                                                                             |
| Secondary Language Preference                            | `secondary_language` | Unset       | Preferred secondary [language code](#languages) <br/>If unset defaults to the second track                                                     |
| [Subtitle Preference](#choosing-subtitles-automatically) | `subtitle_language`  | `primary_forced` | How a subtitle is chosen automatically: `none`, `primary_forced`, `primary`, `secondary_forced`, `secondary`, or a [language code](#languages) |
| Subtitle Size                                            | `subtitle_size`      | `12`        | Point size against the video's resolution, not the screen's                                                                                    |
| [Subtitle Font](#subtitle-fonts)                         | `subtitle_font`      | `Sans Bold` | Font Family and style name                                                                                                                     |
| Resume Threshold                                         | `resume_min_percent` | `5`         | How far in before stopping counts as somewhere to resume from, as a percentage of the running time <br/>Never less than 10 seconds             |
| Watched Threshold                                        | `watched_percent`    | `90`        | Past this percentage a video counts as watched, and its position is forgotten rather than saved                                                |

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

The two thresholds match what media servers do: Jellyfin resumes past 5% and
counts 90% as watched, and Plex and Kodi also treat 90% as watched. Raise
`resume_min_percent` if you often start something, change your mind, and would
rather it began again next time.

TinePlayer writes a few keys of its own to the same file - `last_video`,
`last_folder` and `fullscreen` - so it reopens where you left it. There is no
need to set those by hand.

## Audio description

An audio description track narrates what is happening on screen, for a viewer
who is blind or has low vision. Because each output picks its own track, one
person can hear the description while everyone else hears the ordinary
soundtrack.

Set **Prefer Audio Description** to Yes under either output to have described
tracks chosen automatically. It can be combined with a language preference,
and a described track in another language is never chosen.

### Where to find described audio

If you have a video file without a described audio track, you can check
[Audiovault](https://audiovault.net/) to find one, and use [this fork of
describealaign](https://github.com/matalvernaz/describealaign) to combine
the audio track with your video file, which specifically preserves the
original audio along with the described audio track instead of replacing it,
which makes the resulting file usable with TinePlayer.

Both are third-party projects with no connection to TinePlayer, mentioned
because they answer a real question rather than as a recommendation.

## Choosing subtitles automatically

`subtitle_language` decides what to show for a video you have not picked
subtitles for yourself. A choice you make for a video is remembered and always
wins over this.

| Value                       | Chooses                                                         |
|-----------------------------|-----------------------------------------------------------------|
| `none`                      | nothing                                                         |
| `primary_forced`            | forced only, preferring the primary output's language (default) |
| `primary`                   | full subtitles in the primary output's language                 |
| `secondary_forced`          | forced only, preferring the secondary output's language         |
| `secondary`                 | full subtitles in the secondary output's language               |
| [language code](#languages) | full subtitiles in a specific language                          |

The language followed is the one actually playing on that output, not the
Primary or Secondary Language Preference, so it stays right even when a
preference matched nothing and the first track was used instead.

Forced subtitles carry only what a viewer who understands the dialogue still
needs: alien speech, foreign lines, signs. The forced modes prefer one output
language but will fall back to the other, then to none.

> [!NOTE]
> Forced tracks are recognized by name, since the flag containers have for this
> is usually not set and GStreamer does not expose it. A track titled `Forced`,
> or a file named `Film.en.forced.srt`, is recognized; one that is forced in
> fact but named something else is not.

## Subtitle fonts

`subtitle_font` is a font family followed by optional style words. It carries
no size; that is `subtitle_size`.

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
`Oblique` and `Condensed`, and they combine - `Noto Sans Condensed Bold Italic`
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
      "subtitle": { "External": "Example (2019).en.srt" }
    }
  }
}
```

| Field                  | Meaning                                                                                                                    |
|------------------------|----------------------------------------------------------------------------------------------------------------------------|
| `position_ns`          | Where playback stopped, in nanoseconds                                                                                     |
| `tracks`               | The choices made for this video, or `null` if none have ever been made                                                     |
| `primary`, `secondary` | Audio track for each output, counted from `0` <br/>`null` for no audio on that output                                      |
| `subtitle`             | `{"Embedded": N}` for a track inside the video, `{"External": "name"}` for a subtitle file beside it, `null` for no subtitles <br/>The file is stored by name, not path, so the choice survives the library moving |

Track choices are kept per video rather than globally, because they are a
property of the file rather than of the machine: returning to a film you were
halfway through restores the languages you picked for it, not the ones you last
used on something else.

> [!NOTE]
> Tracks are counted from `0` here, one lower than the numbering
> `--list-tracks` prints and `--primary` takes, where `0` means no audio.

A saved position under ten seconds is treated as no position at all - stopping
a few seconds in is a false start rather than a place you left off.

The path is the key, so moving or renaming a video loses its entry. Deleting
`positions.json` is harmless: it forgets every position and track choice, and
is rebuilt as you play things. This data can be cleared at any time from the
Settings menu.

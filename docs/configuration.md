# Configuration

All settings can be configured from the **Settings** menu inside TinePlayer,
which offers only values that make sense and lists the audio devices this
machine actually has. For driving the player itself, see
[Using TinePlayer](usage.md).

![TinePlayer's settings screen: interface size with its automatic switch, and
each output's language, description, volume and audio
sync.](screenshots/settings-menu.png)

The same settings are stored in `config.yaml`, which can be edited directly. TinePlayer reads it at startup, so
restart it after editing.

It lives in the per-user config directory:
* Windows: `%LOCALAPPDATA%\TinePlayer\config.yaml`
* macOS: `~/.config/tineplayer/config.yaml`
* Linux: `~/.config/tineplayer/config.yaml`


| Setting                                                  | Key                  | Default     | Description                                                                                                                                    |
|----------------------------------------------------------|----------------------|-------------|------------------------------------------------------------------------------------------------------------------------------------------------|
| [Interface Size](#interface-size)                        | `ui_scale`           | Unset       | Interface scale, such as `1.5`, from `0.33` to `3` <br/>If unset it defaults to 1.0 and auto-scales to the display resolution in fullscreen    |
| Navigation Sounds                                        | `sounds`             | `true`      | Navigation clicks, `true` or `false`                                                                                                           |
| Check for updates                                        | `check_for_updates`  | `true`      | Asks GitHub once a day whether a newer TinePlayer has been released, `true` or `false` <br/>Nothing is ever downloaded or installed            |
| [Primary Audio Device](#output-devices)                  | `primary_sink`       | Unset       | Primary output device name. Required                                                                                                           |
| Primary Language Preference                              | `primary_language`   | Unset       | Preferred primary [language code](#languages) <br/>If unset defaults to the first track                                                        |
| [Secondary Audio Device](#output-devices)                | `secondary_sink`     | Unset       | Second output device name <br/>`null` to play through primary only                                                                             |
| Secondary Language Preference                            | `secondary_language` | Unset       | Preferred secondary [language code](#languages) <br/>If unset defaults to the second track                                                     |
| [Subtitle Preference](#choosing-subtitles-automatically) | `subtitle_language`  | `primary_forced` | How a subtitle is chosen automatically: `none`, `primary_forced`, `primary`, `secondary_forced`, `secondary`, or a [language code](#languages) |
| Subtitle Size                                            | `subtitle_size`      | `12`        | Point size against the video's resolution, not the screen's, from `8` to `24`                                                                  |
| [Subtitle Font](#subtitle-fonts)                         | `subtitle_font`      | `Sans Bold` | Font Family and style name                                                                                                                     |
| Resume Threshold                                         | `resume_min_percent` | `5`         | How far in before stopping counts as somewhere to resume from, as a percentage of the running time. Never less than 10 seconds                 |
| Watched Threshold                                        | `watched_percent`    | `90`        | Past this percentage a video counts as watched, and its position is forgotten rather than saved                                                |
| [Prefer Audio Description](#audio-description)           | `primary_audio_description` <br/>`secondary_audio_description` | `false` | Whether that output prefers a described track                                                                                                  |
| [Volume](#volumes)                                       | `primary_volume` <br/>`secondary_volume` | `1.0` | That output's level, from `0.0` to `1.0`                                                                                                       |
| Mute                                                     | `primary_muted` <br/>`secondary_muted` | `false` | Whether the output is muted                                                                                                                    |
| [Audio Sync](#audio-sync)                                | `primary_offset_ms` <br/>`secondary_offset_ms` | `0` | Adjust the sync of the audio to line it up with the picture, in milliseconds from `-1000` to `1000`                                            |
|                                                          | `primary_offset_on` <br/>`secondary_offset_on` | `false` | Whether that output's sync adjustment is applied. Off keeps the value without using it                                                         |

## Remembered State

Some settings are written by TinePlayer in order to remember state.
They appear in `config.yaml` alongside everything else, and can be edited or
deleted.

| Key           | What it remembers                                                      |
|---------------|------------------------------------------------------------------------|
| `last_folder` | Where the file browser was, so it reopens there                        |
| `last_video`  | The most recent video chosen, remembered when re-opening the app       |
| `fullscreen`  | The remembered fullscreen toggled state                                |
| `kodi_paths`  | Any custom Kodi integrations. See [Integrations](integrations.md#kodi) |

## Example

Everything is optional except `primary_sink`. Leave a setting out and its
default applies.

A fully commented version of the whole file is at
**[examples/config.yaml](../examples/config.yaml)**, if you would rather start
from that than build one up.

```yaml
ui_scale: 1.5
sounds: true
check_for_updates: true

primary_sink: Built-in Audio Analogue Stereo
primary_language: en

secondary_sink: Sennheiser USB Headset Analogue Stereo
secondary_language: ru

subtitle_language: ru
subtitle_size: 12
subtitle_font: Sans Bold
```

## Interface Size

The interface can be scaled to your liking, or left to auto-scale itself.

When auto-scaling is enabled (default) the normal scale will be 1.0x when
windowed, and when fullscreen it scales to the display's height, using 1080p as
the 1.0x baseline, so a 4K TV goes to 2.0x. The scale is based on the display
height the desktop reports, which takes into account its own scaling.

Auto-scaling can be disabled in the Settings menu or by setting `ui_scale`
in the config file. A size set this way applies to both windowed and
fullscreen, and is held to the same `0.33` to `3` the slider offers.

## Output Devices

The two output devices,`primary_sink` and `secondary_sink`, are set by device name and can differ by
platform.

To get the available device names, run TinePlayer with the following argument from the command line:

```sh
tineplayer --list-devices
```
## Volumes

Each output carries its own level and can be adjusted or muted independently.
The volume settings persist between sessions. Set them from the settings menu
under either output, or from the volume button during
[playback](usage.md#volume-and-sync).

Holding or long-pressing the
primary volume button will mute both immediately, and holding it again restores
what each output was doing. Adjusting anything in the panel keeps the muted
state instead, and your change applies from there.

## Audio Sync

Bluetooth headphones or other speaker setups may add noticeable delay, and not
all systems compensate for it. Or perhaps the audio track is just not in sync
with the video to begin with. This setting will allow you to nudge the audio
sync forward or backwards to line it up by ear.

`primary_offset_ms` and `secondary_offset_ms` adjust the sync by the
given number of milliseconds, from `-1000` to `1000`:

| Value      | Use when                                          |
|------------|---------------------------------------------------|
| Positive   | The sound arrives before the picture. It is held back |
| Negative   | The sound arrives after the picture. It is moved earlier |
| `0`        | The output needs no correction                    |

Each output is corrected separately, so wireless headphones can be lined up
without touching what's playing on speakers.

Each one has a switch beside it in **Settings**, and starts off - nobody needs
a delay until they find they do. Turning it off again keeps the value without
using it, which is how to hear whether a delay is helping: winding it to zero
answers the same question and loses the figure.

**Setting the delay in the file means setting both.** `primary_offset_ms` on
its own does nothing until `primary_offset_on` is `true` alongside it. The
reading shows `0ms` while an output is switched off, whatever the value behind
it is.

It can be adjusted live during playback from the volume button. See
[Volume and sync](usage.md#volume-and-sync).

## Audio Description

An audio description track narrates what is happening on screen, for a viewer
who is blind or has low vision. Because each output picks its own track, one
person can hear the description while everyone else hears the ordinary
soundtrack.

Set **Prefer Audio Description** to Yes under either output to have described
tracks chosen automatically. It can be combined with a language preference,
and a described track in another language is never chosen.

Described tracks are recognized by their title, since no container flag for
them exists. For where to find described audio, how to add it to a file you
already have, and how to title the track so it is picked up, see
[Where to Get Multi-track Videos](multi-track-video.md#audio-description).

## Choosing Subtitles Automatically

`subtitle_language` decides what to show for a video you have not yet picked
subtitles for yourself. A choice you make for a video is remembered and always
wins. For choosing subtitles while watching, and which files are offered
alongside the embedded tracks, see [Subtitles](usage.md#subtitles).

| Value                       | Chooses                                                         |
|-----------------------------|-----------------------------------------------------------------|
| `none`                      | Nothing                                                         |
| `primary_forced`            | Forced only, preferring the primary output's language (default) |
| `primary`                   | Full subtitles in the primary output's language                 |
| `secondary_forced`          | Forced only, preferring the secondary output's language         |
| `secondary`                 | Full subtitles in the secondary output's language               |
| [language code](#languages) | Full subtitles in a specific language                          |

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

## Subtitle Fonts

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

These are the languages TinePlayer offers for **Primary/Secondary Language
Preference** and for **Subtitle Preference**. Use the code in `config.yaml`.

The list is deliberately not every language ISO defines, but what generally
turns up as an alternate audio track on commercial discs and the rips made
from them. A language that is missing can still be played by choosing its
track by hand.

| Code | Language | Native name |
|------|----------|-------------|
| `ar` | Arabic | العربية |
| `hy` | Armenian | Հայերեն |
| `az` | Azerbaijani | Azərbaycan |
| `bn` | Bengali | বাংলা |
| `bs` | Bosnian | Bosanski |
| `bg` | Bulgarian | Български |
| `yue` | Cantonese | 粵語 |
| `ca` | Catalan | Català |
| `zh` | Chinese | 中文 |
| `hr` | Croatian | Hrvatski |
| `cs` | Czech | Čeština |
| `da` | Danish | Dansk |
| `nl` | Dutch | Nederlands |
| `en` | English | English |
| `et` | Estonian | Eesti |
| `fi` | Finnish | Suomi |
| `fr` | French | Français |
| `ka` | Georgian | ქართული |
| `de` | German | Deutsch |
| `el` | Greek | Ελληνικά |
| `he` | Hebrew | עברית |
| `hi` | Hindi | हिन्दी |
| `hu` | Hungarian | Magyar |
| `is` | Icelandic | Íslenska |
| `id` | Indonesian | Bahasa Indonesia |
| `it` | Italian | Italiano |
| `ja` | Japanese | 日本語 |
| `kk` | Kazakh | Қазақша |
| `ko` | Korean | 한국어 |
| `lv` | Latvian | Latviešu |
| `lt` | Lithuanian | Lietuvių |
| `ms` | Malay | Bahasa Melayu |
| `ml` | Malayalam | മലയാളം |
| `no` | Norwegian | Norsk |
| `fa` | Persian | فارسی |
| `pl` | Polish | Polski |
| `pt` | Portuguese | Português |
| `pa` | Punjabi | ਪੰਜਾਬੀ |
| `ro` | Romanian | Română |
| `ru` | Russian | Русский |
| `sr` | Serbian | Српски |
| `sk` | Slovak | Slovenčina |
| `sl` | Slovenian | Slovenščina |
| `es` | Spanish | Español |
| `sv` | Swedish | Svenska |
| `tl` | Tagalog | Tagalog |
| `ta` | Tamil | தமிழ் |
| `te` | Telugu | తెలుగు |
| `th` | Thai | ไทย |
| `tr` | Turkish | Türkçe |
| `uk` | Ukrainian | Українська |
| `ur` | Urdu | اردو |
| `vi` | Vietnamese | Tiếng Việt |

Only the leading letters are compared, so `en` matches a track tagged `eng` or
`en-US`, and a subtitle file named `film.en.hi.srt`.

If a language you need is missing,
[say so](https://github.com/scottarius/TinePlayer/issues) and it can be added.

## Saved Playback Resume Data

Details about resuming playback of a video that didn't finish are kept in
`positions.json`, also in the per-user config directory:

* Windows: `%LOCALAPPDATA%\TinePlayer\positions.json`
* macOS: `~/.local/share/tineplayer/positions.json`
* Linux: `~/.local/share/tineplayer/positions.json`

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

A saved position under the resume threshold setting is treated as no position at all as stopping
a few seconds in is treated as a false start rather than a place you left off.

The path is the key, so moving or renaming a video loses its entry. Deleting
`positions.json` is harmless: it forgets every position and track choice, and
is rebuilt as you play things. This data can be cleared at any time from the
Settings menu.

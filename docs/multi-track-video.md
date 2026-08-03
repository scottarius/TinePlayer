# Where to Get Multi-track Videos

TinePlayer plays two audio tracks at once, which means it needs a video file
that carries more than one. This page is about finding files that do, checking
what a file has, and building one yourself when it doesn't.

## Checking Files for Multiple Tracks

```sh
tineplayer --list-tracks "film.mkv"
```

That prints every audio track and subtitle with its number, language and
title. The same list appears in the app under **Primary Audio Track**, so you
can also just open the file and look.

A file with one audio track still plays, the second output will just remain silent.

## Sources That Contain Multiple Tracks

| Source | What you tend to get                                                                        |
|--------|---------------------------------------------------------------------------------------------|
| Blu-ray and DVD discs | Several languages, plus commentary and sometimes description. The richest source by far     |
| Blu-ray and DVD rips (MKV) | Potentially everything the disc had, or a subset. Depends on the settings when ripped       |
| Broadcast recordings (`.ts`) | Often a second language or a described track, depending on the broadcaster                  |
| Streaming downloads | Usually one track. Services generally deliver the language you chose rather than all of them |

The pattern is that **physical media and broadcast carry multiple tracks;
streaming usually doesn't**. A disc has to serve everyone who buys it, so the
tracks are all on there. A stream is assembled for one viewer.

## Ripping your own Blu-ray or DVD Discs

Ripping discs you own into a personal media library is how a great many people
build one - it is what most Kodi, Jellyfin and Plex libraries are made of, and
those are exactly the libraries TinePlayer is meant to play from.

The one thing to watch is that most ripping tools keep a single audio track
unless told otherwise. That means the tracks you want are usually lost at this
step rather than never having been there, and a file with one audio track
leaves TinePlayer with nothing to send to a second output. It is worth getting
right the first time: re-ripping a shelf of discs is a long evening.

### MakeMKV

[MakeMKV](https://makemkv.com/) reads Blu-rays and DVDs and writes MKV files.
It copies rather than re-encodes, so it is fast, loses no quality, and keeps
the track titles and language tags the disc carried - which is what TinePlayer
reads to choose tracks.

Expand the title in the track list before starting and **tick every audio
track you want**, rather than trusting the selection it opens with. The
default rules drop tracks in languages you have not listed as preferred, and
a described track often has no language tag at all, so it is one of the first
things to be left behind.

For most people this is the whole job: the MKV it produces is ready to play.

### HandBrake

[HandBrake](https://handbrake.fr/) re-encodes, which is for making a file
smaller rather than for getting it off a disc. If size is not a concern, skip
it - every re-encode costs quality and hours.

If you do use it, two settings matter:

- **Audio → Selection Behavior**: set the track selection to *All* rather than
  the default, which takes one track matching a single language.
- **Audio → Codec**: choose *Auto Passthru* so the audio is copied rather than
  re-encoded alongside the video.

HandBrake does not always carry track titles across. Check the result and
retitle anything that lost its name, with `mkvpropedit` as shown below -
particularly a described track, which is recognised by its title.

### Afterwards

```sh
tineplayer --list-tracks "film.mkv"
```

If a track you expected is missing, it was dropped during the rip rather than
by TinePlayer, and it is quicker to rip again than to hunt for it.

## Audio Description

An audio description track narrates what is happening on screen for a viewer
who is blind or has low vision. It is what makes TinePlayer's two outputs
useful beyond language: one person hears the description on headphones while
the room hears the ordinary soundtrack.

Some discs and broadcasts carry one already. If they don't, you can check:

- **[Audiovault](https://audiovault.net/)** collects described audio tracks
  for films and television.
- **[This fork of describealign](https://github.com/matalvernaz/describealaign)**
  combines a described track with your video file. Use this fork rather than
  the original: it keeps the original audio *alongside* the described track
  instead of replacing it, which is what leaves a file TinePlayer can use.

Both are third-party projects with no connection to TinePlayer, mentioned
because they answer a real question rather than as a recommendation.

Once the file has both, see [Audio
Description](configuration.md#audio-description) for having TinePlayer choose
the described track automatically.

## Combining Tracks Yourself

Any two audio files can be muxed into one video with
[MKVToolNix](https://mkvtoolnix.download/), the standard set of tools for
working with MKV files, or [ffmpeg](https://ffmpeg.org/), which handles media
files generally. Both are separate programs and have to be installed. Nothing
is re-encoded - the tracks are copied as they are, so there is no quality loss
and it takes seconds rather than hours.

```sh
mkvmerge -o "film.mkv" "video-with-english.mkv" \
    --language 0:es --track-name "0:Español" "spanish-audio.m4a"
```

The same with `ffmpeg`:

```sh
ffmpeg -i "video-with-english.mkv" -i "spanish-audio.m4a" \
    -map 0 -map 1:a -c copy \
    -metadata:s:a:1 language=spa -metadata:s:a:1 title="Español" \
    "film.mkv"
```

## Tagging Tracks So TinePlayer Can Choose Them

Muxing is only half of it. TinePlayer picks tracks automatically from what the
file says about them, so the tags are worth getting right.

**Language** comes from the track's language tag, and is what a language
preference matches against. An untagged track can still be chosen by hand, but
never automatically.

For **Audio Description** a title counts as described if, ignoring case, it contains any of:

| Pattern | Matches titles like |
|---------|---------------------|
| `descri` | "Audio Description", "Described", "Descriptive Audio" |
| `visually impaired`, `visual impaired`, `impaired vision` | "English - Visually Impaired" |
| `narration` | "Narration Track" |
| `ad` as a whole word | "AD", "English AD" |

**"Commentary" is deliberately not on that list.** A director's commentary is
a different thing, and files carry both. If your described
track is titled only "Commentary", retitle it.

So, when muxing a described track, give it a title that says so:

```sh
mkvmerge -o "film.mkv" "film-original.mkv" \
    --language 0:en --track-name "0:English Audio Description" "described.m4a"
```

An existing file's tags can be changed without remuxing, with
`mkvpropedit` from the same MKVToolNix set:

```sh
mkvpropedit "film.mkv" --edit track:a2 --set "name=English Audio Description"
mkvpropedit "film.mkv" --edit track:a2 --set language=eng
```

## What TinePlayer Will Not Do

It plays what a file contains and does not fetch, generate or synthesise
anything. There is no text-to-speech description, and no downloading a second
language on demand. If the file has one audio track, it has one audio track.

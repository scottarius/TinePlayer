# Troubleshooting

If none of this covers what you are seeing, please
[open an issue](https://github.com/scottarius/TinePlayer/issues). Something
going wrong and not being listed here is itself worth knowing about.

## It will not start

### Windows says it protected your PC

SmartScreen does not recognize the download, because it is not signed with a
certificate yet. Choose **More info**, then **Run anyway**.

### apt cannot find the package

```
E: Unable to locate package tineplayer_1.0.0_linux_amd64.deb
```

The leading `./` is missing. Without it apt looks for a package by that name
in its own sources rather than installing the file in front of it:

```sh
sudo apt install ./tineplayer_1.0.0_linux_amd64.deb
```

### apt says the download is performed unsandboxed

```
N: Download is performed unsandboxed as root as file
'/home/you/Downloads/tineplayer_1.0.0_linux_arm64.deb' couldn't be accessed
by user '_apt'. - pkgAcquire::Run (13: Permission denied)
```

Nothing went wrong, and the package installs normally. The `N:` marks it as a
note rather than an error.

apt normally copies a file as the unprivileged `_apt` user, and cannot here
because your home directory is not readable by anyone else - Raspberry Pi OS
and some other distributions create them that way. It falls back to copying as
root and says so.

Check that it installed:

```sh
dpkg -l tineplayer
```

To avoid the message, install from somewhere apt can reach:

```sh
mv tineplayer_1.0.0_linux_arm64.deb /tmp
sudo apt install /tmp/tineplayer_1.0.0_linux_arm64.deb
```

### A build from source will not start on Windows

If something else launches TinePlayer from its own folder, Windows may find
that program's copies of libraries GStreamer also uses before GStreamer's own.
Start it through `launch-tineplayer-windows.cmd` instead. See [Built from
source](usage.md#built-from-source). Installed builds are unaffected.

## No sound, or only one output

### No devices are offered

TinePlayer needs a primary output device set before it can play anything.
Choose one in **Settings → Audio**.

On Linux, devices come from PulseAudio or PipeWire. If the list is empty,
check that one of them is running and can see your hardware:

```sh
pactl list short sinks
```

To see the list TinePlayer itself has, which is what the menu offers:

```sh
tineplayer --list-devices
```

A machine with only ALSA will play through the primary output but has no
device list to choose from.

### Nothing comes out of the second output

Check that **Secondary Audio Device** is set to a different device from the
primary, and that the video actually has a second audio track:

```sh
tineplayer --list-tracks film.mkv
```

A file with one audio track has nothing to send to a second output.

### The sound is not in sync with the video

If using a Bluetooth output it can add 100-200ms of delay which puts it behind
both the picture and the other output. Latency compensation may or may not work
depending on your system and device.

It's recommended to use wired or built-in audio devices, or a lag-free wireless headset with a USB dongle.

## Audio tracks

### Track names do not show in TinePlayer but show elsewhere (VLC)

The file is probably an MP4. Track names there are kept in a QuickTime `udta`/`name`
box, which players like VLC read and GStreamer does not, so the tracks often
display in TinePlayer with no name or language.

That also means a described track cannot be chosen automatically, since
[Prefer Audio Description](configuration.md#audio-description) recognizes one
by its title.

Remuxing to MKV carries the names across and can be done with
[ffmpeg](https://ffmpeg.org/), a command-line tool for working with media
files. Nothing is re-encoded, so it takes seconds rather than hours:

```sh
ffmpeg -i film.mp4 -map 0 -c copy \
    -metadata:s:a:0 language=eng -metadata:s:a:0 title="Original" \
    -metadata:s:a:1 language=eng -metadata:s:a:1 title="Audio Description" \
    film.mkv
```

Adjust the numbers to match the tracks, then check:

```sh
tineplayer --list-tracks film.mkv
```

### A described track is not chosen automatically

Check that **Prefer Audio Description** is on for that output, and that the
track's title says what it is:

```sh
tineplayer --list-tracks film.mkv
```

A title counts if it contains `descri`, `narration`, `visually impaired`, or
`ad` as a word of its own. "Commentary" alone does not - a director's
commentary is a different thing, and files often carry both.

A title can be changed without remuxing, with `mkvpropedit` from
[MKVToolNix](https://mkvtoolnix.download/):

```sh
mkvpropedit film.mkv --edit track:a2 --set name="English Audio Description"
```

## Subtitles

### A subtitle track in the file is not in the list

Blu-ray discs use PGS subtitles, which are images rather than text, and
GStreamer ships no decoder for them. TinePlayer leaves those tracks out
rather than offering one that would draw nothing, so they appear neither in
the menu nor in `--list-tracks`.

### A subtitle file beside the video is not offered

External subtitle files are only found for videos opened by path - a local
file, a UNC path, or a mounted share. A video opened as `http://` or `smb://`
offers only the subtitles embedded in it.

Check the name, too. The file has to start with the video's name and end in
`.srt`, `.ass`, `.ssa` or `.vtt`. See [Subtitles](usage.md#subtitles).

## Kodi

### Kodi still plays videos itself

Restart Kodi first. It reads `playercorefactory.xml` once, at startup, so a
change made while it is running has no effect until then.

If it still does, check which way it was set up: **Settings → Kodi** in
TinePlayer lists each Kodi it is configured in and what that one is set to do.

* **Default Player** hands every video over automatically. If it is set to
  this and Kodi is still playing videos itself, something is wrong - see
  [Choosing TinePlayer does nothing](#choosing-tineplayer-does-nothing) below.
* **Optional Player** leaves Kodi playing videos as usual, and TinePlayer has
  to be picked per video. In Kodi, highlight a video and open its context menu
  - <kbd>C</kbd> on a keyboard, <kbd>Menu</kbd> on a remote, or a long press
  on a touchscreen - then choose **Play using...** and pick TinePlayer.

If **Play using...** is not in that menu at all, see [There is no "Play
using..." anywhere](#there-is-no-play-using-anywhere).

Re-running the setup and choosing **Default Player** switches it over if you
would rather not pick each time.

### There is no "Play using..." anywhere

On Kodi 20 and earlier it only appears under **Videos → Files**, not in the
libraries. Kodi 21 and later show it throughout. Setting TinePlayer as
**Default Player** instead avoids the menu entirely.

### Choosing TinePlayer does nothing

If Kodi is installed as a Flatpak, it needs permission to start programs
outside its sandbox. See [If Kodi is
sandboxed](integrations.md#if-kodi-is-sandboxed).


## Settings

### Settings went back to their defaults

A `config.yaml` that cannot be parsed is not overwritten. TinePlayer starts on
defaults, says so on screen, and copies the file to `config.yaml.invalid`
beside it so you can see what went wrong. Fix the file, rename it back, and
restart.

### A video does not resume where it stopped

Positions are keyed by the video's full path, so moving or renaming a file
loses its entry. A position under ten seconds is also treated as no position
at all. See [Saved Playback Resume
Data](configuration.md#saved-playback-resume-data).

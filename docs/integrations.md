# Integrations

TinePlayer can be launched by another application to play a video, handing
control back when playback ends. A media center app can do this to keep its
own library browsing while TinePlayer handles playback, but a launcher does
not have to be a library: a script, a file manager, or a keyboard shortcut works
the same way.

## Launching from another application

Launch the executable by passing the video and the `--external` flag:

### Windows

```powershell
TinePlayer.exe "C:\Videos\film.mkv" --external --fullscreen
```

Or if built from source, prefer the launch script if pointing at the exe directly fails to launch:

```powershell
launch-tineplayer-windows.cmd "C:\Videos\film.mkv" --external --fullscreen
```

### macOS

```sh
/Applications/TinePlayer.app/Contents/MacOS/TinePlayer "/Users/you/Movies/film.mkv" --external --fullscreen
```

### Linux

```sh
tineplayer "/home/you/Videos/film.mkv" --external --fullscreen
```

Passing `--external` puts TinePlayer into an integration mode, where it plays
only the video provided (file browser is disabled) and exits immediately after
the video finishes to return control to the launching application.

Adding `--fullscreen` forces fullscreen mode for the whole
run: the fullscreen buttons are hidden and the shortcuts for it do nothing, so
a viewer cannot end up on the desktop behind a launcher that is waiting. See
[Fixed fullscreen](usage.md#fixed-fullscreen). Leave `--fullscreen` off if you
would rather it stayed toggleable.

Additionally, the `--primary`, `--secondary` and `--subtitle` arguments can be
supplied to skip the menu entirely and go directly to playback. See the [
command line usage](usage.md#command-line).

Below are some supported integrations and how to set them up.

## Kodi

[Kodi](https://kodi.tv) is a media center application: it catalogs your films
and TV files with artwork and plays them on a television (or other device). It can hand playback
to TinePlayer rather than playing a video itself, and TinePlayer will report
watch percentage back to Kodi.

There are two ways to set it up:

* **Default Player** - Kodi hands every video straight to TinePlayer.
* **Optional Player** - Kodi keeps playing videos itself, and TinePlayer
  appears under **Play using...** in a video's context menu.

> [!IMPORTANT]
> Which Kodi version you run changes where **Play using...** appears:
>
> * On Kodi 21 and later, throughout.
> * On Kodi 20 and earlier, only under **Videos → Files**, not in the
>   libraries.
>
> This affects **Optional Player** only. **Default Player** works the same on
> every version.

### From TinePlayer

The easiest way to configure Kodi is from TinePlayer itself. In
TinePlayer, open **Settings** and choose **Kodi**. You will see a list of any install of Kodi that was found, and how each one is
configured.

**Add Configuration** starts the configuration wizard with the
following steps:

1. **Choose a Kodi Installation**

   Choose one of the detected installs. Picking one that is already configured
   will update its configuration. If yours is somewhere unusual, **Custom
   install location** opens a folder browser to point at Kodi's `userdata`
   folder.

2. **How to Configure**

   Default Player or Optional Player, as above.

3. **When TinePlayer Starts**

   **Play Video** starts the film right away, using the tracks remembered for
   that video or your language preferences. **Show the Menu** opens
   TinePlayer's menu so the audio tracks and subtitles can be chosen for each
   video.

4. **Confirm Configuration**

   Shows the file that will be changed, the backup that will be kept, and what
   will be added. Choose **Configure** to write the configuration.

To remove a configuration from Kodi, choose it in the list and confirm.

An existing `playercorefactory.xml` is edited in place rather than replaced.
Other players in it, and your own comments and formatting, are left exactly as
they are. A backup is made before editing just in case.

Restart Kodi for the changes to take effect.

#### If Kodi is sandboxed

Kodi installed on Linux as a Flatpak starts an external player *inside its own sandbox*,
where TinePlayer is not installed and your files are not visible. TinePlayer
writes a command that steps out to the machine first, but Kodi does not ship
with permission to do that, so the setup shows you the one command to run:

```sh
flatpak override --user --talk-name=org.freedesktop.Flatpak tv.kodi.Kodi
```

> [!IMPORTANT]
> That permission lets Kodi run **any** program on your machine, not only
> TinePlayer. TinePlayer will never run it for you. To undo it:
> `flatpak override --user --reset tv.kodi.Kodi`, which clears every override
> you have set for Kodi.

If Kodi is installed as a **Snap** it is not supported as Snap confinement offers
no way to start a program outside itself. TinePlayer lists such an install
and marks it unsupported rather than letting it be configured. Use a Kodi from
your distribution's packages, or from Flathub.

### Manual Installation

Kodi has no interface for this, so it means editing `playercorefactory.xml`
yourself. It lives in Kodi's userdata directory, and you can create it if it
isn't there already:

* Windows: `%APPDATA%\Kodi\userdata\playercorefactory.xml`
* macOS: `~/Library/Application Support/Kodi/userdata/playercorefactory.xml`
* Linux: `~/.kodi/userdata/playercorefactory.xml`
* Linux, Kodi as a Flatpak:
  `~/.var/app/tv.kodi.Kodi/data/userdata/playercorefactory.xml`

**[examples/playercorefactory.xml](../examples/playercorefactory.xml)** is a
complete, fully commented copy to start from. Two things required to change:

* Set `<filename>` to the command that starts TinePlayer, which depends on how
  both programs were installed:

  | Kodi | TinePlayer | What Kodi has to run |
  |------|------------|----------------------|
  | Normal install | Installed | the executable, by path |
  | Normal install | Built from source, on Windows | `launch-tineplayer-windows.cmd` at the top of the source tree, if the executable alone fails to start. See [Built from source](usage.md#built-from-source) |
  | Flatpak | either | `/usr/bin/flatpak-spawn`, with `--host` and then the path above |
* Uncomment the `<rules>` block at the bottom to make TinePlayer the default
  player. Left commented, TinePlayer appears under **Play using...** instead.

Restart Kodi to see changes.

### Add-Ons

Add-ons can bring an external media server's library into Kodi. With correct
configuration Kodi may be able to hand off those video files to TinePlayer as
well.

Requirements for an Add-On to work with TinePlayer:

* **It must leave playback to Kodi.** Some add-ons play videos themselves
  rather than handing them back, and Kodi then never offers **Play using...**
  at all.
* **It must provide the raw video file**, without conversion or transcoding,
  for direct play.
* **It must provide an accessible path to the file.** Either as a local path, a
  path on an accessible network share (SMB), or a direct link over HTTP.

Tested Add-Ons:

* **[Jellyfin](#jellyfin)**
* **[Plex](#plex)**

## Jellyfin

[Jellyfin](https://jellyfin.org) is a free software media server. It catalogs
your films and TV and streams them to clients over the network.

### Native

Not yet supported, but being investigated for a future release.

### Integrate via Kodi Add-On

Follow the steps above to set up Kodi handoff to TinePlayer and then install
[Jellyfin for Kodi](https://jellyfin.org/docs/general/clients/kodi/).

During setup, choose an appropriate Playback Mode:
* **Add-on (default)** This is the recommended option, should work for most
  cases.
* **Native (direct paths)** Only use this if Kodi can reach the video files
  directly, on the same machine or a share it can open.

## Plex

[Plex](https://www.plex.tv) is a media server with free and paid versions that
catalogs your films and TV and streams them to clients over the network. Plex
clients do not currently provide support for external player handoff, but can
work via Kodi.

### Integrate via Kodi Add-On

Install [PlexKodiConnect](https://github.com/croneter/PlexKodiConnect/wiki) by
its own instructions, then set up Kodi handoff as above.

> [!IMPORTANT]
> Kodi external player handoff only works with PKC when **Playback Mode** is
> set to **Direct Paths**. This means that media served from Plex can only be
> handed off to TinePlayer if it's accessible locally, not over the internet.

Direct Paths hands over the "local" path that the Plex server uses. This path
will work as-is if Kodi is running on the same machine as the Plex server. If
not, the paths need to be mapped to a network path that's accessible from where
Kodi is running in the PKC Add-On settings under **Customize Paths**:

* Enable *Replace Plex paths with custom SMB paths*
* Set the original path to what Plex uses, e.g. `/mnt/media/Movies`
* Set the replacement to a path this machine can reach, e.g.
  `smb://server/media/Movies` or a mounted path such as `/mnt/nas/Movies`

On Windows, Kodi further replaces SMB paths with UNC paths automatically, so
`smb://server/media/Movies` will become `\\server\media\Movies` when it's
handed off to TinePlayer.

> [!NOTE]
> How you make the media reachable is up to you - a mapped drive, a mounted
> share, whatever your system already uses. TinePlayer opens it like any other
> file.

> [!IMPORTANT]
> You must resync or repair the Kodi database for path replacements to take
> effect.

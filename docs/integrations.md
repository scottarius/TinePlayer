# Integrations

TinePlayer can be launched by another application to play a video, handing
control back when playback ends. A media center does this to keep its own
library browsing while TinePlayer handles playback, but a launcher does not
have to be a library: a script, a file manager, or a keyboard shortcut works
the same way.

Launch the executable by passing the video and the `--external` flag:

### Linux

```sh
TinePlayer "/home/you/Videos/film.mkv" --external --fullscreen
```

### Windows

```powershell
launch-tineplayer-windows.cmd "H:\Videos\film.mkv" --external --fullscreen
```

On Windows, using `launch-tineplayer-windows.cmd` rather than the exe directly starts
TinePlayer from its own folder, avoiding any working directory issues with
loading conflicting libraries. See [the launch script
usage](usage.md#launch-script).

Passing `--external` puts TinePlayer into an integration mode, where it plays
only the video provided (file browser is disabled) and exits immediately after
the video finishes to return control to the launching application.

Additionally, the `--primary`, `--secondary` and `--subtitle` arguments can be
supplied to skip the menu entirely and go directly to playback. See [the
command line](usage.md#command-line).

Below are some supported integrations and how to set them up.

## Kodi

[Kodi](https://kodi.tv) is a media center application: it catalogs your films
and TV files with artwork and plays them on a television. It can hand playback
to TinePlayer rather than playing video itself, and TinePlayer will report
watch percentage back to Kodi.

There are two ways to set it up depending on your preference:

* **As a choice per video:** By default Kodi will play videos itself, and
  TinePlayer becomes an extra option under **Play using...** in a video's
  context menu.
* **As the default player:** Kodi opens every video in TinePlayer.

> [!IMPORTANT]
> Which Kodi version you run changes where the option appears:
>
> * On Kodi 21 and later, **Play using...** appears throughout.
> * On Kodi 20 and earlier, it only appears under **Videos → Files**, not in
>   the libraries.

### Automated Installation

The easiest way. These scripts write the configuration file for you:

```sh
./integrations/configure-kodi-linux.sh             # Linux
./integrations/configure-kodi-linux.sh --default   # Linux, as default player

.\integrations\configure-kodi-windows.ps1            # Windows
.\integrations\configure-kodi-windows.ps1 -Default   # Windows, as default player
```

They find Kodi's userdata directory themselves and write the correct
configuration file. Any existing `playercorefactory.xml` is backed up rather
than replaced.

Restart Kodi to see changes.

### Manual Installation

Kodi has no interface for this, so it means editing `playercorefactory.xml`
yourself. It lives in Kodi's userdata directory, and you can create it if it
isn't there already:

* Linux: `~/.kodi/userdata/playercorefactory.xml`
* Windows: `%APPDATA%\Kodi\userdata\playercorefactory.xml`

**[examples/playercorefactory.xml](../examples/playercorefactory.xml)** is a
complete, fully commented copy to start from. Two things required to change:

* Set `<filename>` to the TinePlayer executable path. On Windows, point it at
  `launch-tineplayer-windows.cmd` rather than straight at the executable.
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

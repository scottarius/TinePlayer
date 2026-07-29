# Integrations

TinePlayer can be launched by other applications, so you keep the benefits of
their library browsing with easy hand-off to TinePlayer for playback.

## Kodi

[Kodi](https://kodi.tv) is a media center application: it catalogues your films
and TV files with artwork and plays them on a television. It can hand playback to
TinePlayer rather than playing video itself.

There are two ways to set it up depending on your preference:

* **As a choice per video:** By default Kodi will play videos itself, and 
TinePlayer becomes an extra option under **Play using...** in a video's context menu.
* **As the default player:** Kodi opens every video in TinePlayer.

Caveat about Kodi versions: 
* On Kodi 21 and later, the **Play using...** option appears throughout.
* On Kodi 20 and earlier, the **Play using...** only appears under the **Videos → Files** section, not in the Libraries.

### Automated Installation

The easiest way. These scripts write the configuration file for you:

```sh
./install-kodi.sh             # Linux
./install-kodi.sh --default   # Linux, as default player

.\install-kodi.ps1            # Windows
.\install-kodi.ps1 -Default   # Windows, as default player
```

They find Kodi's userdata directory themselves and write the correct configuration file.
Any existing `playercorefactory.xml` is backed up rather than replaced.

Restart Kodi to see changes.

### Manual Installation

Kodi has no interface for this, so it means editing `playercorefactory.xml`
yourself. It lives in Kodi's userdata directory, and you can create it if it
isn't there already:

* Linux: `~/.kodi/userdata/playercorefactory.xml`
* Windows: `%APPDATA%\Kodi\userdata\playercorefactory.xml`

**[examples/playercorefactory.xml](../examples/playercorefactory.xml)** is a
complete, commented copy to start from. Two things to change:

* Set `<filename>` to the TinePlayer executable path. On Windows, point it at
  `launch-tineplayer.cmd` rather than straight at the executable.
* Uncomment the `<rules>` block at the bottom to make TinePlayer the default
  player. Left commented, TinePlayer appears under **Play Using...** instead.

Refer to the comments in the file for further explanation.

Restart Kodi to see changes.

### Media servers through Kodi

Kodi add-ons can bring a Jellyfin or Plex library into Kodi, and TinePlayer
plays those the same way it plays anything else — it asks Kodi what is playing
and never learns which server the video came from. Install each add-on by its
own instructions; only the settings below matter for TinePlayer.

**[Jellyfin for Kodi](https://jellyfin.org/docs/general/clients/kodi/)**

Works with either playback mode, and needs nothing special. What matters is
that Jellyfin **direct plays** rather than transcodes: a transcode collapses
the file to a single audio track, which leaves nothing to route to a second
output. Playing the original is the default for a client that can handle the
file, and TinePlayer can handle anything GStreamer can.

**[PlexKodiConnect](https://github.com/croneter/PlexKodiConnect/wiki)**

Set **playback mode to Direct Paths**. In the other mode PlexKodiConnect plays
videos itself, so Kodi never offers **Play Using...** and TinePlayer is never
reached.

Direct Paths hands over the path *the Plex server* uses. If Plex runs on
another machine, that path means nothing locally, and playback fails with a
"couldn't open" error naming a path you may not recognise. Fix it under
**Customize Paths**:

* Enable *Replace Plex paths with custom SMB paths*
* Set the original path to what Plex reports, e.g. `/mnt/media/Movies`
* Set the replacement to a share this machine can reach, e.g.
  `smb://server/media/Movies`

Those apply while syncing, so run **Repair local database** afterwards —
restarting Kodi alone leaves the old paths in place. On Windows, Kodi converts
`smb://` to a `\\server\share` path before handing it over.

Note that PlexKodiConnect rebuilds Kodi's video library and removes entries
other library add-ons created, so running it alongside Jellyfin for Kodi means
each resync clears the other's videos.

*Plex for Kodi* (`script.plex`) cannot work at all: it replaces Kodi's
interface instead of filling its library, so there is no library item to hand
over and no player to choose.

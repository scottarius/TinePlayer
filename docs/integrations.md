# Integrations

TinePlayer can be launched by other applications, so you keep the benefits of
their library browsing with easy handoff to TinePlayer for playback.

## Kodi

[Kodi](https://kodi.tv) is a media center application: it catalogs your films
and TV files with artwork and plays them on a television. It can hand playback
to TinePlayer rather than playing video itself.

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
./install-kodi.sh             # Linux
./install-kodi.sh --default   # Linux, as default player

.\install-kodi.ps1            # Windows
.\install-kodi.ps1 -Default   # Windows, as default player
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
complete, commented copy to start from. Two things to change:

* Set `<filename>` to the TinePlayer executable path. On Windows, point it at
  `launch-tineplayer.cmd` rather than straight at the executable.
* Uncomment the `<rules>` block at the bottom to make TinePlayer the default
  player. Left commented, TinePlayer appears under **Play using...** instead.

Refer to the comments in the file for further explanation.

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
  path on an accessible network share (SMB), or a direct link.

Tested Add-Ons:

* **[Jellyfin](#jellyfin)**
* **[Plex](#plex)**

## Jellyfin

[Jellyfin](https://jellyfin.org) is a free software media server. It catalogs
your films and TV and streams them to clients over the network.

### Native

Not yet supported, being investigated for a future release.

### Via Kodi Add-On

Follow the steps above to set up Kodi handoff to TinePlayer and then install
[Jellyfin for Kodi](https://jellyfin.org/docs/general/clients/kodi/) using its
own instructions.

> [!TIP]
> Use the default **Add-on mode** when installing, rather than Native. This
> mode streams from the server over HTTP instead of reading files off a share,
> so there is nothing to mount and no permissions to line up, and should work
> with a server outside your network.

## Plex

[Plex](https://www.plex.tv) is a media server with free and paid versions that
catalogs your films and TV and streams them to clients over the network.

### Native

Not planned. Plex clients do not currently provide support for external player
handoff.

### Via Kodi Add-On

Install [PlexKodiConnect](https://github.com/croneter/PlexKodiConnect/wiki) by
its own instructions, then set up Kodi handoff as above.

> [!IMPORTANT]
> Set **playback mode to Direct Paths**. In the other mode PlexKodiConnect
> plays videos itself, so Kodi never offers **Play using...** and TinePlayer is
> never reached.

Direct Paths hands over the path *the Plex server* uses. If Plex runs on
another machine, that path means nothing locally, and playback fails with a
"couldn't open" error naming a path you may not recognize. You can fix it in
the Add-On settings under **Customize Paths**:

* Enable *Replace Plex paths with custom SMB paths*
* Set the original path to what Plex reports, e.g. `/mnt/media/Movies`
* Set the replacement to a share this machine can reach, e.g.
  `smb://server/media/Movies`

Re-sync your Plex library or run **Repair local database** afterward to apply
the path replacements.

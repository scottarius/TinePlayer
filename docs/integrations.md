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

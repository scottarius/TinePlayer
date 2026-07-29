# Integrations

TinePlayer can be launched by other applications, so you keep the benefits of
their library browsing with easy hand-off to TinePlayer.

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

**Manual Installation**

Kodi has no interface for this, so it means editing `playercorefactory.xml` in Kodi's userdata directory — `~/.kodi/userdata/` on
Linux, `%APPDATA%\Kodi\userdata\` on Windows. If it doesn't exist already, you can create it yourself. 

Paste in the following snippit, replacing `<filename>` with the TinePlayer executable path.

```xml
<playercorefactory>
  <players>
    <player name="TinePlayer" type="ExternalPlayer" audio="false" video="true">
      <filename>/path/to/TinePlayer</filename>
      <args>"{1}" --fullscreen</args>
      <hidexbmc>true</hidexbmc>
      <hideconsole>true</hideconsole>
    </player>
  </players>
  <!-- Rules here -->
</playercorefactory>
```

This will add an option to the **Play Using...** menu.<br/> 
To force TinePlayer to act as the default player, insert the following under `<!-- Rules here -->`:

```xml
  <rules action="prepend">
    <rule video="true" player="TinePlayer" />
  </rules>
```

On Windows, you must point `<filename>` at `launch-tineplayer.cmd` rather than straight at the
executable. It will start the player from the right working directory so it can launch without conflicts.

**Automated Installation**

If you don't want to do the above manual installation, the following scripts will do it for you:

```sh
./install-kodi.sh             # Linux
./install-kodi.sh --default   # Linux, as default player

.\install-kodi.ps1            # Windows
.\install-kodi.ps1 -Default   # Windows, as default player
```

They find Kodi's userdata directory themselves and write the correct configuration file.
Any existing `playercorefactory.xml` is backed up rather than replaced.

Restart Kodi after either installation method.

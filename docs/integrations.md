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
/Applications/TinePlayer.app/Contents/MacOS/tineplayer "/Users/you/Movies/film.mkv" --external --fullscreen
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
* **Additional Player** - Kodi keeps playing videos itself, and TinePlayer
  appears under **Play using...** in a video's context menu.

> [!IMPORTANT]
> Which Kodi version you run changes where **Play using...** appears:
>
> * On Kodi 21 and later, throughout.
> * On Kodi 20 and earlier, only under **Videos → Files**, not in the
>   libraries.
>
> This affects **Additional Player** only. **Default Player** works the same on
> every version.

### From TinePlayer

The easiest way to configure Kodi is from TinePlayer itself. Open **Settings**
and choose **Kodi**. Every install of Kodi that was found has its own
group of settings, headed by its name and how it was installed - for example
**KODI 21.1 (DEFAULT INSTALLATION)** or **KODI 20.5 (FLATPAK)**.

Under each heading is which `playercorefactory.xml` that group will change, and
**Open File Location** to see it. That folder is also what tells two
installations apart.

Each group has these settings:

* **Configure As**

  **Default Player** or **Additional Player**, as above, or **Not configured**.
  Setting this is what registers TinePlayer with that Kodi.

  Once something is configured, this is also where it is removed: the same
  setting offers **Remove configuration**.

* **When Kodi Opens TinePlayer**

  **Show Track Selection Menu** opens TinePlayer's menu so the audio tracks and
  subtitles can be chosen for each video. **Play Video Immediately** starts the
  film right away, using the tracks remembered for that video or your language
  preferences.

If your Kodi is somewhere unusual and was not found - a portable install, for
example - **Add User Data Folder** opens a folder browser to point at it. It
then appears as a group of its own like any other.

> [!IMPORTANT]
> This asks for Kodi's **user data** folder, which is not the folder Kodi
> itself is installed in. It is the one holding `guisettings.xml`, listed under
> [Manual Installation](#manual-installation) below.
>
> Kodi creates that folder the first time it runs, so a Kodi that has been
> installed but never started does not have one yet and cannot be pointed at.
> Start Kodi once and close it, and TinePlayer will find it on its own without
> any of this.

The first time TinePlayer changes a given Kodi's configuration file, it asks
first, and names the file it will change and the backup it will keep. Changing
a setting after that is not asked about again, because by then it is
TinePlayer's own entry being edited. Removing a configuration always asks.

An existing `playercorefactory.xml` is edited in place rather than replaced.
Other players in it, and your own comments and formatting, are left exactly as
they are.

Kodi reads this file when it starts, so restart Kodi for any change to take
effect.

#### If Kodi is sandboxed

Kodi installed on Linux as a Flatpak starts an external player *inside its own sandbox*,
where TinePlayer is not installed and your files are not visible. TinePlayer
writes a command that steps out to the machine first, but Kodi does not ship
with permission to do that.

Such an install gets a third setting, **Sandbox Permission**, which shows the
one command to run:

```sh
flatpak override --user --talk-name=org.freedesktop.Flatpak tv.kodi.Kodi
```

> [!IMPORTANT]
> That permission lets Kodi run **any** program on your machine, not only
> TinePlayer. TinePlayer will never run it for you. To undo it:
> `flatpak override --user --reset tv.kodi.Kodi`, which clears every override
> you have set for Kodi.

If Kodi is installed as a **Snap** it is not supported as Snap confinement offers
no way to start a program outside itself. TinePlayer still lists such an
install, with its **Configure As** reading **Not supported** and saying why,
rather than leaving it out and having you wonder whether it was found. Use a
Kodi from your distribution's packages, or from Flathub.

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

TinePlayer connects to a Jellyfin server as a **cast target**. It does not
browse the library itself: once connected it appears as a player in the
Jellyfin app on a phone, tablet or browser, and whoever is holding that device
picks the video and presses cast. The television plays it, with TinePlayer's
own soundtrack and subtitle choices available as usual.

That split is deliberate. The person who needs the described soundtrack drives
playback from their own device, rather than asking for the remote.

Videos are always played as the original file, never transcoded, so every
soundtrack and subtitle in it survives. What TinePlayer plays back is what your
library holds.

#### Connecting

1. Open **Settings**, then **Jellyfin**.
2. Press **Server Address**. TinePlayer looks for servers on your network and
   lists whatever answers, by name. Choose yours.

   If nothing answers, press **Enter Address** and type it as you would into a
   browser: `http://jellyfin.local:8096`. If you leave out the `http://`, it is
   assumed. Typing it is also the way to reach a server on another network or
   behind a VPN, which cannot be found by looking.
3. Press **Connect**. A six character code appears.
4. In a Jellyfin app you are already signed in to, open **Quick Connect** from
   the user menu and enter that code.

The code is approved on a device you have already signed in to, so no password
is ever typed into TinePlayer. If your server's administrator has turned Quick
Connect off, TinePlayer says so rather than offering another way in.

Looking for servers sends one small UDP broadcast per network this machine is
on, to port 7359, and listens for two seconds. That is Jellyfin's own
discovery mechanism, the same one its apps use, and TinePlayer sends it only
while that panel is open. Some networks block broadcasts between clients, and
some firewalls will ask the first time: allowing it is only needed to find a
server by looking, never to play from one.

Once connected, TinePlayer appears as a player in the Jellyfin app whenever it
is running. Playback positions are reported back to Jellyfin as the film plays,
so a video started on the television can be resumed on a phone and the other
way round.

> [!IMPORTANT]
> Connecting stores an access token in `jellyfin.json`, in TinePlayer's user
> data folder. That token can read and stream the library as the account that
> approved the code, so treat that file as you would a password. It is stored
> as a plain file, readable only by your account where the system supports it:
> anything TinePlayer can read unattended, so can anything else running as you,
> and obfuscating it would only look like protection. A portable install keeps
> it on the drive it runs from.

#### Disconnecting

**Settings**, **Jellyfin**, then **Disconnect**. This removes the stored token
from this machine and signs the device out of the server.

It also asks the server to remove TinePlayer from your **Devices** list, which
some servers only allow an administrator to do. If that part fails, TinePlayer
says so: the token here is gone either way, and removing the device in the
Jellyfin dashboard revokes it at the server as well.

A pairing can also be ended from the Jellyfin side, by deleting the device or
the user. TinePlayer treats that as an ordinary thing to happen: it stops
appearing as a player, and the Jellyfin settings offer a new code.

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

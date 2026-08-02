# Building from source

Setup scripts are provided for each platform. Each installs only what is needed
to build, and each is idempotent, skipping anything already present, so they
are safe to re-run.

**Windows**

```powershell
.\setup-windows.ps1
cargo build --release
```

Installs Rust, the Visual Studio 2022 C++ build tools and GStreamer, then sets
the environment variables needed to build. Open a new terminal afterwards so
those take effect.

GStreamer's Windows distribution bundles GTK 4 and glib alongside GStreamer, so
it supplies every native dependency in one place.

> [!WARNING]
> **Don't install GTK separately** (with gvsbuild, for instance). A second GTK
> brings a second copy of glib, and mixing one library's headers with the
> other's build tools fails to build. Everything must be an MSVC build to match
> Rust's MSVC toolchain, because MSYS2/MinGW builds use a different ABI and
> will not link.

**macOS**

```sh
./setup-mac.sh
cargo build --release
```

Installs Homebrew, and the Xcode Command Line Tools along with it, then Rust,
GTK 4, GStreamer and pkg-config. Run it from a terminal rather than through a
pipe or an `ssh` command: Homebrew needs your password, and macOS will not
prompt for one without a terminal attached.

> [!NOTE]
> GTK and GStreamer both come from Homebrew on purpose. GStreamer's own macOS
> package ships no GTK, so taking it from there would mean getting GTK
> somewhere else, and each source brings its own copy of glib. Two of those in
> one process will not build. Installing both from Homebrew keeps them on a
> single copy.

**Linux**

```sh
./setup-linux.sh
cargo build --release
```

Installs the Rust toolchain, GTK 4 development headers, and the GStreamer
runtime, development headers and plugins (including those for common
Blu-ray-rip codecs like AC3 and DTS).

> [!NOTE]
> `setup-linux.sh` uses `apt`, so it works on Debian, Ubuntu, Raspberry Pi OS
> and their derivatives. On Fedora, Arch, openSUSE or anything else, install
> the equivalents by hand and then `cargo build --release` as usual. What is
> needed is:
>
> * Rust, from [rustup.rs](https://rustup.rs) or your distribution
> * `pkg-config`
> * GTK 4, with its development headers
> * GStreamer, with its development headers and the base, good, bad, ugly and
>   libav plugin sets
>
> Package names differ but the pieces do not. On Fedora those are roughly
> `gtk4-devel`, `gstreamer1-devel`, `gstreamer1-plugins-base-devel` and the
> matching plugin packages; on Arch, `gtk4`, `gstreamer`, `gst-plugins-base`
> and the rest of the `gst-plugins-*` set.
>
> There is no `.deb` for these distributions, so building from source is the
> way in. If you get it working and the list above was wrong or incomplete,
> [please say so](https://github.com/scottarius/TinePlayer/issues) and it will
> be corrected.

To start it from the desktop rather than a terminal, add a launcher entry for
the copy you have built:

```sh
./install-desktop-linux.sh
```

The entry points into this working tree, so run it again if the tree moves.

# Building from source

Setup scripts are provided for both platforms. Each is idempotent, skipping
anything already present, so they are safe to re-run.

**Linux**

```sh
./install.sh
cargo build --release
```

Installs the Rust toolchain, GTK 4 development headers, and the GStreamer
runtime, development headers and plugins (including those for common
Blu-ray-rip codecs like AC3 and DTS).

**Windows**

```powershell
.\install.ps1
cargo build --release
```

Installs Rust, the Visual Studio 2022 C++ build tools and GStreamer, then sets
the environment variables needed to build. Open a new terminal afterwards so
those take effect.

GStreamer's Windows distribution bundles GTK 4 and glib alongside GStreamer, so
it supplies every native dependency in one place. **Don't install GTK
separately** (with gvsbuild, for instance): a second GTK brings a second copy
of glib, and mixing one library's headers with the other's build tools fails to
build. Everything must be an MSVC build to match Rust's MSVC toolchain, because
MSYS2/MinGW builds use a different ABI and will not link.

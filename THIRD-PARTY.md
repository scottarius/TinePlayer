# Third-party notices

TinePlayer is built on open source work by other people. This file lists what
it depends on and under what terms, as the licenses of those works require.

It is generated from the dependency tree. To regenerate it after changing
dependencies:

    cargo metadata --format-version 1 | python packaging/third-party.py

## Native libraries

These are separate libraries TinePlayer loads at runtime rather than compiles
into itself.

| Library | License | Source |
|---------|---------|--------|
| GStreamer | LGPL-2.1-or-later | https://gitlab.freedesktop.org/gstreamer/gstreamer |
| GTK 4 | LGPL-2.1-or-later | https://gitlab.gnome.org/GNOME/gtk |
| GLib | LGPL-2.1-or-later | https://gitlab.gnome.org/GNOME/glib |
| FFmpeg | LGPL-2.1-or-later | https://ffmpeg.org |

Where they come from depends on how you got TinePlayer, and the difference
matters for what these terms oblige:

- **The Windows and macOS packages ship them**, alongside the executable or
  inside the application bundle, with their license texts. Distributing them
  is what brings the obligations below.
- **The Linux package ships none of them.** It is a `.deb` that declares them
  as dependencies, and apt installs your distribution's own copies under your
  distribution's terms. Depending on software is not distributing it, so
  nothing here is being redistributed on Linux.
- **A build from source** uses whatever is already installed on the machine.

Where they are shipped, they are used unmodified. Because they are loaded as
separate shared libraries, they can be replaced with your own build of the
same version, which is what the LGPL asks for.

> [!NOTE]
> Some GStreamer plugins carry different terms from GStreamer itself. The
> `a52dec` and `dtsdec` plugins, which decode AC-3 and DTS, are GPL-licensed,
> and TinePlayer ships neither: including them in a package would place the
> whole of it under the GPL.
>
> AC-3 and DTS soundtracks still play. On Windows and macOS they are decoded
> by FFmpeg, which is LGPL as long as it is built without its GPL components,
> and the FFmpeg in those packages is checked for exactly that. On Linux the
> decoder is whichever one your distribution installed, under whatever terms
> your distribution chose - which is its business rather than TinePlayer's,
> precisely because nothing is being redistributed.
>
> Patents are a separate question from copyright and vary by country. This is
> a statement about licenses, not about patents.

## Rust dependencies

Every crate compiled into TinePlayer, direct and transitive. `OR` means the
crate is offered under either license, at your choice.

| Crate | Version | License |
|-------|---------|---------|
| android_system_properties | 0.1.5 | MIT/Apache-2.0 |
| anstream | 1.0.0 | MIT OR Apache-2.0 |
| anstyle | 1.0.14 | MIT OR Apache-2.0 |
| anstyle-parse | 1.0.0 | MIT OR Apache-2.0 |
| anstyle-query | 1.1.5 | MIT OR Apache-2.0 |
| anstyle-wincon | 3.0.11 | MIT OR Apache-2.0 |
| async-channel | 2.5.0 | Apache-2.0 OR MIT |
| atomic_refcell | 0.1.14 | Apache-2.0 OR MIT |
| autocfg | 1.5.1 | Apache-2.0 OR MIT |
| bitflags | 2.13.1 | MIT OR Apache-2.0 |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 |
| cairo-rs | 0.20.12 | MIT |
| cairo-sys-rs | 0.20.10 | MIT |
| cc | 1.4.0 | MIT OR Apache-2.0 |
| cfg-expr | 0.20.8 | MIT OR Apache-2.0 |
| cfg-if | 1.0.4 | MIT OR Apache-2.0 |
| cfg_aliases | 0.2.2 | MIT |
| chrono | 0.4.45 | MIT OR Apache-2.0 |
| clap | 4.6.4 | MIT OR Apache-2.0 |
| clap_builder | 4.6.2 | MIT OR Apache-2.0 |
| clap_derive | 4.6.4 | MIT OR Apache-2.0 |
| clap_lex | 1.1.0 | MIT OR Apache-2.0 |
| colorchoice | 1.0.5 | MIT OR Apache-2.0 |
| concurrent-queue | 2.5.0 | Apache-2.0 OR MIT |
| core-foundation-sys | 0.8.7 | MIT OR Apache-2.0 |
| crossbeam-utils | 0.8.22 | MIT OR Apache-2.0 |
| either | 1.17.0 | MIT OR Apache-2.0 |
| equivalent | 1.0.2 | Apache-2.0 OR MIT |
| errno | 0.3.14 | MIT OR Apache-2.0 |
| event-listener | 5.4.2 | Apache-2.0 OR MIT |
| event-listener-strategy | 0.5.4 | Apache-2.0 OR MIT |
| field-offset | 0.3.6 | MIT OR Apache-2.0 |
| find-msvc-tools | 0.1.9 | MIT OR Apache-2.0 |
| fnv | 1.0.7 | Apache-2.0 / MIT |
| futures-channel | 0.3.33 | MIT OR Apache-2.0 |
| futures-core | 0.3.33 | MIT OR Apache-2.0 |
| futures-executor | 0.3.33 | MIT OR Apache-2.0 |
| futures-io | 0.3.33 | MIT OR Apache-2.0 |
| futures-macro | 0.3.33 | MIT OR Apache-2.0 |
| futures-task | 0.3.33 | MIT OR Apache-2.0 |
| futures-util | 0.3.33 | MIT OR Apache-2.0 |
| gdk-pixbuf | 0.20.10 | MIT |
| gdk-pixbuf-sys | 0.20.10 | MIT |
| gdk4 | 0.9.6 | MIT |
| gdk4-sys | 0.9.6 | MIT |
| gdk4-wayland | 0.9.6 | MIT |
| gdk4-wayland-sys | 0.9.6 | MIT |
| gdk4-win32 | 0.9.5 | MIT |
| gdk4-win32-sys | 0.9.5 | MIT |
| gdk4-x11 | 0.9.6 | MIT |
| gdk4-x11-sys | 0.9.6 | MIT |
| gilrs | 0.11.2 | Apache-2.0/MIT |
| gilrs-core | 0.6.8 | Apache-2.0/MIT |
| gio | 0.20.12 | MIT |
| gio-sys | 0.20.10 | MIT |
| glib | 0.20.12 | MIT |
| glib-macros | 0.20.12 | MIT |
| glib-sys | 0.20.10 | MIT |
| gobject-sys | 0.20.10 | MIT |
| graphene-rs | 0.20.10 | MIT |
| graphene-sys | 0.20.10 | MIT |
| gsk4 | 0.9.6 | MIT |
| gsk4-sys | 0.9.6 | MIT |
| gst-plugin-gtk4 | 0.13.7 | MPL-2.0 |
| gst-plugin-version-helper | 0.8.4 | MIT |
| gstreamer | 0.23.7 | MIT OR Apache-2.0 |
| gstreamer-audio | 0.23.6 | MIT OR Apache-2.0 |
| gstreamer-audio-sys | 0.23.6 | MIT |
| gstreamer-base | 0.23.6 | MIT OR Apache-2.0 |
| gstreamer-base-sys | 0.23.6 | MIT |
| gstreamer-gl | 0.23.7 | MIT OR Apache-2.0 |
| gstreamer-gl-egl | 0.23.6 | MIT OR Apache-2.0 |
| gstreamer-gl-egl-sys | 0.23.6 | MIT |
| gstreamer-gl-sys | 0.23.6 | MIT |
| gstreamer-gl-wayland | 0.23.5 | MIT OR Apache-2.0 |
| gstreamer-gl-wayland-sys | 0.23.5 | MIT |
| gstreamer-gl-x11 | 0.23.5 | MIT OR Apache-2.0 |
| gstreamer-gl-x11-sys | 0.23.5 | MIT |
| gstreamer-pbutils | 0.23.5 | MIT OR Apache-2.0 |
| gstreamer-pbutils-sys | 0.23.5 | MIT |
| gstreamer-sys | 0.23.6 | MIT |
| gstreamer-video | 0.23.6 | MIT OR Apache-2.0 |
| gstreamer-video-sys | 0.23.6 | MIT |
| gtk4 | 0.9.7 | MIT |
| gtk4-macros | 0.9.5 | MIT |
| gtk4-sys | 0.9.6 | MIT |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| heck | 0.5.0 | MIT OR Apache-2.0 |
| iana-time-zone | 0.1.65 | MIT OR Apache-2.0 |
| iana-time-zone-haiku | 0.1.2 | MIT OR Apache-2.0 |
| indexmap | 2.14.0 | Apache-2.0 OR MIT |
| inotify | 0.11.4 | ISC |
| inotify-sys | 0.1.8 | ISC |
| is_terminal_polyfill | 1.70.2 | MIT OR Apache-2.0 |
| itertools | 0.14.0 | MIT OR Apache-2.0 |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| js-sys | 0.3.103 | MIT OR Apache-2.0 |
| libc | 0.2.189 | MIT OR Apache-2.0 |
| libudev-sys | 0.1.4 | MIT |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| log | 0.4.33 | MIT OR Apache-2.0 |
| memchr | 2.8.3 | Unlicense OR MIT |
| memoffset | 0.9.1 | MIT |
| muldiv | 1.0.1 | MIT |
| nix | 0.31.3 | MIT |
| num-integer | 0.1.46 | MIT OR Apache-2.0 |
| num-rational | 0.4.2 | MIT OR Apache-2.0 |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| objc2-core-foundation | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-io-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| once_cell_polyfill | 1.70.2 | MIT OR Apache-2.0 |
| option-operations | 0.5.0 | MIT/Apache-2.0 |
| pango | 0.20.12 | MIT |
| pango-sys | 0.20.10 | MIT |
| parking | 2.2.1 | Apache-2.0 OR MIT |
| paste | 1.0.15 | MIT OR Apache-2.0 |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT |
| pkg-config | 0.3.33 | MIT OR Apache-2.0 |
| proc-macro-crate | 3.5.0 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 |
| rustix | 1.1.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 |
| semver | 1.0.28 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| serde_spanned | 1.1.1 | MIT OR Apache-2.0 |
| serde_yaml | 0.9.34+deprecated | MIT OR Apache-2.0 |
| shlex | 2.0.1 | MIT OR Apache-2.0 |
| slab | 0.4.12 | MIT |
| smallvec | 1.15.2 | MIT OR Apache-2.0 |
| strsim | 0.11.1 | MIT |
| syn | 2.0.119 | MIT OR Apache-2.0 |
| syn | 3.0.3 | MIT OR Apache-2.0 |
| system-deps | 7.0.8 | MIT OR Apache-2.0 |
| target-lexicon | 0.13.5 | Apache-2.0 WITH LLVM-exception |
| terminal_size | 0.4.4 | MIT OR Apache-2.0 |
| thiserror | 2.0.19 | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.19 | MIT OR Apache-2.0 |
| toml | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_datetime | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_edit | 0.25.13+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_parser | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_writer | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unsafe-libyaml | 0.2.11 | MIT |
| utf8parse | 0.2.2 | Apache-2.0 OR MIT |
| uuid | 1.24.0 | Apache-2.0 OR MIT |
| vec_map | 0.8.2 | MIT/Apache-2.0 |
| version-compare | 0.2.1 | MIT |
| version_check | 0.9.5 | MIT/Apache-2.0 |
| wasm-bindgen | 0.2.126 | MIT OR Apache-2.0 |
| wasm-bindgen-macro | 0.2.126 | MIT OR Apache-2.0 |
| wasm-bindgen-macro-support | 0.2.126 | MIT OR Apache-2.0 |
| wasm-bindgen-shared | 0.2.126 | MIT OR Apache-2.0 |
| web-sys | 0.3.103 | MIT OR Apache-2.0 |
| windows | 0.62.2 | MIT OR Apache-2.0 |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 |
| windows-core | 0.62.2 | MIT OR Apache-2.0 |
| windows-future | 0.3.2 | MIT OR Apache-2.0 |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 |
| windows-result | 0.4.1 | MIT OR Apache-2.0 |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 |
| windows-sys | 0.59.0 | MIT OR Apache-2.0 |
| windows-sys | 0.60.2 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 |
| windows-targets | 0.53.5 | MIT OR Apache-2.0 |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.53.1 | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.53.1 | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.53.1 | MIT OR Apache-2.0 |
| windows_i686_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnullvm | 0.53.1 | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.53.1 | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.53.1 | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.53.1 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.53.1 | MIT OR Apache-2.0 |
| winnow | 1.0.4 | MIT |
| winresource | 0.1.31 | MIT |
| zmij | 1.0.23 | MIT |

Full license texts are reproduced in the packaged builds, and are available in
each project's repository.

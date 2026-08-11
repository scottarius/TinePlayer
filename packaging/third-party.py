"""Regenerates THIRD-PARTY.md from the dependency tree.

The licenses of the crates TinePlayer is built on require their notices to
travel with it, and there are close to two hundred of them. Keeping the list
by hand would mean it was wrong within a release.

Run from the top of the source tree:

    cargo metadata --format-version 1 | python packaging/third-party.py

The prose above the crate table is kept here rather than in a separate file,
so that regenerating cannot quietly drop it.
"""

import json
import sys

PREAMBLE = """# Third-Party Notices

TinePlayer includes and depends on open source work by other people. This file
lists it and the terms it is provided under.

## Fonts

Included in every package:

| Font | License | Source |
|------|---------|--------|
| Noto Sans | OFL-1.1 | https://github.com/notofonts |
| Noto Sans Arabic, Armenian, Bengali, Devanagari, Georgian, Gurmukhi, Hebrew, Malayalam, Tamil, Telugu, Thai | OFL-1.1 | https://github.com/notofonts |
| Noto Sans TC, Noto Sans KR | OFL-1.1 | https://github.com/google/fonts |

They are subset to the characters TinePlayer draws and renamed to "TinePlayer
Sans", which the license requires of a modified copy. Its text is included
beside them.

## Native libraries

Loaded at runtime rather than compiled in.

| Library | License | Source |
|---------|---------|--------|
| GStreamer | LGPL-2.1-or-later | https://gitlab.freedesktop.org/gstreamer/gstreamer |
| GTK 4 | LGPL-2.1-or-later | https://gitlab.gnome.org/GNOME/gtk |
| GLib | LGPL-2.1-or-later | https://gitlab.gnome.org/GNOME/glib |
| FFmpeg | LGPL-2.1-or-later | https://ffmpeg.org |

- The Windows and macOS packages include them, with their license texts.
- The Linux package includes none of them. It declares them as dependencies,
  and apt installs your distribution's own copies under its terms.
- A build from source uses the copies already installed on the machine.

Where included they are unmodified, and being separate shared libraries they
can be replaced with your own build of the same version.

> [!NOTE]
> The GPL-licensed `a52dec` and `dtsdec` plugins are included in no package.
> AC-3 and DTS soundtracks still play: through FFmpeg on Windows and macOS,
> built without its GPL components, and through whichever decoder your
> distribution installed on Linux.
>
> This concerns licensing only. Patent status is a separate matter and varies
> by country.

## Rust dependencies

Every crate compiled into TinePlayer, direct and transitive. `OR` means the
crate is offered under either license, at your choice.

| Crate | Version | License |
|-------|---------|---------|"""

CLOSING = """
Full license texts are reproduced in the packaged builds, and are available in
each project's repository.
"""


def main() -> None:
    metadata = json.load(sys.stdin)
    rows = sorted(
        (
            (package["name"], package["version"], package.get("license") or "see repository")
            for package in metadata["packages"]
            if package["name"] != "tineplayer"
        ),
        key=lambda row: row[0].lower(),
    )

    lines = [PREAMBLE]
    lines += [f"| {name} | {version} | {license} |" for name, version, license in rows]
    lines.append(CLOSING)

    with open("THIRD-PARTY.md", "w", encoding="utf-8", newline="\n") as out:
        out.write("\n".join(lines))
    print(f"THIRD-PARTY.md: {len(rows)} crates")


if __name__ == "__main__":
    main()

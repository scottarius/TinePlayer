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

PREAMBLE = """# Third-party notices

TinePlayer is built on open source work by other people. This file lists what
it depends on and under what terms, as the licenses of those works require.

It is generated from the dependency tree. To regenerate it after changing
dependencies:

    cargo metadata --format-version 1 | python packaging/third-party.py

## Native libraries

These are separate libraries TinePlayer loads at runtime rather than compiles
into itself. Packaged builds ship them alongside the executable; a build from
source uses the copies already installed on the machine.

| Library | License | Source |
|---------|---------|--------|
| GStreamer | LGPL-2.1-or-later | https://gitlab.freedesktop.org/gstreamer/gstreamer |
| GTK 4 | LGPL-2.1-or-later | https://gitlab.gnome.org/GNOME/gtk |
| GLib | LGPL-2.1-or-later | https://gitlab.gnome.org/GNOME/glib |

They are used unmodified. Because they are loaded as separate shared
libraries, they can be replaced with your own build of the same version, which
is what the LGPL asks for. Their full license text ships with them.

> [!NOTE]
> Some GStreamer plugins carry different terms from GStreamer itself. The
> decoders for AC-3 and DTS in particular are GPL-licensed, and patent-
> encumbered in some countries. TinePlayer ships neither; it plays whatever
> the GStreamer installation on the machine provides.

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

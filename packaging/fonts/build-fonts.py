#!/usr/bin/env python3
"""Builds the fonts TinePlayer ships, so its own text looks the same everywhere.

Why bundle fonts at all: the language menu names every language in its own
script, and no desktop reliably has all of them. Measured 2026-08-03 - Windows
draws nothing for Bengali, Hindi, Malayalam, Punjabi, Tamil and Telugu;
Raspberry Pi OS has no Korean, Chinese or Telugu font at all; macOS is missing
most of the same set. Asking a package manager for a hundred megabytes of Noto
to draw six words is out of proportion, and it would still leave Windows and
macOS unfixed.

Why several files rather than one merged font: merging GSUB and GPOS tables
across scripts is where Indic shaping quietly breaks, and a mis-shaped
हिन्दी is worse than a missing one because nobody can see that it is wrong.
Each font keeps its own layout tables, and fontconfig picks between them by
coverage, which is the arrangement Noto is designed for.

Each font is cut down to only the characters TinePlayer actually draws, which
is why the whole set is a few hundred kilobytes rather than a few hundred
megabytes. The Latin font keeps whole ranges, because file names and device
names are not ours to predict; every other script keeps only the exact
characters in the language table.

Run it after changing the language table:

    python packaging/fonts/build-fonts.py

Needs fonttools:  pip install fonttools brotli
"""

import re
import sys
import unicodedata
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "data" / "fonts"
CACHE = Path(__file__).resolve().parent / ".sources"

NOTO = "https://raw.githubusercontent.com/notofonts/notofonts.github.io/main/fonts"

# CJK is not in the repository above - it is built separately, being two
# orders of magnitude larger than any other Noto face. Google Fonts carries a
# variable version, which subsets the same way. Only the Regular axis instance
# is taken, since the interface asks for one weight of these.
GOOGLE = "https://raw.githubusercontent.com/google/fonts/main/ofl"
ELSEWHERE = {
    "NotoSansTC": f"{GOOGLE}/notosanstc/NotoSansTC%5Bwght%5D.ttf",
    "NotoSansKR": f"{GOOGLE}/notosanskr/NotoSansKR%5Bwght%5D.ttf",
}

# The Latin font carries the interface itself, so it keeps whole ranges rather
# than a counted set: menu labels, file names, device names and folder paths
# are all drawn in it and none of them are known in advance.
#
#   Basic Latin, Latin-1, Latin Extended-A ... European languages
#   Latin Extended Additional ................ Vietnamese
#   General Punctuation ...................... quotes, dashes, the ellipsis
#   Currency, arrows, geometric shapes ....... the play triangle, chevrons
#   IPA Extensions ........................... the schwa in Azərbaycan
#   Greek and Cyrillic ....................... Ελληνικά, Русский, Български
#
# Greek and Cyrillic have to be listed even though `script_of` sends them to
# this font, and leaving them out is not a small mistake: the font then has no
# Cyrillic, each character falls back separately to whatever the machine
# offers, and the result is a line of letters with gaps between them. It
# looked fine on Windows, which has good Cyrillic to fall back to, and wrong
# on macOS, which does not.
LATIN_RANGES = (
    "U+0000-00FF,U+0100-017F,U+0180-024F,U+0250-02AF,U+0370-03FF,U+0400-04FF,"
    "U+1E00-1EFF,U+2000-206F,U+20A0-20BF,U+2190-21FF,U+25A0-25FF,U+2600-26FF"
)

# Which font covers which script. The name is the family fontconfig will see,
# and the file is what gets downloaded and cut down.
SCRIPTS = {
    "ARABIC": "NotoSansArabic",
    "ARMENIAN": "NotoSansArmenian",
    "BENGALI": "NotoSansBengali",
    "CJK": "NotoSansTC",
    "DEVANAGARI": "NotoSansDevanagari",
    "GEORGIAN": "NotoSansGeorgian",
    "GURMUKHI": "NotoSansGurmukhi",
    "HANGUL": "NotoSansKR",
    "HEBREW": "NotoSansHebrew",
    "MALAYALAM": "NotoSansMalayalam",
    "TAMIL": "NotoSansTamil",
    "TELUGU": "NotoSansTelugu",
    "THAI": "NotoSansThai",
}


def native_names():
    """Every native language name in the table, read from the source itself.

    Parsed rather than copied so the two cannot drift: a language added with a
    script nobody thought about would otherwise ship a font that cannot draw
    it, and the failure would be a row of boxes in front of a viewer.
    """
    source = (ROOT / "src" / "languages.rs").read_text(encoding="utf-8")
    table = re.findall(
        r'\(\s*"[a-z-]{2,8}"\s*,\s*"[^"]*"\s*,\s*"([^"]*)"', source
    )
    if not table:
        sys.exit("No language table found in src/languages.rs")
    return table


def script_of(character):
    """Which font a character belongs to, by Unicode block."""
    code = ord(character)
    if code < 0x02B0 or 0x1E00 <= code <= 0x1EFF or 0x2000 <= code <= 0x206F:
        return "LATIN"
    if 0x0370 <= code <= 0x03FF or 0x0400 <= code <= 0x04FF:
        return "LATIN"  # Greek and Cyrillic are in the Noto Sans core face
    for block, first, last in (
        ("HEBREW", 0x0590, 0x05FF),
        ("ARABIC", 0x0600, 0x06FF),
        ("DEVANAGARI", 0x0900, 0x097F),
        ("BENGALI", 0x0980, 0x09FF),
        ("GURMUKHI", 0x0A00, 0x0A7F),
        ("TAMIL", 0x0B80, 0x0BFF),
        ("TELUGU", 0x0C00, 0x0C7F),
        ("MALAYALAM", 0x0D00, 0x0D7F),
        ("THAI", 0x0E00, 0x0E7F),
        ("GEORGIAN", 0x10A0, 0x10FF),
        ("ARMENIAN", 0x0530, 0x058F),
        ("HANGUL", 0xAC00, 0xD7AF),
        ("CJK", 0x4E00, 0x9FFF),
    ):
        if first <= code <= last:
            return block
    return None


def fetch(family, weight):
    """The upstream font, kept between runs so a rebuild is not a download."""
    CACHE.mkdir(parents=True, exist_ok=True)
    name = f"{family}-{weight}.ttf"
    local = CACHE / name
    if local.exists():
        return local
    url = ELSEWHERE.get(family) or f"{NOTO}/{family}/hinted/ttf/{name}"
    print(f"  downloading {name}")
    try:
        with urllib.request.urlopen(url, timeout=120) as response:
            local.write_bytes(response.read())
    except Exception as e:
        sys.exit(f"Could not fetch {url}\n  {e}")
    return local


def subset(source, target, weight, unicodes=None, ranges=None):
    from fontTools import subset as ftsubset

    args = [
        str(source),
        f"--output-file={target}",
        # Every layout feature is kept. Indic scripts do their reordering and
        # conjunct forming in GSUB, so dropping "unused" features is how a
        # subset font ends up drawing the right letters in the wrong shapes.
        "--layout-features=*",
        "--glyph-names",
        "--notdef-outline",
        "--name-IDs=*",
        "--recalc-bounds",
    ]
    if ranges:
        args.append(f"--unicodes={ranges}")
    else:
        args.append("--unicodes=" + ",".join(f"U+{ord(c):04X}" for c in sorted(unicodes)))
    ftsubset.main(args)


def finish(path, family, weight):
    """Pins the weight and sets the family name.

    The name has to change: Noto is under the SIL Open Font License with
    Reserved Font Names, so a modified copy may not keep the name, and
    subsetting is a modification. It is written outright rather than
    substituted into the old one - replacing "Noto Sans" inside "Noto Sans
    Arabic" produced "TinePlayer Sans Arabic Arabic", which is the sort of
    thing nobody reads until a stylesheet asks for the name and misses.

    The weight has to be pinned because the CJK and Korean faces are variable
    fonts, and a variable font subset without instancing keeps its default
    instance - which for those two is Thin. They rendered noticeably lighter
    than everything beside them.
    """
    from fontTools.ttLib import TTFont
    from fontTools.varLib import instancer

    font = TTFont(path)
    if "fvar" in font:
        font = instancer.instantiateVariableFont(
            font, {"wght": 700 if weight == "Bold" else 400}, updateFontNames=False
        )

    postscript = family.replace(" ", "") + "-" + weight
    names = {
        1: family,
        2: weight,
        4: f"{family} {weight}",
        6: postscript,
        16: family,
        17: weight,
    }
    for record in font["name"].names:
        if record.nameID in names:
            record.string = names[record.nameID]
    font.save(path)
    font.close()


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    names = native_names()
    print(f"{len(names)} language names in the table")

    wanted = {}
    unknown = set()
    for name in names:
        for character in name:
            if character.isspace():
                continue
            script = script_of(character)
            if script is None:
                unknown.add(character)
            elif script != "LATIN":
                wanted.setdefault(script, set()).add(character)

    if unknown:
        detail = ", ".join(
            f"{c!r} U+{ord(c):04X} ({unicodedata.name(c, 'unnamed')})" for c in sorted(unknown)
        )
        sys.exit(
            "These characters belong to no font this script knows about:\n  "
            + detail
            + "\nAdd the script to SCRIPTS and script_of() before shipping."
        )

    built = []
    for weight in ("Regular", "Bold"):
        source = fetch("NotoSans", weight)
        target = OUT / f"TinePlayerSans-{weight}.ttf"
        print(f"  {target.name}: full Latin, Greek, Cyrillic and punctuation")
        subset(source, target, weight, ranges=LATIN_RANGES)
        finish(target, "TinePlayer Sans", weight)
        built.append(target)

    for script, characters in sorted(wanted.items()):
        family = SCRIPTS[script]
        source = fetch(family, "Regular")
        target = OUT / f"TinePlayerSans{script.title()}-Regular.ttf"
        text = "".join(sorted(characters))
        print(f"  {target.name}: {len(characters)} characters  {text}")
        subset(source, target, "Regular", unicodes=characters)
        finish(target, f"TinePlayer Sans {script.title()}", "Regular")
        built.append(target)

    total = sum(f.stat().st_size for f in built)
    print(f"\n{len(built)} fonts, {total / 1024:.0f} KB total, in {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()

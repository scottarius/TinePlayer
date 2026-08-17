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

Most of these ship whole. Three do not - see SUBSET_ONLY, which carries the
measurements and the decision. The short version is that cutting a font to a
counted set only works when the text is known in advance, and it is not: the
interface draws film titles and plots out of `.nfo` files and out of a Jellyfin
library, in whatever script the film is from. The whole non-CJK set is 2.4 MB.
CJK and Korean whole would be another 31 MB, so those two stay cut down to the
language menu plus whatever the translation catalogs use, and CJK metadata
falls back to the system font.

Run it after changing the language table, or after a translation lands in a
script that is still cut down:

    python packaging/fonts/build-fonts.py

Needs fonttools:  pip install fonttools brotli
"""

import re
import shutil
import sys
import unicodedata
import urllib.request
from pathlib import Path

# This script prints the characters it is subsetting to, which are by
# definition not Latin - and a Windows console is cp1252, where writing 日本語
# raises UnicodeEncodeError and kills the run partway through, leaving half the
# fonts rebuilt. Errors are replaced rather than raised: a console that cannot
# draw a character is a fact about the console, not a reason to stop.
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

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
# Kept for reference rather than used. The Latin face now ships whole - see
# SUBSET_ONLY below - and this is what it was cut to for the first year, in
# case a reason to go back to cutting it ever appears.
LATIN_RANGES = (
    "U+0000-00FF,U+0100-017F,U+0180-024F,U+0250-02AF,U+0370-03FF,U+0400-04FF,"
    "U+1E00-1EFF,U+2000-206F,U+20A0-20BF,U+2190-21FF,U+25A0-25FF,U+2600-26FF"
)

# Marks the interface draws itself, which no language name contains and so
# nothing below would otherwise ask for.
#
# They are listed here because asking for them in LATIN_RANGES does not work
# and silently looks as though it did. Those ranges do include the arrows,
# geometric shapes and symbols blocks - and the comment above has always
# claimed the play triangle among them - but Noto Sans carries none of those
# characters, so subsetting to a range the source does not cover produces
# nothing at all and says nothing about it. Checked 2026-08-10: the star, the
# play triangle and the reload arrow were all absent from the shipped fonts
# and had been drawn by whatever each machine happened to have.
#
# Noto keeps them in a separate face, which is why this needs its own entry
# rather than a wider range on the Latin one.
#   ★  U+2605  BLACK STAR, beside a rating
#
# Worth knowing before adding to this: the play triangle U+25B6 and the reload
# arrow U+21BB on the buttons are *not* here, and so are still drawn by
# whatever each machine happens to have - DejaVu on the Pi, Segoe on Windows.
# U+25B6 is available in this same face if that is ever wanted; U+21BB is in
# no Noto face at all, and U+2B6E is the nearest shipped equivalent.
INTERFACE_SYMBOLS = "★"

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
    # Not a script: the interface's own marks, which Noto keeps apart from the
    # text faces. Symbols 2 rather than Symbols - the star is in the second.
    "SYMBOLS": "NotoSansSymbols2",
    "TAMIL": "NotoSansTamil",
    "TELUGU": "NotoSansTelugu",
    "THAI": "NotoSansThai",
}

# The three faces that are still cut down. Everything else ships whole.
#
# WHY WHOLE, since this file spent a year arguing the opposite. Cutting a font
# to a counted set only works when the text is known in advance, and it is not.
# The interface draws film titles, plots and genres out of `.nfo` files and out
# of a Jellyfin library - arbitrary text in whatever script the film is from -
# and on top of that the interface itself is now translated. Neither is
# predictable, so the counted set was answering a question nobody was asking.
# Measured 2026-08-17: the whole non-CJK set weighs 2.4 MB against 655 KB cut
# down, which is 6% of a 31 MB installer for a whole class of problem.
#
# WHY THESE THREE ARE STILL CUT. The CJK and Korean faces are 11 MB and 10 MB
# whole - bundling them would roughly double every package on every platform,
# to fix a gap that only Linux has, since Windows and macOS both ship CJK
# already. They are kept at exactly the characters the language menu names
# itself with - 日本語, 한국어, 中文, 粵語 - so that menu is right everywhere,
# and CJK text from anywhere else falls back to the system font. Decided
# 2026-08-17, deliberately and knowing what it costs: a Linux machine with no
# CJK font shows boxes for a Japanese film title, and the alternative was 31 MB
# on every download for everybody.
#
# Symbols is here for a different reason: the interface wants one star out of a
# 656 KB face, and there is no metadata that draws from it.
SUBSET_ONLY = {"CJK", "HANGUL", "SYMBOLS"}


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


def translated_text():
    """Every translated string in po/, as one blob to take characters from.

    THE LANGUAGE TABLE IS NOT ENOUGH, and assuming it was is how this went
    wrong. Those names cost a handful of characters per script - enough to
    draw the word for the language and nothing else - so the Japanese font
    shipped with six characters in it and the Arabic one with twelve. That is
    exactly right for a menu of language names and useless for an interface
    translated into either: every other word would have drawn as a box.

    Every `.po` in the folder, not only the ones in LINGUAS. A catalog that
    does not ship yet is one somebody is working on with TINEPLAYER_PO, which
    is the moment they most need to see their own language rather than boxes.

    Read as raw text rather than parsed. This wants the characters, and a
    msgid, a msgstr, a translator's name and a comment are all equally good
    sources of them - over-inclusion here costs a few glyphs and prevents the
    failure that matters.
    """
    catalogs = sorted((ROOT / "po").glob("*.po"))
    if not catalogs:
        print("  no catalogs in po/ - fonts will cover the language table only")
        return ""
    print(f"  {len(catalogs)} catalog(s): {', '.join(c.stem for c in catalogs)}")
    return "".join(c.read_text(encoding="utf-8") for c in catalogs)


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


def needed():
    """Every character the interface can be asked to draw, sorted by script.

    Two sources, and both matter: the language table, which names each language
    in its own script, and the translation catalogs, which are the interface
    itself in somebody else's language.

    The two are kept apart, because what is promised of each differs. The
    language menu has to be right on every platform, so its characters are
    required of every bundled face including the three that are still cut down.
    Translated text is required of the faces that ship whole, and for CJK it is
    knowingly left to the system font - see SUBSET_ONLY.

    Returns `(menu, translated, unknown)`, the first two as `{script: chars}`
    with Latin under "LATIN".
    """
    names = native_names()
    print(f"{len(names)} language names in the table")
    catalogs = translated_text()

    unknown = set()

    def sort(text, into):
        for character in text:
            if character.isspace():
                continue
            script = script_of(character)
            if script is None:
                unknown.add(character)
            else:
                into.setdefault(script, set()).add(character)

    # Seeded rather than discovered, being the one set of characters that comes
    # from the interface itself rather than from either source below.
    menu = {"SYMBOLS": set(INTERFACE_SYMBOLS)}
    sort("".join(names), menu)

    translated = {}
    sort(catalogs, translated)

    return menu, translated, unknown


def check():
    """Whether the fonts in data/fonts can draw everything the interface has.

    Reads only what is already in the tree, so it needs no network and can run
    in CI - which is the point. Rebuilding the fonts is a manual step, and the
    failure it guards against is quiet: a translation is added or extended, the
    fonts are not rebuilt, and that language ships as a screen of boxes for
    everybody except the person who tested it on a machine that happened to
    have the font installed anyway.
    """
    from fontTools.ttLib import TTFont

    menu, translated, unknown = needed()
    if unknown:
        report_unknown(unknown)

    def coverage(path):
        if not path.exists():
            return None
        font = TTFont(path, lazy=True)
        found = set()
        for table in font["cmap"].tables:
            found |= set(table.cmap.keys())
        font.close()
        return found

    def file_for(script):
        if script == "LATIN":
            return "TinePlayerSans-Regular.ttf"
        return f"TinePlayerSans{script.title()}-Regular.ttf"

    # Every script is checked the same way, cut down or not. The cut-down faces
    # are cut to exactly this - the language menu plus whatever the catalogs
    # use - so anything missing here means the fonts have not been rebuilt
    # since the text changed, which a rebuild fixes.
    #
    # What this cannot check is metadata: a film's title arrives from a library
    # at run time and is not in this repository to be counted. That is the gap
    # the whole-face bundling closes for every script except CJK, and the one
    # SUBSET_ONLY knowingly leaves to the system font.
    problems = []
    for script in sorted(set(menu) | set(translated)):
        name = file_for(script)
        covered = coverage(OUT / name)
        if covered is None:
            problems.append(f"{name} is missing")
            continue

        characters = menu.get(script, set()) | translated.get(script, set())
        missing = {c for c in characters if ord(c) not in covered}
        if missing:
            problems.append(
                f"{name} cannot draw {len(missing)} of {len(characters)}: "
                + " ".join(sorted(missing))
            )

    if problems:
        print("\nThe bundled fonts are out of date:\n", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        print(
            "\nRebuild them:  python packaging/fonts/build-fonts.py"
            "\n(needs network and `pip install fonttools brotli`)",
            file=sys.stderr,
        )
        return 1

    print("\nThe bundled fonts cover every character the interface can draw.")
    return 0


def report_unknown(unknown):
    detail = ", ".join(
        f"{c!r} U+{ord(c):04X} ({unicodedata.name(c, 'unnamed')})"
        for c in sorted(unknown)
    )
    sys.exit(
        "These characters belong to no font this script knows about:\n  "
        + detail
        + "\nAdd the script to SCRIPTS and script_of() before shipping."
    )


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    menu, translated, unknown = needed()

    if unknown:
        report_unknown(unknown)

    # What the cut-down faces are cut to. Both sources, so a CJK translation
    # that somebody has begun is carried as far as it can be - the face is
    # already being built, and the characters cost bytes rather than megabytes.
    wanted = {
        script: characters | translated.get(script, set())
        for script, characters in menu.items()
    }

    built = []
    for weight in ("Regular", "Bold"):
        source = fetch("NotoSans", weight)
        target = OUT / f"TinePlayerSans-{weight}.ttf"
        print(f"  {target.name}: whole - Latin, Greek and Cyrillic")
        shutil.copyfile(source, target)
        finish(target, "TinePlayer Sans", weight)
        built.append(target)

    # Every script the interface can meet, not only the ones some character
    # currently asks for. A film's title arrives from a library rather than
    # from this repository, so a face is worth shipping whether or not anything
    # here happens to name a language written in it.
    for script in sorted(SCRIPTS):
        family = SCRIPTS[script]
        source = fetch(family, "Regular")
        target = OUT / f"TinePlayerSans{script.title()}-Regular.ttf"

        if script in SUBSET_ONLY:
            characters = wanted.get(script, set())
            if not characters:
                continue
            text = "".join(sorted(characters))
            print(f"  {target.name}: {len(characters)} characters  {text}")
            subset(source, target, "Regular", unicodes=characters)
        else:
            print(f"  {target.name}: whole")
            shutil.copyfile(source, target)

        finish(target, f"TinePlayer Sans {script.title()}", "Regular")
        built.append(target)

    total = sum(f.stat().st_size for f in built)
    print(f"\n{len(built)} fonts, {total / 1024:.0f} KB total, in {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    if "--check" in sys.argv:
        sys.exit(check())
    main()

#!/usr/bin/env python3
"""Builds po/tineplayer.pot from the tr!, trc! and trn! calls in src/.

Run it after adding or changing any interface string:

    python3 packaging/extract-strings.py

Weblate takes it from there. It merges a new template into every existing
catalog by itself, so nothing has to be run against the .po files by hand -
which matters, because the alternative is msgmerge and that means asking every
contributor on Windows to install gettext.

WHY NOT XGETTEXT
    It has no Rust mode. Running it with --language=C mostly works and then
    quietly does the wrong thing with raw strings and with the r#"..."#
    literals this codebase uses for Windows paths. Since the macros are ours
    and there are exactly three of them, reading them directly is both shorter
    and honest about what it does and does not handle.

WHAT IT DELIBERATELY DOES NOT DO
    It does not follow variables. `tr!(name)` is not extractable and is
    refused rather than skipped, because a string that silently never reaches a
    translator is the failure this whole arrangement exists to prevent.

    It does not read the CLI help in main.rs. Those are clap's `///` doc
    comments, which are not string literals at all - see the note in
    src/i18n.rs about why the command line is not translated.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE = ROOT / "src"
TEMPLATE = ROOT / "po" / "tineplayer.pot"

# `tr!(`, `trc!(`, `trn!(` with any leading path qualification. The opening
# paren is matched here and the rest is scanned by hand, because a Rust string
# can contain a paren and a regex cannot count brackets.
CALL = re.compile(r"\b(?:crate::)?(tr|trc|trn)!\s*\(")


class Refusal(Exception):
    """A call that cannot be extracted, which is a mistake to be fixed."""


def blank_comments(text: str) -> str:
    """The same text with every comment replaced by spaces.

    Length and line breaks are preserved so that offsets and line numbers
    still refer to the real file. This exists because src/i18n.rs documents
    the macros by example, and a `tr!` inside a doc comment is not a string
    anybody should be asked to translate - the first run of this script
    cheerfully extracted three of them.
    """
    out = list(text)
    at = 0
    depth = 0  # nested /* */, which Rust allows

    def blank(start: int, end: int) -> None:
        for index in range(start, end):
            if out[index] != "\n":
                out[index] = " "

    while at < len(text):
        if depth:
            if text.startswith("/*", at):
                depth += 1
                blank(at, at + 2)
                at += 2
            elif text.startswith("*/", at):
                depth -= 1
                blank(at, at + 2)
                at += 2
            else:
                blank(at, at + 1)
                at += 1
            continue

        if text.startswith("//", at):
            end = text.find("\n", at)
            end = len(text) if end == -1 else end
            blank(at, end)
            at = end
            continue

        if text.startswith("/*", at):
            depth = 1
            blank(at, at + 2)
            at += 2
            continue

        # Raw strings, which have no escapes and may contain anything.
        raw = re.match(r'r(#*)"', text[at:])
        if raw:
            closing = '"' + "#" * len(raw.group(1))
            end = text.find(closing, at + len(raw.group(0)))
            at = len(text) if end == -1 else end + len(closing)
            continue

        if text[at] == '"':
            at += 1
            while at < len(text) and text[at] != '"':
                at += 2 if text[at] == "\\" else 1
            at += 1
            continue

        # A char literal may hold a quote - '"' - and must not be mistaken for
        # the start of a string. A lifetime such as 'static looks similar and
        # is not a literal at all, so only the real shapes are consumed.
        if text[at] == "'":
            literal = re.match(r"'(\\.|[^\\'])'", text[at:])
            at += len(literal.group(0)) if literal else 1
            continue

        at += 1

    return "".join(out)


def parse_string(text: str, at: int) -> tuple[str, int]:
    """Reads one Rust string literal starting at `at`. Returns it and the
    index just past its closing quote."""
    if text[at] == "r":
        # A raw string: r"...", r#"..."#, r##"..."##.
        hashes = 0
        cursor = at + 1
        while text[cursor] == "#":
            hashes += 1
            cursor += 1
        if text[cursor] != '"':
            raise Refusal("a raw string that is not a string")
        closing = '"' + "#" * hashes
        end = text.index(closing, cursor + 1)
        return text[cursor + 1 : end], end + len(closing)

    if text[at] != '"':
        raise Refusal("an argument that is not a literal string")

    out = []
    cursor = at + 1
    while True:
        character = text[cursor]
        if character == "\\":
            following = text[cursor + 1]
            # A backslash at the end of a line is Rust's line continuation: the
            # newline AND the next line's indentation are both dropped, and the
            # string is joined with nothing between. Getting this wrong is
            # invisible - the msgid comes out carrying a literal backslash,
            # newline and seventeen spaces, so it never matches the string the
            # compiler actually built and a translation of it silently never
            # appears. Two of the Kodi sandbox messages were exactly that.
            if following in "\r\n":
                cursor += 1
                while cursor < len(text) and text[cursor] in " \t\r\n":
                    cursor += 1
                continue
            out.append(
                {"n": "\n", "t": "\t", "r": "\r", '"': '"', "\\": "\\"}.get(
                    following, "\\" + following
                )
            )
            cursor += 2
            continue
        if character == '"':
            return "".join(out), cursor + 1
        out.append(character)
        cursor += 1


def skip_space(text: str, at: int) -> int:
    """Past whitespace, // line comments and any trailing newline."""
    while at < len(text):
        if text[at].isspace():
            at += 1
            continue
        if text.startswith("//", at):
            end = text.find("\n", at)
            at = len(text) if end == -1 else end + 1
            continue
        return at
    return at


def arguments(text: str, at: int) -> tuple[list[str], int]:
    """The leading string literals of a macro call, stopping at the first
    argument that is not one - which is where the interpolated values start."""
    found: list[str] = []
    cursor = at
    while True:
        cursor = skip_space(text, cursor)
        if cursor >= len(text) or text[cursor] == ")":
            return found, cursor
        if text[cursor] not in ('"', "r"):
            return found, cursor
        # `r` might begin a raw string or an identifier such as `role`.
        if text[cursor] == "r" and not re.match(r'r#*"', text[cursor:]):
            return found, cursor
        literal, cursor = parse_string(text, cursor)
        found.append(literal)
        cursor = skip_space(text, cursor)
        if cursor < len(text) and text[cursor] == ",":
            cursor += 1


def line_of(text: str, at: int) -> int:
    return text.count("\n", 0, at) + 1


def extract() -> dict[tuple[str | None, str], dict]:
    """Every message in the source, keyed by (context, msgid)."""
    messages: dict[tuple[str | None, str], dict] = {}
    problems: list[str] = []

    for path in sorted(SOURCE.glob("*.rs")):
        text = blank_comments(path.read_text(encoding="utf-8"))
        for call in CALL.finditer(text):
            macro = call.group(1)
            where = f"src/{path.name}:{line_of(text, call.start())}"
            try:
                found, _ = arguments(text, call.end())
            except (Refusal, IndexError, ValueError) as e:
                problems.append(f"{where}: {macro}! could not be read ({e})")
                continue

            wanted = {"tr": 1, "trc": 2, "trn": 2}[macro]
            if len(found) < wanted:
                problems.append(
                    f"{where}: {macro}! needs {wanted} literal string(s), found "
                    f"{len(found)}. A message built from a variable never "
                    f"reaches a translator."
                )
                continue

            # `fill` matches `{name}` exactly and knows nothing of Rust's
            # format specifiers, so `{ms:.0}` never substitutes and reaches the
            # screen written out. It compiles, it extracts, and it is only
            # visible by looking at the running interface - which is exactly
            # the kind of fault worth refusing here instead.
            for message in found:
                bad = re.findall(r"\{([a-z_][a-z_0-9]*)[:!][^}]*\}", message)
                if bad:
                    problems.append(
                        f"{where}: {macro}! has a format specifier in "
                        f"{{{bad[0]}:...}}. Format the value before passing it "
                        f"- placeholders are substituted by name and nothing else."
                    )

            if macro == "tr":
                key = (None, found[0])
                entry = messages.setdefault(key, {"plural": None, "places": []})
            elif macro == "trc":
                key = (found[0], found[1])
                entry = messages.setdefault(key, {"plural": None, "places": []})
            else:
                key = (None, found[0])
                entry = messages.setdefault(key, {"plural": found[1], "places": []})
                entry["plural"] = found[1]

            entry["places"].append(where)

    if problems:
        for problem in problems:
            print(f"error: {problem}", file=sys.stderr)
        raise SystemExit(1)

    return messages


def quote(text: str) -> str:
    """A msgid as a .po writes one: escaped, and split at newlines so a long
    message is readable in a diff and in Weblate."""
    escaped = (
        text.replace("\\", "\\\\").replace('"', '\\"').replace("\t", "\\t")
    )
    lines = escaped.split("\n")
    if len(lines) == 1:
        return f'"{lines[0]}"'
    # gettext's own convention: an empty first line, then one line per part
    # with the newline written back in.
    out = ['""']
    for line in lines[:-1]:
        out.append(f'"{line}\\n"')
    if lines[-1]:
        out.append(f'"{lines[-1]}"')
    return "\n".join(out)


def similar(text: str) -> str:
    """A message flattened to what it would mean to a translator.

    Case, trailing punctuation, doubled spaces and the *names* of holes are all
    things two strings can differ in while saying the same thing - and a
    translator handed both translates the same sentence twice, after which the
    two can drift apart in the interface with nothing to catch it.
    """
    flattened = re.sub(r"\s+", " ", text).strip()
    # An all-capitals message is a group heading - INTERFACE, FIRST OUTPUT,
    # SUBTITLES - and that is a deliberate style rather than an accident of
    # capitalization. Folding its case would report every heading against any
    # ordinary label using the same word, which is three false positives and
    # no true ones.
    if flattened != flattened.upper():
        flattened = flattened.lower()
    flattened = re.sub(r"\{[a-z_0-9]+\}", "{}", flattened, flags=re.I)
    # A trailing full stop or colon is punctuation. A question mark is not:
    # "Close the Player?" is a heading asking something and "Close the player"
    # is a button that does it, and they are two messages however alike they
    # look. Stripping `?` reported that pair as worth consolidating, which
    # would have been wrong in both languages.
    return flattened.rstrip(".:…")


def report_duplicates(messages: dict) -> None:
    """Prints messages that are near enough to be worth consolidating.

    NOT done automatically, and not a failure. Whether two similar strings
    should be one is a judgment about the interface - "Choose a video" and
    "Choose a video file" may be two rows that should agree, or a row and a
    dialog title that should not. What the script can do is make sure nobody
    has to notice them by reading.

    Worth acting on BEFORE a catalog reaches Weblate. A msgid is its own key,
    so changing one afterwards orphans every translation of it: the string
    comes back untranslated in every language, and somebody has to do the work
    again. Consolidating is free today and expensive next month.
    """
    groups: dict[str, list] = {}
    for (context, msgid), entry in messages.items():
        groups.setdefault(similar(msgid), []).append((context, msgid, entry))

    families = [g for g in groups.values() if len({m for _, m, _ in g}) > 1]
    if not families:
        return

    print(f"\n{len(families)} group(s) worth looking at for consolidation:\n")
    for family in sorted(families, key=lambda g: g[0][1]):
        for context, msgid, entry in sorted(family, key=lambda m: m[1]):
            marker = f" [context: {context}]" if context else ""
            print(f'  "{msgid}"{marker}')
            print(f"      {entry['places'][0]}")
        print()


def main() -> None:
    messages = extract()

    # No POT-Creation-Date. gettext's own tools write one, and in a git
    # repository it is pure churn: every regeneration would rewrite the file
    # whether or not a single string changed, which makes "is the template up
    # to date?" unanswerable by `git diff` - and that check is what CI runs.
    # The commit date says when it was generated, more accurately.
    header = """# Translations for TinePlayer.
#
# Generated by packaging/extract-strings.py - do not edit this file by hand.
# Translations live in the .po files beside it; this is only the template
# they are merged from.
#
# See docs/translating.md to start a new language.
#
msgid ""
msgstr ""
"Project-Id-Version: TinePlayer\\n"
"Report-Msgid-Bugs-To: https://github.com/scottarius/TinePlayer/issues\\n"
"MIME-Version: 1.0\\n"
"Content-Type: text/plain; charset=UTF-8\\n"
"Content-Transfer-Encoding: 8bit\\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\\n"
"""

    body = []
    # Sorted by where they appear rather than alphabetically, so a translator
    # working down the file meets a screen's worth of strings together.
    for (context, msgid), entry in sorted(
        messages.items(), key=lambda item: (item[1]["places"][0], item[0][1])
    ):
        block = [f"#: {' '.join(entry['places'])}"]
        if context is not None:
            block.append(f"msgctxt {quote(context)}")
        block.append(f"msgid {quote(msgid)}")
        if entry["plural"] is not None:
            block.append(f"msgid_plural {quote(entry['plural'])}")
            block.append('msgstr[0] ""')
            block.append('msgstr[1] ""')
        else:
            block.append('msgstr ""')
        body.append("\n".join(block))

    TEMPLATE.parent.mkdir(exist_ok=True)
    # newline="" so Python does not translate to the platform's endings on the
    # way out. Without it this writes CRLF on Windows and LF on Linux, and CI
    # asks whether the template is up to date by running it and looking for a
    # diff - a question that cannot be answered if the answer depends on who
    # ran it last. `.gitattributes` pins the checked-out file to match.
    with open(TEMPLATE, "w", encoding="utf-8", newline="") as out:
        out.write(header + "\n" + "\n\n".join(body) + "\n")

    plurals = sum(1 for entry in messages.values() if entry["plural"])
    contexts = sum(1 for key in messages if key[0] is not None)
    print(
        f"{TEMPLATE.relative_to(ROOT)}: {len(messages)} messages "
        f"({plurals} with plural forms, {contexts} with a context)"
    )
    report_duplicates(messages)


if __name__ == "__main__":
    main()

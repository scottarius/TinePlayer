#!/usr/bin/env python3
"""Builds po/tineplayer.pot from the tr!, trc! and trn! calls in src/.

Run it after adding or changing any interface string:

    python3 packaging/extract-strings.py

It also merges that template into every po/*.po, which is the job a
translation platform would otherwise do. There is no such platform here -
hosting one is a paid service or a server to run, and neither is worth
committing to before anybody is translating - so the merge is done in this
script. That also avoids msgmerge, which would mean asking every contributor
on Windows to install gettext.

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
import textwrap

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


def translator_notes(text: str) -> dict[int, str]:
    """`// TRANSLATORS:` comments, by the line of the call they precede.

    gettext's own convention, and the one thing in a catalog that can carry
    context a translator cannot get from the string. Every PO editor shows
    these beside the message.

    Worth having for one kind of string in particular: another project's own
    words. "Quick Connect" is Jellyfin's name for a feature, and a translator
    working from English alone would translate it afresh - sending somebody to
    look for a menu item that does not exist under that name in their Jellyfin.
    The right answer is whatever Jellyfin's own translation says, and only a
    note can ask for that.

    Runs of `// TRANSLATORS:` lines are joined, so a long note can be wrapped
    the way the rest of the source is. The note attaches to the next line that
    is not itself a comment.
    """
    notes: dict[int, str] = {}
    collected: list[str] = []
    for number, line in enumerate(text.split("\n"), 1):
        stripped = line.strip()
        marker = re.match(r"//+\s*TRANSLATORS:\s*(.*)", stripped)
        if marker:
            collected = [marker.group(1).strip()]
            continue
        if collected and stripped.startswith("//"):
            collected.append(stripped.lstrip("/").strip())
            continue
        if collected:
            notes[number] = " ".join(part for part in collected if part)
            collected = []
    return notes


def line_of(text: str, at: int) -> int:
    return text.count("\n", 0, at) + 1


def extract() -> dict[tuple[str | None, str], dict]:
    """Every message in the source, keyed by (context, msgid)."""
    messages: dict[tuple[str | None, str], dict] = {}
    problems: list[str] = []

    for path in sorted(SOURCE.glob("*.rs")):
        original = path.read_text(encoding="utf-8")
        notes = translator_notes(original)
        text = blank_comments(original)
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
            note = notes.get(line_of(text, call.start()))
            if note and note not in entry.setdefault("notes", []):
                entry["notes"].append(note)

    if problems:
        for problem in problems:
            print(f"error: {problem}", file=sys.stderr)
        raise SystemExit(1)

    return messages


ESCAPES = {"n": "\n", "t": "\t", "r": "\r", '"': '"', "\\": "\\"}


def unquote(line: str) -> str:
    """The real text of a quoted `.po` fragment, escapes decoded."""
    joined = "".join(re.findall(r'"((?:[^"\\]|\\.)*)"', line))
    out: list[str] = []
    at = 0
    while at < len(joined):
        if joined[at] == "\\" and at + 1 < len(joined):
            out.append(ESCAPES.get(joined[at + 1], joined[at + 1]))
            at += 2
        else:
            out.append(joined[at])
            at += 1
    return "".join(out)


def read_catalog(path: pathlib.Path) -> tuple[str, dict]:
    """An existing catalog as its header block and its translations.

    Keyed by `(context, msgid)` - the key gettext uses and the one `build.rs`
    compiles against. A translation survives a merge exactly when its msgid
    does, which is the whole reason to be careful about rewording a string
    once a catalog exists.
    """
    header = ""
    known: dict = {}

    for block in path.read_text(encoding="utf-8").split("\n\n"):
        if "Plural-Forms:" in block:
            header = block
            continue
        lines = [l for l in block.split("\n") if l and not l.startswith("#")]
        context = msgid = None
        singular = None
        plural: list[str] = []
        current = None
        for line in lines:
            if line.startswith("msgctxt "):
                current, context = "ctx", unquote(line)
            elif line.startswith("msgid_plural "):
                current = "ignore"
            elif line.startswith("msgid "):
                current, msgid = "id", unquote(line)
            elif line.startswith("msgstr["):
                current = "plural"
                plural.append(unquote(line))
            elif line.startswith("msgstr"):
                current, singular = "str", unquote(line)
            elif line.startswith('"') and current:
                piece = unquote(line)
                if current == "ctx":
                    context += piece
                elif current == "id":
                    msgid += piece
                elif current == "str":
                    singular += piece
                elif current == "plural":
                    plural[-1] += piece
        if msgid:
            known[(context, msgid)] = {"str": singular or "", "plural": plural}

    return header, known


def merge(messages: dict) -> None:
    """Brings every `po/*.po` up to date with the template.

    **This is the job a translation platform would otherwise do.** There is no
    Weblate here: hosting one is either a paid service or a server to run, and
    neither is worth committing to before anyone is actually translating. So
    the merge lives in this script, needing nothing installed - the same
    reasoning that has `build.rs` read `.po` files directly rather than
    shelling out to msgfmt.

    A translation is kept when its msgid is still in the template. When it is
    not, it moves to the end of the file as an obsolete `#~` entry rather than
    being deleted: a string that comes back, or a wording somebody wants to
    reuse, is still there to copy from.
    """
    for path in sorted((ROOT / "po").glob("*.po")):
        header, known = read_catalog(path)
        if not header:
            print(f"warning: {path.name} has no header, skipped", file=sys.stderr)
            continue

        out = [header]
        used = set()
        done = 0

        for key, entry in sorted(
            messages.items(), key=lambda item: (item[1]["places"][0], item[0][1])
        ):
            context, msgid = key
            had = known.get(key)
            used.add(key)

            block = []
            for note in entry.get("notes", []):
                block += [f"#. {line}" for line in textwrap.wrap(note, 74)]
            block += references(entry["places"])
            if context is not None:
                block.append(f"msgctxt {quote(context)}")
            block.append(f"msgid {quote(msgid)}")

            if entry["plural"] is not None:
                block.append(f"msgid_plural {quote(entry['plural'])}")
                forms = (had or {}).get("plural") or []
                for index in range(max(len(forms), 2)):
                    block.append(
                        f"msgstr[{index}] "
                        f"{quote(forms[index] if index < len(forms) else '')}"
                    )
                done += 1 if any(forms) else 0
            else:
                body = (had or {}).get("str") or ""
                block.append(f"msgstr {quote(body)}")
                done += 1 if body else 0

            out.append("\n".join(block))

        orphans = sorted(
            (key for key in known if key not in used), key=lambda k: k[1]
        )
        for context, msgid in orphans:
            lines = []
            if context is not None:
                lines.append(f"#~ msgctxt {quote(context)}")
            lines.append(f"#~ msgid {quote(msgid)}")
            lines.append(f"#~ msgstr {quote(known[(context, msgid)]['str'])}")
            out.append("\n".join(lines))

        path.write_text("\n\n".join(out) + "\n", encoding="utf-8", newline="")
        stale = f", {len(orphans)} obsolete" if orphans else ""
        print(f"{path.relative_to(ROOT)}: {done}/{len(messages)} translated{stale}")


def references(places: list[str]) -> list[str]:
    """`#:` lines, wrapped the way gettext wraps them.

    One long line would be simpler and would fight every PO editor: Poedit
    rewraps these on save, so the file would flip between two spellings of the
    same thing each time a translator touched it and this script ran. Matching
    the convention keeps a diff to what actually changed.
    """
    lines: list[str] = []
    for place in places:
        if lines and len(lines[-1]) + 1 + len(place) <= 78:
            lines[-1] += f" {place}"
        else:
            lines.append(f"#: {place}")
    return lines or ["#:"]


def quote(text: str) -> str:
    """A msgid as a .po writes one: escaped, and split at newlines so a long
    message stays readable in a diff."""
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

    Worth acting on BEFORE a string has been translated. A msgid is its own
    key, so changing one afterwards orphans every translation of it: `merge`
    keeps the words as an obsolete `#~` entry to copy from, but somebody has
    to do that for each language. Consolidating is free today and expensive
    next month.
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
        # Wrapped, because a `#.` line is read by a person rather than
        # parsed by anything.
        block = []
        for note in entry.get("notes", []):
            block += [f"#. {line}" for line in textwrap.wrap(note, 74)]
        block += references(entry["places"])
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
    merge(messages)


if __name__ == "__main__":
    main()

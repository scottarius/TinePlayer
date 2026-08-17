# Translating TinePlayer

TinePlayer is translated in `.po` files. You do not need to build anything, and
you do not need to know Rust.

## Translating a language that already exists

1. Open the file for your language in `po/` - `po/ru.po` for Russian.
2. Fill in the empty `msgstr` lines.
3. Open a pull request.

Any PO editor works: Poedit, Lokalize, Gtranslator, or a plain text editor.

## Starting a new language

1. Copy `po/tineplayer.pot` to `po/<code>.po` - `fi` for Finnish, `pt-BR` for
   Brazilian Portuguese.
2. Fill in `Language:`, `Last-Translator:` and `Plural-Forms:` in the header.
   The [gettext manual][plurals] gives the plural rule for most languages.
   Copying the wrong one is the commonest mistake here.
3. Translate.
4. Add your code to `po/LINGUAS`. A language not listed there is not built.

[plurals]: https://www.gnu.org/software/gettext/manual/html_node/Plural-forms.html

## Seeing your work in TinePlayer

Take any TinePlayer release and point it at your file:

```sh
TINEPLAYER_PO=/path/to/fi.po tineplayer
```

```powershell
$env:TINEPLAYER_PO = "C:\path\to\fi.po"; & "C:\Program Files\TinePlayer\TinePlayer.exe"
```

Restart it to pick up a change. Anything you have not translated yet appears in
English, so a half-finished file is fine to load.

To look at a language that is already built in, use `TINEPLAYER_LANG=fi`
instead. Both override **Settings → General → Interface Language**.

## While you translate

**Keep every `{hole}`.** `Disconnect from {server}` must keep `{server}`
somewhere in your translation. It can go anywhere in the sentence - that is why
the holes are named rather than numbered. Dropping or misspelling one fails our
tests, so it will be caught, but it is quicker to catch yourself.

**Read the notes.** Lines beginning `#.` above a message tell you something the
string alone cannot. Most often they name another project's vocabulary: "Quick
Connect" is Jellyfin's own term, so use whatever **Jellyfin's** translation into
your language calls it. That text sends someone to find the feature in their
Jellyfin app, and a fresh wording sends them looking for something that is not
there. The same goes for Kodi's terms.

**Watch for `msgctxt`.** Where English is ambiguous, a message carries a
context line saying which sense is meant. "None" appears three times - an audio
output device, an audio track, a subtitle track - and many languages need
different words. The context is never shown to anyone using TinePlayer.

**Plurals are not always two.** If your language has three forms or six, say so
in `Plural-Forms:` and fill in `msgstr[0]` upwards. Do not force your language
into English's two.

**Keep it short where you can.** TinePlayer is read from across a room, at
large type, often on a television. A row label that fits in English can be cut
off in a longer language. Where two phrasings are both right, prefer the
shorter - and the row labels matter more than the notes beneath them.

**Match your language's conventions, not English's.** Row labels are Title Case
in English only because English does that. "TinePlayer", "Jellyfin" and "Kodi"
are names and stay as they are.

## Not translated

- **The command line** and anything it prints.
- **The language list** under Preferred Language and Subtitle Preference. Those
  entries already show each language's own name.
- **Messages printed to the terminal on failure.** They end up in bug reports.

## Fonts

TinePlayer bundles its own fonts so text looks the same on every platform.

For almost every language this needs nothing from you: Latin, Greek, Cyrillic,
Arabic, Hebrew, Thai, Devanagari, Bengali, Tamil, Telugu, Malayalam, Gurmukhi,
Georgian and Armenian are all bundled complete.

**Chinese, Japanese and Korean are the exception.** Those fonts are bundled cut
down to the characters TinePlayer uses, so a translation into one of them needs
the fonts rebuilt - which happens here, not in your pull request. Mention it and
it will be handled. Our tests catch it either way, so nothing ships broken.

---

## For maintainers

After adding or changing any interface string:

```sh
python3 packaging/extract-strings.py
```

This rewrites `po/tineplayer.pot` and merges it into every `po/*.po`, reporting
how much of each is now translated. A translation is kept where its msgid still
exists; where it does not, it moves to the end of the file as an obsolete `#~`
entry rather than being deleted.

**Reword an existing string and you orphan its translation in every language.**
The msgid is the key. The old wording survives as `#~` to copy from, but
somebody has to do that per language - so consolidate wording early. The script
lists near-duplicate messages after each run to help with that.

To tell a translator something the string cannot say for itself:

```rust
// TRANSLATORS: "Quick Connect" is Jellyfin's own name for this feature.
// Use whatever Jellyfin's translation into your language calls it.
let page = wizard_page(&tr!("Quick Connect"));
```

That reaches the catalog as a `#.` comment above the message. Worth adding for
another project's vocabulary, for a string whose placement is not obvious, and
wherever the available width is tight.

After a translation lands in a script outside Latin, Greek and Cyrillic:

```sh
pip install fonttools brotli
python3 packaging/fonts/build-fonts.py          # rebuild, needs network
python3 packaging/fonts/build-fonts.py --check  # verify, needs neither
```

The build reads every `.po` in `po/`, including ones not yet in `LINGUAS`.
`--check` runs in CI on every commit.

How catalogs are compiled into the binary is documented at the top of
`src/i18n.rs`.

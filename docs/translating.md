# Translating TinePlayer

TinePlayer is translated in `.po` files, the format most software translation
uses. You do not need to build anything, and you do not need to know Rust.

## Translating an existing language

Use Weblate. It edits the `.po` files in `po/`, keeps them in step with the
English as it changes, and opens the pull request for you.

If you would rather work offline, edit the `.po` file directly and open a pull
request against it. Any PO editor works - Poedit, Lokalize, Gtranslator, or a
text editor.

## Starting a new language

1. Copy `po/tineplayer.pot` to `po/<code>.po`, where `<code>` is the language's
   code: `fi` for Finnish, `pt-BR` for Brazilian Portuguese.
2. Fill in the header: `Language:`, `Last-Translator:`, and `Plural-Forms:` for
   your language. The [gettext manual][plurals] lists the rule for most
   languages; copying the wrong one is the single most common mistake here.
3. Translate.
4. Add the code to `po/LINGUAS`. Nothing ships until it is listed there.

[plurals]: https://www.gnu.org/software/gettext/manual/html_node/Plural-forms.html

## Seeing your work in the application

You do not need a Rust toolchain for this. Take any TinePlayer release and
point it at your file:

```sh
TINEPLAYER_PO=/path/to/fi.po tineplayer
```

```powershell
$env:TINEPLAYER_PO = "C:\path\to\fi.po"; & "C:\Program Files\TinePlayer\TinePlayer.exe"
```

It reads the file at startup, so restart it to see a change. Anything you have
not translated yet shows in English rather than blank, and a half-finished file
is fine to load.

To check a language that is already built in, use `TINEPLAYER_LANG=fi` instead.
Either beats the setting under **Settings > General > Interface Language**,
which is where somebody who is not translating would choose.

## Things worth knowing

**`{holes}` must survive.** `Disconnect from {server}` has to keep `{server}`
somewhere in your translation, though it can go anywhere in the sentence - that
is why they are named rather than numbered. A translation that drops one or
misspells it fails the build's own check, so it will be caught, but it is
quicker to notice now.

**Some strings carry a context.** Where the English is ambiguous, the message
has a `msgctxt` line saying which sense is meant. `None` appears twice, once
for an audio output device and once for a list of languages, and several
languages need different words for the two. The context is never shown to
anyone using TinePlayer.

**Plurals are not always two.** If your language has three forms or six, say so
in `Plural-Forms:` and fill in `msgstr[0]` through `msgstr[n]`. Do not force
your language into English's two.

**Length matters here more than in most applications.** TinePlayer is read from
across a room at deliberately large type, often on a television. German and
Finnish tend to run about 30% longer than English, and a settings row that fits
in English can be cut short in another language. Shorter is better where two
phrasings are both correct, and the row labels are worth more care than the
notes underneath them.

**Sentence case, and TinePlayer's own words.** Row labels are Title Case in
English only because English does that; use whatever your language does.
"TinePlayer", "Jellyfin" and "Kodi" are names and stay as they are.

## What is not translated, and why

- **The command line** (`--help` and everything it prints). It is read by
  people already at a terminal, and the documentation is English regardless.
- **The language list** under Preferred Language and Subtitle Preference. Those
  fifty entries already show each language's own name beside the English one,
  which is what somebody looking for their own language actually reads.
- **Messages printed to the terminal when something goes wrong.** They end up
  in bug reports, and a bug report is more use in the language the issue
  tracker is written in.

## Fonts

TinePlayer bundles its own fonts so that text looks the same on Windows, macOS
and a Raspberry Pi rather than depending on what each machine happens to have.

**For almost every language this needs nothing from you.** The Latin, Greek,
Cyrillic, Arabic, Hebrew, Thai, Devanagari, Bengali, Tamil, Telugu, Malayalam,
Gurmukhi, Georgian and Armenian faces are all bundled whole.

**Chinese, Japanese and Korean are the exception.** Those faces are 10 MB each
whole, which would roughly double every download, so they are bundled cut down
to the characters the interface actually uses. That includes your translation -
but only once the fonts have been rebuilt here, which is a manual step needing
the network and is not something Weblate can do. CI will catch it, so nothing
ships broken; mention it on the pull request and it will be handled.

Even then, a film *title* in Chinese or Japanese coming from a media library
falls back to the system font, since there is no way to know in advance what
characters a library contains. Windows and macOS both ship CJK fonts, so this
only affects Linux machines without one installed.

## For maintainers

After adding or changing any interface string:

```sh
python3 packaging/extract-strings.py
```

That rewrites `po/tineplayer.pot`. Weblate merges it into every catalog by
itself, so nothing needs running against the `.po` files by hand.

After a translation lands in a script outside Latin, Greek and Cyrillic:

```sh
pip install fonttools brotli
python3 packaging/fonts/build-fonts.py          # rebuilds, needs network
python3 packaging/fonts/build-fonts.py --check  # verifies, needs neither
```

The build script reads every `.po` in `po/` - not only the ones in `LINGUAS` -
so a catalog somebody is still working on is covered too. `--check` is what CI
runs on every commit.

The rest of how this works - why catalogs are compiled into the binary rather
than installed beside it, and what `build.rs` does with them - is in the
comment at the top of `src/i18n.rs`.

Two things that will fail the build or the tests, on purpose:

- A `.po` listed in `LINGUAS` that will not parse, or whose `Plural-Forms` rule
  makes no sense, **stops the build**. It is a file in this repository, and the
  alternative is an application that quietly comes up in English for one
  language until somebody who reads that language notices.
- A translation whose `{holes}` do not match the English **fails
  `cargo test`** rather than the build, because that file came from somebody
  outside the project and refusing to build is a poor way to tell them. It
  still has to be fixed before a release.

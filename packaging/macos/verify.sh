#!/usr/bin/env bash
# Checks a finished bundle for the two faults that do not announce themselves.
#
# Run by package.sh once the bundle is filled and repointed.
#
# Both of these ship happily. A bundle with the wrong GTK launches, plays film,
# and is silent to a screen reader; a bundle holding a path into the build
# machine launches on that machine and nowhere else. Neither produces an error
# at any point, which is why they are asserted here rather than trusted.
set -euo pipefail

cd "$(dirname "$0")/../.."

app="${1:-dist/macos/TinePlayer.app}"
frameworks="$app/Contents/Frameworks"
if [[ ! -d "$frameworks" ]]; then
    echo "No bundle at $app. Run: ./packaging/macos/package.sh" >&2
    exit 1
fi

failed=0

# --- Nothing may point outside the bundle -------------------------------
#
# Everything bundled was rewritten to @rpath. Anything still naming an
# absolute path under /Users or a Homebrew prefix was missed by the walk, and
# the bundle depends on a machine it will not be run on. System libraries
# under /usr/lib and /System are the exception and are meant to stay.
#
# A library's own install id is printed first by `otool -L`, among its
# dependencies and looking exactly like one - the same trap `contents.sh`
# documents in `linked()`. It is not a dependency and nothing loads by it once
# every reference has been rewritten to @rpath, so an id still naming
# Homebrew is harmless and is skipped by name here. Counting it reported every
# bundled library as broken.
#
# Collected into one string rather than checked in a loop with `set -e` in
# force: a grep that matches nothing exits non-zero, and under `pipefail` that
# is the *success* case aborting the script. Every pipeline here therefore
# ends in `|| true` and the verdict is read from what it produced.
echo "Checking for paths outside the bundle..."
outside=""
while read -r file; do
    strays="$(otool -L "$file" 2>/dev/null | tail -n +2 | awk '{print $1}' |
        grep -E '^(/Users/|/opt/homebrew/|/usr/local/)' |
        grep -v "/$(basename "$file")\$" || true)"
    if [[ -n "$strays" ]]; then
        outside+="  ${file#"$app/"}"$'\n'
        while read -r dep; do
            [[ -n "$dep" ]] && outside+="    $dep"$'\n'
        done <<< "$strays"
    fi
done < <(find "$app/Contents" -type f \( -name "*.dylib" -o -name "*.so" -o -perm +111 \) 2>/dev/null || true)

if [[ -n "$outside" ]]; then
    echo "Bundle depends on paths outside itself:" >&2
    printf '%s' "$outside" >&2
    failed=1
else
    echo "  none"
fi

# --- The GTK inside has to be the one with AccessKit --------------------
#
# GTK states this about itself, in the backend list it prints for GTK_A11Y.
# Built without it, that line reads "accesskit - Disabled during GTK build" and
# every Mac user gets a window a screen reader cannot see into. Homebrew's gtk4
# is built that way, so this is the check that says which GTK got bundled.
#
# Asserted rather than assumed because the way it goes wrong is quiet: a build
# that compiles against one GTK and links another produces a working player
# with no accessibility and no complaint.
#
# The line is captured and then matched, rather than tested with `grep -q` in
# a condition. Under `pipefail` a matching `grep -q` reports *failure*: it
# exits the moment it matches, `strings` is killed by SIGPIPE, and the
# pipeline takes 141 from it - so the branch that matches is the branch that
# looks false. Measured here, not reasoned about: 141 with pipefail and 0
# without, on the same command.
echo "Checking the bundled GTK for the AccessKit backend..."
gtk="$(find "$frameworks" -name "libgtk-4*.dylib" -type f 2>/dev/null | head -1 || true)"
line="$(strings "$gtk" 2>/dev/null | grep -m1 "accesskit -" || true)"
if [[ -z "$gtk" ]]; then
    echo "  no libgtk-4 in the bundle at all" >&2
    failed=1
elif [[ "$line" == *"Use the AccessKit"* ]]; then
    echo "  $(basename "$gtk"): AccessKit enabled"
elif [[ "$line" == *"Disabled during GTK build"* ]]; then
    echo "  $(basename "$gtk"): built WITHOUT AccessKit" >&2
    echo "  A screen reader will see the window and nothing inside it." >&2
    echo "  Build GTK with -Daccesskit=enabled and set TINEPLAYER_GTK_PREFIX." >&2
    failed=1
else
    echo "  $(basename "$gtk"): cannot tell - no accesskit line at all" >&2
    echo "  This GTK may predate 4.18, which is where the backend arrived." >&2
    failed=1
fi

# --- Everything shipped has to have its terms in the bundle -------------
#
# `licenses.sh` accounts for a library by asking Homebrew which formula owns
# it, so anything built outside Homebrew is invisible to it and lands in a
# list headed "built into TinePlayer itself, or part of macOS" - which is a
# false statement about somebody else's library, in the one file whose job is
# to be accurate about exactly that.
#
# That is how libaccesskit-c came to be shipped with its license nowhere in
# the bundle. Caught here rather than at release time, because README and
# THIRD-PARTY.md both promise a packaged build carries the paperwork.
echo "Checking every bundled library is accounted for..."
report="$app/Contents/Resources/licenses/BUNDLED-VERSIONS.txt"
if [[ ! -f "$report" ]]; then
    echo "  no BUNDLED-VERSIONS.txt at all" >&2
    failed=1
else
    orphans="$(awk '/^Not owned by any Homebrew formula/,0' "$report" |
        grep -E '^\s+lib.*\.(dylib|so)$' || true)"
    if [[ -n "$orphans" ]]; then
        echo "Shipped with no license text and no owner:" >&2
        printf '%s\n' "$orphans" >&2
        echo "  Put its license under \$TINEPLAYER_GTK_PREFIX/share/licenses/<name>/." >&2
        failed=1
    else
        echo "  all accounted for"
    fi
fi

if (( failed )); then
    echo "Bundle verification failed." >&2
    exit 1
fi
echo "Bundle verified."

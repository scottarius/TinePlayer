#!/usr/bin/env bash
# Builds everything macOS needs, in one command: the application bundle, the
# libraries and licenses inside it, and the disk image people download.
#
#   ./packaging/macos/package.sh
#
# The three steps are separate scripts because each is worth running on its
# own while working on it. This is what a release runs.
#
# Signing is ad-hoc unless a Developer ID is named, in which case the bundle
# is signed properly and submitted for notarization. See NOTARIZING below.
set -euo pipefail

cd "$(dirname "$0")/../.."
here="packaging/macos"

case "${1:-}" in
-h | --help)
    sed -n '2,12p' "$here/package.sh" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
esac

# Built here rather than assumed, matching what package.ps1 does on Windows.
# Set TINE_SKIP_BUILD=1 to package whatever is already in target/release.
if [[ "${TINE_SKIP_BUILD:-}" != "1" ]]; then
    echo "=== Building ==="
    cargo build --release
fi

echo
echo "=== Building the bundle ==="
"$here/bundle.sh"

echo
echo "=== Filling it ==="
"$here/contents.sh"

echo
echo "=== Building the disk image ==="
"$here/dmg.sh"

# --- Notarizing ----------------------------------------------------------
#
# Without this, macOS refuses to open a downloaded copy until the user goes
# into System Settings and allows it by hand. Notarization is what removes
# that, and it needs a paid Apple developer account.
#
# Set these and it happens automatically:
#
#   TINE_SIGN_IDENTITY   "Developer ID Application: Your Name (TEAMID)"
#   TINE_NOTARY_PROFILE  a notarytool keychain profile, made once with:
#                        xcrun notarytool store-credentials
#
# Left unset, the bundle keeps its ad-hoc signature, which runs on the
# machine that built it and nowhere else without a warning.
version="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
dmg="dist/macos/TinePlayer-$version-macos.dmg"

if [[ -n "${TINE_SIGN_IDENTITY:-}" ]]; then
    echo
    echo "=== Signing with a Developer ID ==="

    # Extended attributes, cleared before anything is signed. codesign refuses
    # a file carrying Finder metadata outright - "resource fork, Finder
    # information, or similar detritus not allowed" - and building the disk
    # image above is what puts it there: dmgbuild arranges a Finder window and
    # the attributes land on the bundle it was handed.
    #
    # Cleared rather than prevented, because anything that so much as looks at
    # the bundle in Finder can put them back, and the failure arrives at the
    # very end of a long build.
    #
    # Files and directories only, not `xattr -cr`. That follows symlinks, and
    # GTK's hicolor theme ships links to icons for its own demo applications
    # which are not copied here - so it fails on every dangling one and takes
    # the script down with it. A symlink carries no attributes of its own that
    # codesign objects to.
    find "dist/macos/TinePlayer.app" \( -type f -o -type d \) -exec xattr -c {} +

    # Every --timestamp is a call to Apple's timestamp authority, and this
    # bundle has about a hundred and forty things to sign. Somewhere in the
    # run the service starts answering "The timestamp service is not
    # available" - it is up, and rate limiting: measured 2026-08-03, it
    # refused after 142 files while a plain request to it answered in a third
    # of a second.
    #
    # A timestamp is not optional. It is what keeps a signature valid after
    # the certificate expires, and notarization requires one, so the answer is
    # to wait and ask again rather than to sign without.
    signed() {
        local attempt output
        for attempt in 1 2 3 4 5; do
            if output="$(codesign "$@" 2>&1)"; then
                [[ -n "$output" ]] && echo "$output"
                return 0
            fi
            # Only the timestamp service is worth waiting out. Anything else
            # is a real problem that another attempt will not fix.
            if [[ "$output" != *"timestamp service is not available"* ]]; then
                echo "$output" >&2
                return 1
            fi
            echo "  timestamp service busy, waiting $((attempt * 10))s" >&2
            sleep $((attempt * 10))
        done
        echo "The timestamp service kept refusing: ${*: -1}" >&2
        return 1
    }

    # Everything that is a Mach-O binary, found by asking rather than by
    # matching a name. This used to be `-name "*.dylib" -o -name "*.so"`,
    # which missed `libexec/gst-plugin-scanner` - a helper executable with no
    # extension. It kept its ad-hoc signature, and Apple rejected the whole
    # submission for it: not signed with a Developer ID, no secure timestamp,
    # no hardened runtime. One file out of a hundred and forty-four, and the
    # only sign of it was a notarization that came back "Invalid".
    #
    # Read from a process substitution rather than piped into a loop, so that
    # a failure stops the script instead of dying quietly in a subshell.
    #
    # Same order as the ad-hoc signing: nested code first, bundle last, or the
    # nested signatures are the ones from before install_name_tool ran.
    while IFS= read -r -d '' binary; do
        case "$(file -b "$binary")" in
        *Mach-O*) ;;
        *) continue ;;
        esac
        signed --force --options runtime --timestamp \
            --sign "$TINE_SIGN_IDENTITY" "$binary"
    done < <(find "dist/macos/TinePlayer.app" -type f -print0)
    signed --force --options runtime --timestamp \
        --sign "$TINE_SIGN_IDENTITY" "dist/macos/TinePlayer.app/Contents/MacOS/TinePlayer"
    signed --force --options runtime --timestamp \
        --sign "$TINE_SIGN_IDENTITY" "dist/macos/TinePlayer.app"

    # Checked here, before anything else touches the bundle. Building the disk
    # image below puts com.apple.FinderInfo back on it - dmgbuild arranges a
    # Finder window - and this check then fails on metadata added after the
    # signature, complaining about a bundle that is perfectly good. The copy
    # inside the image is made before that happens and is the one that ships.
    codesign --verify --deep --strict --verbose=2 "dist/macos/TinePlayer.app"

    # The disk image is what gets downloaded, so it is what gets notarized.
    # Rebuilt now that the bundle inside it is signed.
    "$here/dmg.sh"
    # For the same reason as above: the image has just come out of Finder.
    xattr -c "$dmg" 2>/dev/null || true
    signed --force --timestamp --sign "$TINE_SIGN_IDENTITY" "$dmg"

    if [[ -n "${TINE_NOTARY_PROFILE:-}" ]]; then
        echo
        echo "=== Notarizing ==="
        # TINE_NOTARY_KEYCHAIN names the keychain holding the profile, for a
        # build machine that keeps credentials somewhere other than the login
        # keychain. A runner does: its keychain is created for the job and
        # thrown away with it.
        #
        # Spelled out twice rather than built as an array of arguments.
        # Expanding an empty array under `set -u` is an error in bash 3.2,
        # which is what macOS still ships and what runs this script - and the
        # empty case is precisely the local one, where the profile lives in
        # the login keychain and no override is given. CI always sets the
        # variable, so the array was never empty there and the fault could
        # only ever appear on a developer's own machine.
        # The submission is kept so that a rejection can be explained. Apple
        # answers "Invalid" and nothing else; the log is what names the files
        # it objected to, and going to fetch it by hand afterwards is a step
        # nobody remembers under pressure.
        submitted="$(mktemp)"
        notary_status=0
        if [[ -n "${TINE_NOTARY_KEYCHAIN:-}" ]]; then
            xcrun notarytool submit "$dmg" \
                --keychain-profile "$TINE_NOTARY_PROFILE" \
                --keychain "$TINE_NOTARY_KEYCHAIN" --wait 2>&1 |
                tee "$submitted" || notary_status=$?
        else
            xcrun notarytool submit "$dmg" \
                --keychain-profile "$TINE_NOTARY_PROFILE" --wait 2>&1 |
                tee "$submitted" || notary_status=$?
        fi

        if [[ "$notary_status" -ne 0 ]] || grep -q "status: Invalid" "$submitted"; then
            id="$(awk '/  id: /{print $2; exit}' "$submitted")"
            echo
            echo "=== Why it was rejected ===" >&2
            if [[ -n "$id" ]]; then
                if [[ -n "${TINE_NOTARY_KEYCHAIN:-}" ]]; then
                    xcrun notarytool log "$id" \
                        --keychain-profile "$TINE_NOTARY_PROFILE" \
                        --keychain "$TINE_NOTARY_KEYCHAIN" >&2 || true
                else
                    xcrun notarytool log "$id" \
                        --keychain-profile "$TINE_NOTARY_PROFILE" >&2 || true
                fi
            else
                echo "No submission id to ask about." >&2
            fi
            rm -f "$submitted"
            exit 1
        fi
        rm -f "$submitted"
        # Stapling puts the ticket inside the image, so it opens even on a
        # machine that cannot reach Apple to ask.
        xcrun stapler staple "$dmg"
        xcrun stapler validate "$dmg"

        # What a viewer's Mac will actually decide, asked the same way
        # Gatekeeper asks it. Worth doing here rather than trusting that a
        # successful notarization means a working download: this is the check
        # that fails if the hardened runtime rejected a bundled library.
        echo
        echo "=== What Gatekeeper makes of it ==="
        spctl --assess --type open --context context:primary-signature \
            --verbose=2 "$dmg"
    else
        echo "TINE_NOTARY_PROFILE not set, so the image is signed but not notarized." >&2
    fi
fi

echo
echo "Done. In dist/:"
ls -1sh "dist/macos/TinePlayer-$version-macos.dmg" 2>/dev/null || true

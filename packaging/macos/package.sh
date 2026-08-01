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
    # Same order as the ad-hoc signing: libraries first, bundle last, or the
    # nested signatures are the ones from before install_name_tool ran.
    find "dist/macos/TinePlayer.app" \( -name "*.dylib" -o -name "*.so" \) -print0 |
        xargs -0 -n1 codesign --force --options runtime --timestamp \
            --sign "$TINE_SIGN_IDENTITY"
    codesign --force --options runtime --timestamp \
        --sign "$TINE_SIGN_IDENTITY" "dist/macos/TinePlayer.app/Contents/MacOS/TinePlayer"
    codesign --force --options runtime --timestamp \
        --sign "$TINE_SIGN_IDENTITY" "dist/macos/TinePlayer.app"
    # The disk image is what gets downloaded, so it is what gets notarized.
    "$here/dmg.sh"
    codesign --force --timestamp --sign "$TINE_SIGN_IDENTITY" "$dmg"

    if [[ -n "${TINE_NOTARY_PROFILE:-}" ]]; then
        echo
        echo "=== Notarizing ==="
        # TINE_NOTARY_KEYCHAIN names the keychain holding the profile, for a
        # build machine that keeps credentials somewhere other than the login
        # keychain. A runner does: its keychain is created for the job and
        # thrown away with it.
        notary_keychain=()
        [[ -n "${TINE_NOTARY_KEYCHAIN:-}" ]] &&
            notary_keychain=(--keychain "$TINE_NOTARY_KEYCHAIN")
        xcrun notarytool submit "$dmg" \
            --keychain-profile "$TINE_NOTARY_PROFILE" "${notary_keychain[@]}" --wait
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
        codesign --verify --deep --strict --verbose=2 "dist/macos/TinePlayer.app"
    else
        echo "TINE_NOTARY_PROFILE not set, so the image is signed but not notarized." >&2
    fi
fi

echo
echo "Done. In dist/:"
ls -1sh "dist/macos/TinePlayer-$version-macos.dmg" 2>/dev/null || true

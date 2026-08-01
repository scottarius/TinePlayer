# Layout for the disk image, read by dmgbuild.
#
# This is what makes the window look like every other Mac installer: a fixed
# size, large icons, the application on the left and a shortcut to
# Applications on the right, with nothing else in the way.
#
# dmgbuild writes all of it into the image's .DS_Store itself, so no Finder
# and no graphical session is involved. That is the difference between this
# and create-dmg, which drives Finder with AppleScript and therefore cannot
# run over SSH or on a build runner.

import os

application = os.environ.get("TINE_APP", "dist/macos/TinePlayer.app")
appname = os.path.basename(application)

# What ends up in the window: the application, and the usual shortcut to drag
# it into.
files = [application]
symlinks = {"Applications": "/Applications"}

# The artwork is 660 x 400, and that is the size of the *content* area. A
# window's height includes its title bar, so asking for 400 leaves the bottom
# 28 points of the background off the end of the window. Hence 428: the title
# bar, plus exactly the picture.
TITLE_BAR = 28
window_rect = ((200, 120), (660, 400 + TITLE_BAR))
icon_size = 128
text_size = 14

icon_locations = {
    appname: (165, 190),
    "Applications": (495, 190),
}

# Without this the icon is labelled "TinePlayer.app", extension and all,
# which is not how an application is named anywhere else on a Mac. The
# setting is plural: the singular form is the per-file spelling, and setting
# that name here does nothing at all, silently.
hide_extensions = [appname]

# Icon view, no toolbar or sidebar: a plain window with two icons in it, which
# is what people expect when a disk image opens.
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
arrange_by = None
grid_offset = (0, 0)
label_pos = "bottom"

# Compressed, and no bigger than it needs to be.
format = "UDZO"
compression_level = 9

# The mounted volume takes the application's own icon, rather than the
# generic white disk. bundle.sh generates this from data/tineplayer.png, so
# there is nothing extra to keep in step.
_icns = "dist/macos/TinePlayer.app/Contents/Resources/TinePlayer.icns"
if os.path.exists(_icns):
    icon = _icns
else:
    badge_icon = None

# Artwork if there is any, and the plain arrow if not, so the image builds
# either way. See dmg-background.md beside this file for what to draw.
# dmg.sh works out which artwork to use, building the two-resolution TIFF
# from a single PNG when that is what it finds, and passes the result here.
# Falls back to macOS's plain arrow so the image builds with no artwork.
_art = os.environ.get("TINE_DMG_BACKGROUND", "")
background = _art if _art and os.path.exists(_art) else "builtin-arrow"

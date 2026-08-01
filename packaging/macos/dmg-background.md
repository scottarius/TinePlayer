# The disk image background

Drop a file called `dmg-background.tiff` beside this one and the disk image
will use it. Without it, the image gets macOS's plain built-in arrow, which
works but says nothing.

## What to draw

The window is **660 x 400 points**. Two icons sit on top of it, each 128
points square, centered at:

- the application, at **(165, 190)**
- the shortcut to Applications, at **(495, 190)**

measured from the top left. So the artwork wants to leave those two areas
clear and put whatever it says - an arrow, a line, a word - in the space
between them, roughly x 240 to 420.

Bear in mind the icon labels sit *below* each icon, so leave room under them
too. Anything within about 30 points of the window edges risks being clipped
on a window that opens slightly differently.

## Just supply one image

Save it as **`dmg-background.png` at 1320 x 800**, which is the retina size,
and `dmg.sh` does the rest: it makes the half-size version with `sips` and
combines both into the two-resolution TIFF that a disk image needs. Both
tools ship with macOS, so there is nothing to install.

If you would rather build the TIFF yourself - to control the two resolutions
separately, say - name it `dmg-background.tiff` and it will be used as it is,
ahead of any PNG.

## Worth knowing

The volume name appears in the window's title bar, above the artwork, so
there is no need to repeat the application's name in the image itself.

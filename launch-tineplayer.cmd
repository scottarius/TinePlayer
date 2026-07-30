@echo off
rem Starts TinePlayer with whatever arguments it is given. Anything that needs
rem to launch the player on Windows should point here rather than straight at
rem TinePlayer.exe: Kodi's playercorefactory.xml, a shortcut, another front end.
rem
rem Windows looks for libraries in the working directory before it looks in most
rem other places, so a launcher that starts TinePlayer while sitting in its own
rem folder can hand it the wrong copies of libraries GStreamer also ships. That
rem kills the player before it runs a line of its own code, with nowhere to
rem report the problem. Kodi does exactly this. Moving to TinePlayer's own
rem folder first is enough for the right copies to win.
rem
rem The player is run rather than started in the background, so a launcher that
rem waits for it - Kodi does - sees a process that lives as long as the film.

rem Locations are worked out from this script's own folder (%~dp0), so nothing
rem has to be substituted in and the file works wherever TinePlayer is kept.
rem Beside the executable in a packaged build, at the top of a source tree.
set "TINE=%~dp0"
if not exist "%TINE%TinePlayer.exe" set "TINE=%~dp0target\release\"

rem pushd rather than cd: it also handles a UNC path, by mapping it to a drive
rem for the duration, which cd cannot do at all.
pushd "%TINE%" || exit /b 1
"%TINE%TinePlayer.exe" %*

; Installer for TinePlayer on Windows.
;
; Compiled by package.ps1 beside it, which stages the application first and
; passes in where it put it. Nothing here builds anything; this only wraps
; what is already in the staging folder.
;
; Installs per user, into %LOCALAPPDATA%\Programs, so it needs no
; administrator and raises no prompt. That matters more than it sounds: this
; is an application for people who want to watch a film together, some of whom
; are being talked through it by someone else, and "type an administrator
; password" is where that conversation ends.

#define AppName "TinePlayer"
#define AppPublisher "Scott Bounds"
#define AppUrl "https://github.com/scottarius/TinePlayer"

; Passed in by the packaging script: /DAppVersion=... /DStageDir=...
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef AppVersionNumeric
  #define AppVersionNumeric "0.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\dist\TinePlayer-0.0.0-windows-x64"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist\windows"
#endif
; The top of the source tree, passed in rather than worked out by counting
; ".." from the staging folder - which broke the moment that folder moved.
#ifndef RootDir
  #define RootDir "..\.."
#endif

[Setup]
; Never reuse this: it is how Windows recognises an upgrade of the same
; application rather than a second copy of it.
AppId={{8F3B2A14-6C5D-4E7A-9B21-TINEPLAYER01}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
; Numbers only - Inno rejects a version with a suffix here, while AppVersion
; above takes any string. Between releases Cargo.toml carries something like
; 1.1.0-dev, so the two have to be allowed to differ.
VersionInfoVersion={#AppVersionNumeric}

DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; Per user, so no administrator is needed. autopf resolves to
; %LOCALAPPDATA%\Programs under this setting.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

LicenseFile={#StageDir}\licenses\TinePlayer-MIT.txt
OutputDir={#OutputDir}
OutputBaseFilename={#AppName}-{#AppVersion}-windows-x64-setup
SetupIconFile={#RootDir}\data\branding\tineplayer.ico
UninstallDisplayIcon={app}\TinePlayer.exe
UninstallDisplayName={#AppName} {#AppVersion}

Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
; The application is for televisions; the installer is not, and its own
; defaults read better than anything invented here.

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"
; Off unless asked for, because it changes something outside the installation
; folder. A shortcut is undone by deleting it; an environment variable is not
; obvious to find, so it should be opted into rather than out of.
Name: "addtopath"; Description: "Add TinePlayer to &PATH, so it can be run from a terminal"; GroupDescription: "Command line:"; Flags: unchecked

[Registry]
; PATH, when the box above is ticked.
;
; Which PATH follows the install: a per-user install writes the user's, an
; elevated all-users install writes the machine's. Somebody installing for
; every account means every account. {app} is whichever folder was chosen -
; Program Files, the per-user Programs folder, or a custom one - so the value
; needs no special handling for the two cases, only the location does.
;
; expandsz rather than string, and this is the entry worth being careful
; about: the machine PATH normally contains %SystemRoot%\system32, and writing
; it back as a plain string would bake today's expansion in permanently.
;
; Inno notifies running applications that the environment changed once Setup
; finishes, so a terminal opened afterwards sees it without logging out.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Tasks: addtopath; Check: not IsAdminInstallMode and NeedsAddPath
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Tasks: addtopath; Check: IsAdminInstallMode and NeedsAddPath

[InstallDelete]
; Cleared before the new files land, because installing over an old copy
; overwrites what this version ships and leaves behind anything it does not.
; For most applications that is wasted disk. Here it is a bug waiting to
; happen: GStreamer scans lib\gstreamer-1.0 and registers whatever it finds,
; so a plugin dropped by an older version keeps being loaded beside the new
; ones. That fails only on upgrades, never on a clean install, which is the
; hardest kind of report to make sense of.
;
; Only directories this installer owns and fills. TinePlayer.exe is left to be
; overwritten in place rather than deleted, since it may be what the user just
; closed at the Restart Manager prompt.
Type: filesandordirs; Name: "{app}\lib"
Type: filesandordirs; Name: "{app}\libexec"
Type: filesandordirs; Name: "{app}\share"
Type: filesandordirs; Name: "{app}\licenses"
; The fonts, for the same reason as the plugins: fontconfig is pointed at this
; directory and registers whatever is in it, so a face left behind by an older
; version stays available to be picked. Missed when the fonts were first
; packaged, and found by installing over a planted file rather than by reading
; this list.
Type: filesandordirs; Name: "{app}\fonts"
Type: files; Name: "{app}\*.dll"

[Files]
; Everything the packaging script staged, libraries and all. Recursing over
; the staging folder rather than listing files keeps this from drifting out of
; step with what the package actually contains.
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\TinePlayer.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\TinePlayer.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\TinePlayer.exe"; Description: "Start {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; GStreamer's plugin registry, written beside the settings rather than into
; the installation. Left behind otherwise, naming plugins that are gone. It is
; ours and it rebuilds itself on the next run, so it always goes.
;
; The settings and the saved positions in the same folder are the user's, and
; are only removed if they ask: see CurUninstallStepChanged below.
Type: files; Name: "{localappdata}\TinePlayer\registry.bin"

[Code]
// --- PATH ------------------------------------------------------------------
//
// Which of the two PATH variables this install is entitled to edit. The root
// and the subkey both differ, which is why this is a procedure rather than
// Inno's HKA shorthand.
procedure PathLocation(var Root: Integer; var Key: string);
begin
  if IsAdminInstallMode then
  begin
    Root := HKEY_LOCAL_MACHINE;
    Key := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  end
  else
  begin
    Root := HKEY_CURRENT_USER;
    Key := 'Environment';
  end;
end;

// Whether {app} is missing from PATH and so wants adding.
//
// Without this, installing over an existing copy appends the same folder
// again, and again on the one after that. Compared as a whole entry between
// separators rather than as a substring, so a folder is not mistaken for one
// whose name it happens to begin.
//
// RegQueryStringValue reads REG_EXPAND_SZ without expanding it, which is what
// this needs: the comparison is against the stored text, and the value is
// never written back from here.
function NeedsAddPath: Boolean;
var
  Root: Integer;
  Key, Existing, Dir: string;
begin
  PathLocation(Root, Key);
  Dir := ExpandConstant('{app}');
  if not RegQueryStringValue(Root, Key, 'Path', Existing) then
  begin
    // No PATH of their own yet, which is ordinary for a user account.
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Existing) + ';') = 0;
end;

// Takes {app} back out again, leaving the rest of PATH as it was.
//
// String surgery rather than an uninsdeletevalue flag on the entry above,
// which would delete the whole Path variable instead of the part this
// installer added. Case-insensitively, because what is stored may not be
// spelled the way {app} is.
procedure RemoveFromPath;
var
  Root, P: Integer;
  Key, Existing, Padded, Needle, Updated, Dir: string;
begin
  PathLocation(Root, Key);
  Dir := ExpandConstant('{app}');
  if not RegQueryStringValue(Root, Key, 'Path', Existing) then
    exit;

  // Padded at both ends so the first and last entries have separators either
  // side of them like every other entry, and one comparison covers all three
  // positions.
  Padded := ';' + Existing + ';';
  Needle := ';' + Uppercase(Dir) + ';';
  P := Pos(Needle, Uppercase(Padded));
  if P = 0 then
    exit;

  // Keeps one of the two separators that surrounded the entry, so what was
  // either side of it stays joined.
  Updated := Copy(Padded, 1, P) + Copy(Padded, P + Length(Needle), MaxInt);
  if (Length(Updated) > 0) and (Updated[1] = ';') then
    Delete(Updated, 1, 1);
  if (Length(Updated) > 0) and (Updated[Length(Updated)] = ';') then
    Delete(Updated, Length(Updated), 1);

  RegWriteExpandStringValue(Root, Key, 'Path', Updated);
end;

// Uninstalling offers to take the settings with it, rather than deciding.
//
// The folder holds config.yaml and positions.json as well as the registry:
// every preference, and where you stopped in every video. Removing an
// application should not silently discard that, and uninstall-then-reinstall
// is a normal thing to try when something is wrong - the moment it would hurt
// most.
//
// No is the default button, so the destructive answer is never the one that
// arrives by pressing Enter. A silent uninstall never asks and never deletes,
// because there is nobody there to have been asked.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: string;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    // Unconditionally, rather than only when the task was ticked: the folder
    // is being deleted either way, and an entry pointing at nothing is worth
    // clearing however it got there. Does nothing when it is not present.
    RemoveFromPath;

    DataDir := ExpandConstant('{localappdata}\TinePlayer');
    if DirExists(DataDir) and (not UninstallSilent) then
    begin
      if MsgBox('Also delete your TinePlayer settings and saved playback positions?'
                + #13#10#13#10
                + 'Choose No if you are reinstalling or upgrading.',
                mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(DataDir, True, True, True);
    end;
    // Tidies the folder away when the registry was all that was in it, and
    // does nothing when anything remains.
    RemoveDir(DataDir);
  end;
end;

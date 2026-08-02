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
VersionInfoVersion={#AppVersion}

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

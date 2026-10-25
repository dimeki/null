; Inno Setup script for ytmusic-lite (https://jrsoftware.org/isinfo.php).
; build-package.ps1 runs it automatically when Inno Setup is installed.
; It installs for the current user only, so no administrator prompt is needed,
; and installing a newer version over an older one just replaces the files.

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\dist\ytmusic-lite-" + AppVersion + "-win64"
#endif

[Setup]
; keep this id the same in every release: it is how Windows knows a new version is an upgrade
AppId={{B0C4E3C0-6A57-4C2E-9D5B-3E5F7E1B8A11}
AppName=ytmusic-lite
AppVersion={#AppVersion}
AppPublisher=ytmusic-lite
DefaultDirName={localappdata}\Programs\ytmusic-lite
DefaultGroupName=ytmusic-lite
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=ytmusic-lite-setup-{#AppVersion}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\ytmusic-lite.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; the app lives in the tray, so ask Windows to close it before replacing its files
CloseApplications=yes
DisableProgramGroupPage=yes

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\ytmusic-lite"; Filename: "{app}\ytmusic-lite.exe"
Name: "{autodesktop}\ytmusic-lite"; Filename: "{app}\ytmusic-lite.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ytmusic-lite.exe"; Description: "Start ytmusic-lite"; Flags: nowait postinstall skipifsilent

[Code]
// The app keeps your queue, settings and Google sign-in under %LOCALAPPDATA%\ytmusic-lite.
// Uninstalling leaves them (so reinstalling keeps you signed in) unless you say otherwise.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    if MsgBox('Also delete your saved queue, settings and Google sign-in?', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(ExpandConstant('{localappdata}\ytmusic-lite'), True, True, True);
  end;
end;

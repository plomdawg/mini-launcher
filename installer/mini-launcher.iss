; Mini Launcher per-user installer (Inno Setup 6). No admin rights: installs to
; %LOCALAPPDATA%\Programs\Mini Launcher and writes only to HKCU.
;
; Build (see .github/workflows/release.yml):
;   ISCC.exe /DAppVersion=<cargo version> /DPayloadDir=<dir with both exes> /O<out dir> installer\mini-launcher.iss
; Relative paths below are relative to this file.

#define AppName "Mini Launcher"
#ifndef AppVersion
  #error Pass /DAppVersion=<version> (the [workspace.package] version in app/Cargo.toml)
#endif
#ifndef PayloadDir
  #define PayloadDir "..\app\target\dist"
#endif
#define TrayExe "mini-launcher-tray.exe"
#define RepoUrl "https://github.com/plomdawg/mini-launcher"
; Must match mini_launcher_core::autostart::VALUE_NAME.
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"
#define RunValue "Mini Launcher"

[Setup]
; Never change AppId: it identifies the install for upgrades and uninstall.
AppId={{2CF94C44-B086-4260-A67B-B6EEFF027C09}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=The Mini Launcher contributors
AppPublisherURL={#RepoUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
AppCopyright=Copyright (c) The Mini Launcher contributors
; Per-user only; no PrivilegesRequiredOverridesAllowed, so it never asks for admin.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; Named mutex held by a running tray app or `mini-launcher run` (core::instance::NAME):
; setup and the uninstaller ask the user to quit it first.
AppMutex=Local\mini-launcher
SetupIconFile=..\app\assets\icon\mini-launcher.ico
UninstallDisplayIcon={app}\{#TrayExe}
UninstallDisplayName={#AppName}
OutputBaseFilename=mini-launcher-{#AppVersion}-windows-x86_64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "autostart"; Description: "Start {#AppName} when I sign in"

[Files]
Source: "{#PayloadDir}\{#TrayExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\mini-launcher.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#TrayExe}"

[Registry]
; Same value the tray's "Start at login" writes (core::autostart::enable(tray exe, &[])):
; the quoted exe path, no arguments. Removed on uninstall by the [Code] below.
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "{#RunValue}"; ValueData: """{app}\{#TrayExe}"""; Tasks: autostart

[Run]
Filename: "{app}\{#TrayExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[Code]
// Remove the start-at-login value if it starts a program from this install
// (written by the task above, the tray's checkbox or `autostart enable`), but
// leave one that points at another copy. User config and logs are not touched.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Command: String;
begin
  if (CurUninstallStep = usUninstall) and
     RegQueryStringValue(HKCU, '{#RunKey}', '{#RunValue}', Command) and
     (Pos(Lowercase('"' + AddBackslash(ExpandConstant('{app}'))), Lowercase(Command)) = 1) then
    RegDeleteValue(HKCU, '{#RunKey}', '{#RunValue}');
end;

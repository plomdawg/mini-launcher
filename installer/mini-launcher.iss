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
; Must match mini_launcher_core::instance::NAME and QUIT_EVENT.
#define AppMutex "Local\mini-launcher"
#define QuitEvent "Local\mini-launcher-quit"

[Setup]
; Never change AppId: it identifies the install for upgrades and uninstall.
AppId={{2CF94C44-B086-4260-A67B-B6EEFF027C09}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=plomdawg
AppPublisherURL={#RepoUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
AppCopyright=Copyright (C) 2026 plomdawg and Mini Launcher contributors
; Per-user only; no PrivilegesRequiredOverridesAllowed, so it never asks for admin.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; Named mutex held by a running tray app or `mini-launcher run`. Setup and the
; uninstaller first ask a running tray app to quit (CloseRunningApp below); if it is
; still running after that, they ask the user to quit it.
AppMutex={#AppMutex}
SetupIconFile=..\app\assets\icon\mini-launcher.ico
UninstallDisplayIcon={app}\{#TrayExe}
UninstallDisplayName={#AppName}
OutputBaseFilename=mini-launcher-{#AppVersion}-windows-x86_64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Don't restore the task choice from the last install: on an upgrade the code below
; ticks "autostart" only if start at login is currently on (the tray toggle wins).
UsePreviousTasks=no

[Tasks]
Name: "autostart"; Description: "Start {#AppName} when I sign in"

[Files]
Source: "{#PayloadDir}\{#TrayExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\mini-launcher.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
; The settings window embeds Departure Mono; the OFL travels with it.
Source: "..\app\assets\fonts\DepartureMono-OFL.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#TrayExe}"

[Registry]
; Same value the tray's "Start at login" writes (core::autostart::enable(tray exe, &[])):
; the quoted exe path, no arguments. Removed on uninstall by the [Code] below.
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "{#RunValue}"; ValueData: """{app}\{#TrayExe}"""; Tasks: autostart

[Run]
Filename: "{app}\{#TrayExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent; Check: not ClosedRunningApp
; Setup closed the running tray app: start the new one (silent installs too).
Filename: "{app}\{#TrayExe}"; Flags: nowait; Check: ClosedRunningApp

[Code]
// True if the start-at-login value starts a program from Dir (written by the task
// above, the tray's checkbox or `autostart enable`, with or without arguments).
// A value pointing at another copy doesn't count.
function RunValueInDir(Dir: String): Boolean;
var
  Command: String;
begin
  Result := (Dir <> '') and
    RegQueryStringValue(HKCU, '{#RunKey}', '{#RunValue}', Command) and
    (Pos(Lowercase('"' + AddBackslash(Dir)), Lowercase(Command)) = 1);
end;

// True if /TASKS= or /MERGETASKS= on the command line decides the autostart task.
function AutostartFromCommandLine(): Boolean;
var
  I: Integer;
  Param: String;
begin
  Result := False;
  for I := 1 to ParamCount do begin
    Param := Lowercase(ParamStr(I));
    if (Pos('/tasks=', Param) = 1) or
       ((Pos('/mergetasks=', Param) = 1) and (Pos('autostart', Param) > 0)) then
      Result := True;
  end;
end;

var
  TasksDefaulted: Boolean;

// Upgrade: default the task to the current state, so re-running setup doesn't turn
// start at login back on after the user turned it off in the tray. Runs for
// silent installs too (they still step through the pages).
procedure CurPageChanged(CurPageID: Integer);
begin
  if (CurPageID = wpSelectTasks) and not TasksDefaulted then begin
    TasksDefaulted := True;
    if (WizardForm.PrevAppDir <> '') and not AutostartFromCommandLine() and
       not RunValueInDir(WizardForm.PrevAppDir) then
      WizardSelectTasks('!autostart');
  end;
end;

const
  EVENT_MODIFY_STATE = 2;

function OpenEvent(DesiredAccess: DWORD; InheritHandle: Integer; Name: String): THandle;
  external 'OpenEventW@kernel32.dll stdcall';
function SetEvent(Handle_: THandle): BOOL;
  external 'SetEvent@kernel32.dll stdcall';
function CloseHandle(Handle_: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

var
  ClosedApp, InstallDone: Boolean;
  PrevTrayExe: String;

// Asks a running tray app to quit (it listens on QuitEvent) and waits up to 10 s.
// The request repeats while waiting: the app does not quit while its Settings
// window is open (it may hold unsaved edits), but does once that is closed.
// True if it was running and has exited. `mini-launcher run` and older tray
// versions don't listen: the AppMutex prompt handles those.
function CloseRunningApp(): Boolean;
var
  QuitEvt: THandle;
  I: Integer;
begin
  Result := False;
  if not CheckForMutexes('{#AppMutex}') then
    Exit;
  QuitEvt := OpenEvent(EVENT_MODIFY_STATE, 0, '{#QuitEvent}');
  if QuitEvt = 0 then begin
    Log('Mini Launcher is running but does not listen for quit requests');
    Exit;
  end;
  Log('Asking the running Mini Launcher to quit');
  I := 0;
  while (I < 40) and not Result do begin
    SetEvent(QuitEvt);
    Sleep(250);
    Result := not CheckForMutexes('{#AppMutex}');
    I := I + 1;
  end;
  CloseHandle(QuitEvt);
  if Result then begin
    // The mutex goes just before the process ends: let it release its exe.
    Sleep(1000);
    Log('Mini Launcher quit');
  end
  else
    Log('Mini Launcher is still running (Settings open?)');
end;

// Runs before the AppMutex check, so that only prompts if this didn't work.
function InitializeSetup(): Boolean;
var
  Dir: String;
begin
  // The uninstall key of a previous install (AppId + '_is1').
  if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{2CF94C44-B086-4260-A67B-B6EEFF027C09}_is1',
     'InstallLocation', Dir) and (Dir <> '') then
    PrevTrayExe := AddBackslash(Dir) + '{#TrayExe}';
  ClosedApp := CloseRunningApp();
  Result := True;
end;

// [Run] check: start the app after install only if setup closed it.
function ClosedRunningApp(): Boolean;
begin
  Result := ClosedApp;
end;

// Setup closed the app but then didn't install (cancelled): start it again.
procedure DeinitializeSetup();
var
  ResultCode: Integer;
begin
  if ClosedApp and not InstallDone and (PrevTrayExe <> '') and FileExists(PrevTrayExe) then
    Exec(PrevTrayExe, '', '', SW_SHOWNORMAL, ewNoWait, ResultCode);
end;

// Task unticked: turn start at login off if it starts this install.
procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    InstallDone := True;
  if (CurStep = ssPostInstall) and not WizardIsTaskSelected('autostart') and
     RunValueInDir(ExpandConstant('{app}')) then
    RegDeleteValue(HKCU, '{#RunKey}', '{#RunValue}');
end;

// Uninstall: remove the start-at-login value if it starts this install, leave one
// that points at another copy. User config and logs are not touched.
// Before its AppMutex check, the uninstaller asks a running tray app to quit too.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usAppMutexCheck then
    CloseRunningApp();
  if (CurUninstallStep = usUninstall) and RunValueInDir(ExpandConstant('{app}')) then
    RegDeleteValue(HKCU, '{#RunKey}', '{#RunValue}');
end;

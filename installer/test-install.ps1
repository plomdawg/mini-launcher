# Silent install/uninstall check for the per-user setup.exe (run by release.yml;
# also usable by hand on a test machine, where it installs and uninstalls for the
# current user and leaves any existing config/logs in place).
#   pwsh installer/test-install.ps1 -Setup <setup.exe> -Version <x.y.z>
param(
    [Parameter(Mandatory)][string]$Setup,
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'

$dir = Join-Path $env:LOCALAPPDATA 'Programs\Mini Launcher'
$tray = Join-Path $dir 'mini-launcher-tray.exe'
$shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Mini Launcher.lnk'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{2CF94C44-B086-4260-A67B-B6EEFF027C09}_is1'
$files = 'mini-launcher-tray.exe', 'mini-launcher.exe', 'LICENSE-MIT', 'LICENSE-APACHE'
# User data lives outside the install dir and must survive uninstall.
$userFiles = @(
    (Join-Path $env:APPDATA 'mini-launcher\config\config.toml'),
    (Join-Path $env:LOCALAPPDATA 'mini-launcher\logs\install-test.log')
)

function Check([bool]$ok, [string]$what) {
    if (-not $ok) { throw "FAILED: $what" }
    Write-Host "ok: $what"
}
function RunValue { (Get-ItemProperty $runKey -ErrorAction SilentlyContinue).'Mini Launcher' }
function Invoke([string]$exe, [string[]]$arguments) {
    $p = Start-Process $exe -ArgumentList $arguments -Wait -PassThru
    Check ($p.ExitCode -eq 0) "$(Split-Path $exe -Leaf) $arguments exited with $($p.ExitCode)"
}

$created = @()
foreach ($f in $userFiles) {
    if (-not (Test-Path $f)) {
        New-Item -ItemType File -Force $f -Value 'user data' | Out-Null
        $created += $f
    }
}
$userHashes = $userFiles | ForEach-Object { (Get-FileHash $_).Hash }
Check (-not (Test-Path $dir)) "not installed yet at $dir"

# 1. Install without the start-at-login task.
Invoke $Setup '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/MERGETASKS=!autostart', '/LOG=install-notask.log'
foreach ($f in $files) { Check (Test-Path (Join-Path $dir $f)) "installed $f" }
Check ($null -eq (RunValue)) 'no Run value without the autostart task'
$lnk = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
Check ((Test-Path $shortcut) -and $lnk.TargetPath -eq $tray) "Start menu shortcut -> $($lnk.TargetPath)"
$entry = Get-ItemProperty $uninstallKey
Check ($entry.DisplayVersion -eq $Version) "HKCU uninstall entry, version $($entry.DisplayVersion)"

# 2. Reinstall over it with the task: same Run value core::autostart writes for the tray.
Invoke $Setup '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/TASKS=autostart', '/LOG=install-task.log'
Check ((RunValue) -eq "`"$tray`"") "Run value = $(RunValue)"

# 3. Silent uninstall. The uninstaller re-launches itself from %TEMP%, so poll.
Invoke (Join-Path $dir 'unins000.exe') '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/LOG=uninstall.log'
for ($i = 0; $i -lt 30 -and (Test-Path $dir); $i++) { Start-Sleep -Seconds 1 }
Check (-not (Test-Path $dir)) 'install dir removed'
Check (-not (Test-Path $shortcut)) 'Start menu shortcut removed'
Check ($null -eq (RunValue)) 'Run value removed'
Check (-not (Test-Path $uninstallKey)) 'uninstall entry removed'
for ($i = 0; $i -lt $userFiles.Count; $i++) {
    Check ((Test-Path $userFiles[$i]) -and (Get-FileHash $userFiles[$i]).Hash -eq $userHashes[$i]) "kept $($userFiles[$i])"
}
$created | Remove-Item

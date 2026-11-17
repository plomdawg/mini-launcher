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
function SetRunValue([string]$value) {
    if ([string]::IsNullOrEmpty($value)) { Remove-ItemProperty $runKey 'Mini Launcher' -ErrorAction SilentlyContinue }
    else { New-ItemProperty $runKey 'Mini Launcher' -Value $value -Force | Out-Null }
}
function Invoke([string]$exe, [string[]]$arguments) {
    $p = Start-Process $exe -ArgumentList $arguments -Wait -PassThru
    Check ($p.ExitCode -eq 0) "$(Split-Path $exe -Leaf) $arguments exited with $($p.ExitCode)"
}

# Uninstall, then wait for the files, the uninstall entry and (unless it should
# survive) the Run value to go: the uninstaller re-launches itself from %TEMP%.
function Uninstall([string]$log, [bool]$keepRunValue) {
    Invoke (Join-Path $dir 'unins000.exe') '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=$log"
    for ($i = 0; $i -lt 30; $i++) {
        if (-not (Test-Path $dir) -and -not (Test-Path $uninstallKey) -and
            ($keepRunValue -or $null -eq (RunValue))) { break }
        Start-Sleep -Seconds 1
    }
    Check (-not (Test-Path $dir)) 'install dir removed'
    Check (-not (Test-Path $shortcut)) 'Start menu shortcut removed'
    Check (-not (Test-Path $uninstallKey)) 'uninstall entry removed'
}

$quiet = '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
$trayValue = "`"$tray`""
# Restored at the end, for runs by hand.
$savedRunValue = RunValue
SetRunValue $null

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
Invoke $Setup ($quiet + '/MERGETASKS=!autostart', '/LOG=install-notask.log')
foreach ($f in $files) { Check (Test-Path (Join-Path $dir $f)) "installed $f" }
Check ($null -eq (RunValue)) 'no Run value without the autostart task'
$lnk = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
Check ((Test-Path $shortcut) -and $lnk.TargetPath -eq $tray) "Start menu shortcut -> $($lnk.TargetPath)"
$entry = Get-ItemProperty $uninstallKey
Check ($entry.DisplayVersion -eq $Version) "HKCU uninstall entry, version $($entry.DisplayVersion)"

# 2. Reinstall over it with the task: same Run value core::autostart writes for the tray.
Invoke $Setup ($quiet + '/TASKS=autostart', '/LOG=install-task.log')
Check ((RunValue) -eq $trayValue) "Run value = $(RunValue)"

# 3. Upgrades follow the current start-at-login state: kept while on ...
Invoke $Setup ($quiet + '/LOG=upgrade-on.log')
Check ((RunValue) -eq $trayValue) "upgrade keeps start at login on: $(RunValue)"
# ... turned off when the task is unticked ...
Invoke $Setup ($quiet + '/MERGETASKS=!autostart', '/LOG=upgrade-untick.log')
Check ($null -eq (RunValue)) 'unticking the task on upgrade removes the Run value'
# ... and not turned back on once off (as after unticking it in the tray).
Invoke $Setup ($quiet + '/LOG=upgrade-off.log')
Check ($null -eq (RunValue)) 'upgrade keeps start at login off'

# 4. Uninstall removes a CLI-style Run value (`autostart enable`) for this install.
SetRunValue "`"$(Join-Path $dir 'mini-launcher.exe')`" run"
Uninstall 'uninstall.log' $false
Check ($null -eq (RunValue)) 'CLI-style Run value removed'

# 5. ... but leaves one that starts another copy (a sibling dir must not match).
Invoke $Setup ($quiet + '/MERGETASKS=!autostart', '/LOG=install-again.log')
$foreign = "`"${dir}2\mini-launcher-tray.exe`""
SetRunValue $foreign
Uninstall 'uninstall-foreign.log' $true
Check ((RunValue) -eq $foreign) "Run value for another copy kept: $(RunValue)"

SetRunValue $savedRunValue
for ($i = 0; $i -lt $userFiles.Count; $i++) {
    Check ((Test-Path $userFiles[$i]) -and (Get-FileHash $userFiles[$i]).Hash -eq $userHashes[$i]) "kept $($userFiles[$i])"
}
$created | Remove-Item

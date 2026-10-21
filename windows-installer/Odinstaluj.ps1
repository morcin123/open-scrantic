# Removes the Johnny Castaway screensaver; the original files and story progress in %APPDATA%\OpenScrantic stay.
# Run through Odinstaluj.cmd. Messages are in Polish.

$ErrorActionPreference = 'Stop'
$systemDir = if ([Environment]::Is64BitProcess) { "$env:windir\System32" } else { "$env:windir\Sysnative" }
$scrTarget = Join-Path $systemDir 'JohnnyCastaway.scr'
$dataDir = Join-Path $env:APPDATA 'OpenScrantic'

Write-Host ''
Write-Host '=== Johnny Castaway - odinstalowanie ===' -ForegroundColor Cyan
Write-Host ''

$desktop = 'HKCU:\Control Panel\Desktop'
$current = (Get-ItemProperty -Path $desktop -Name 'SCRNSAVE.EXE' -ErrorAction SilentlyContinue).'SCRNSAVE.EXE'
if ($current -and $current -like '*JohnnyCastaway.scr') {
    Remove-ItemProperty -Path $desktop -Name 'SCRNSAVE.EXE'
    Write-Host 'Wyłączono Johnny Castaway jako wygaszacz ekranu.'
}

if (Test-Path -LiteralPath $scrTarget) {
    Write-Host "Usuwam $scrTarget (Windows zapyta o uprawnienia administratora)"
    $remove = "Remove-Item -LiteralPath '$($scrTarget.Replace("'", "''"))' -Force"
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remove))
    Start-Process powershell.exe -Verb RunAs -Wait -WindowStyle Hidden `
        -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $encoded)
}
if (Test-Path -LiteralPath $scrTarget) {
    Write-Host "Nie udało się usunąć $scrTarget" -ForegroundColor Red
} else {
    Write-Host 'Wygaszacz usunięty.' -ForegroundColor Green
}

Write-Host ''
Write-Host "Pliki gry i postęp historii zostały w: $dataDir"
Write-Host 'Jeśli ich nie potrzebujesz, usuń ten folder ręcznie.'
Write-Host ''

# Installs the Johnny Castaway screensaver (this fork's .scr) for the current user. Run through Zainstaluj.cmd.
# Messages are in Polish, for the user this package is for.

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$scrSource = Join-Path $here 'JohnnyCastaway.scr'
$dataSource = Join-Path $here 'dane'
$dataDir = Join-Path $env:APPDATA 'OpenScrantic'

# From a 32-bit PowerShell, System32 is redirected to SysWOW64.
$systemDir = if ([Environment]::Is64BitProcess) { "$env:windir\System32" } else { "$env:windir\Sysnative" }
$scrTarget = Join-Path $systemDir 'JohnnyCastaway.scr'
$scrRegistered = "$env:windir\System32\JohnnyCastaway.scr"

Write-Host ''
Write-Host '=== Johnny Castaway - instalacja wygaszacza ===' -ForegroundColor Cyan
Write-Host ''

foreach ($f in @($scrSource, (Join-Path $dataSource 'SCRANTIC.SCR'), (Join-Path $dataSource 'RESOURCE.MAP'), (Join-Path $dataSource 'RESOURCE.001'))) {
    if (-not (Test-Path -LiteralPath $f)) { throw "Brak pliku: $f" }
}

# Files unpacked from a downloaded .zip carry the mark of the web; clear it so Windows does not block the .scr.
Get-ChildItem -LiteralPath $here -Recurse -File | Unblock-File

# 1. The original files go to %APPDATA%\OpenScrantic, where the .scr looks for them and keeps SCRANTIC.INI.
Write-Host "1/4  Kopiuję pliki gry do $dataDir"
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
foreach ($name in 'SCRANTIC.SCR', 'RESOURCE.MAP', 'RESOURCE.001') {
    Copy-Item -LiteralPath (Join-Path $dataSource $name) -Destination (Join-Path $dataDir $name) -Force
}
$ini = Join-Path $dataDir 'SCRANTIC.INI'
if (-not (Test-Path -LiteralPath $ini)) {
    # No BOM: the screensaver reads the INI as plain UTF-8.
    [IO.File]::WriteAllText($ini, "[ScreenSaver.ScreenAntics]`r`nWeatherLocation=Warszawa`r`n")
    Write-Host '     Utworzono SCRANTIC.INI (pogoda: Warszawa)'
} else {
    Write-Host '     SCRANTIC.INI już istnieje - zostawiam Twoje ustawienia i postęp historii'
}

# 2. The .scr goes to System32, the only folder Windows lists screensavers from; that needs administrator rights.
Write-Host "2/4  Kopiuję wygaszacz do $scrTarget (Windows zapyta o uprawnienia administratora)"
$copy = "Copy-Item -LiteralPath '$($scrSource.Replace("'", "''"))' -Destination '$($scrTarget.Replace("'", "''"))' -Force"
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($copy))
try {
    $p = Start-Process powershell.exe -Verb RunAs -Wait -PassThru -WindowStyle Hidden `
        -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $encoded)
} catch {
    throw 'Anulowano zgodę administratora - wygaszacz nie został skopiowany do System32.'
}
if ($p.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $scrTarget)) {
    throw "Nie udało się skopiować wygaszacza do $scrTarget"
}
if ((Get-FileHash -LiteralPath $scrTarget).Hash -ne (Get-FileHash -LiteralPath $scrSource).Hash) {
    throw "Plik w $scrTarget różni się od kopiowanego - spróbuj ponownie"
}

# 3. Make it the active screensaver (5 minutes if no wait time was set).
Write-Host '3/4  Ustawiam Johnny Castaway jako wygaszacz ekranu'
$desktop = 'HKCU:\Control Panel\Desktop'
Set-ItemProperty -Path $desktop -Name 'SCRNSAVE.EXE' -Value $scrRegistered
Set-ItemProperty -Path $desktop -Name 'ScreenSaveActive' -Value '1'
$timeout = (Get-ItemProperty -Path $desktop -Name 'ScreenSaveTimeOut' -ErrorAction SilentlyContinue).ScreenSaveTimeOut
if (-not $timeout -or $timeout -eq '0') {
    Set-ItemProperty -Path $desktop -Name 'ScreenSaveTimeOut' -Value '300'
}

# 4. The Screen Saver Settings window: pressing OK there makes Windows apply the change at once.
Write-Host '4/4  Otwieram ustawienia wygaszacza ekranu'
Start-Process control.exe -ArgumentList 'desk.cpl,,@screensaver'

Write-Host ''
Write-Host 'Gotowe!' -ForegroundColor Green
Write-Host 'W oknie "Ustawienia wygaszacza ekranu" sprawdź, czy wybrany jest "Johnny Castaway", i kliknij OK.'
Write-Host 'Przycisk "Ustawienia..." otwiera opcje (dźwięk, zegar, pogoda, miejscowość), "Podgląd" uruchamia go od razu.'
Write-Host ''

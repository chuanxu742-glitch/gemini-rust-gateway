param(
    [switch]$SkipDoctor
)

$ErrorActionPreference = "Stop"
Set-Location -LiteralPath $PSScriptRoot

# Source builds dynamically link the MSYS2 libstdc++ runtime. Packaged GitHub
# Releases carry the DLL beside the executable; source checkouts use this PATH.
$msysRoots = @(
    $env:GEMINI_MSYS2_ROOT,
    $env:MSYS2_ROOT,
    (Join-Path $env:USERPROFILE ".cargo\msys64"),
    "C:\msys64"
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }
$runtimeRoot = $msysRoots | Where-Object {
    Test-Path -LiteralPath (Join-Path $_ "ucrt64\bin\libstdc++-6.dll")
} | Select-Object -First 1
if ($runtimeRoot) {
    $env:Path = "$(Join-Path $runtimeRoot 'ucrt64\bin');$env:Path"
}

$packaged = Join-Path $PSScriptRoot "gemini-rust-gateway.exe"
$built = Join-Path $PSScriptRoot "target\release\gemini-rust-gateway.exe"
if (Test-Path -LiteralPath $packaged) {
    $gateway = $packaged
} elseif (Test-Path -LiteralPath $built) {
    $gateway = $built
} else {
    Write-Host "No prebuilt executable found; building the release version..."
    & (Join-Path $PSScriptRoot "build.ps1") build -Release
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $gateway = $built
}

if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot ".env"))) {
    Write-Host "First run: creating local configuration."
    & $gateway init
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

if (-not $SkipDoctor) {
    & $gateway doctor --online
    if ($LASTEXITCODE -ne 0) {
        $syncEnabled = Select-String -LiteralPath (Join-Path $PSScriptRoot ".env") -Pattern '^GEMINI_COOKIE_SYNC_ENABLED=(true|1|yes|on)$' -Quiet
        if ($syncEnabled) {
            Write-Warning "Online cookie check failed. Starting degraded mode so the Chrome sync extension can refresh the session."
        } else {
            Write-Error "Configuration check failed. Fix the message above and run start.ps1 again."
            exit $LASTEXITCODE
        }
    }
}

& $gateway serve
exit $LASTEXITCODE

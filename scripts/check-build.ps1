$ErrorActionPreference = "Stop"

$rustTools = Join-Path $env:USERPROFILE ".cargo\bin"
$cargo = Get-Command cargo.exe -ErrorAction SilentlyContinue
if (-not $cargo -and (Test-Path -LiteralPath (Join-Path $rustTools "cargo.exe"))) {
    $cargo = Get-Item -LiteralPath (Join-Path $rustTools "cargo.exe")
}

$roots = @(
    $env:GEMINI_MSYS2_ROOT,
    $env:MSYS2_ROOT,
    (Join-Path $env:USERPROFILE ".cargo\msys64"),
    "C:\msys64"
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }
$msysRoot = $roots | Where-Object {
    Test-Path -LiteralPath (Join-Path $_ "ucrt64\bin\gcc.exe")
} | Select-Object -First 1

$checks = @(
    [pscustomobject]@{ Name = "Rust/Cargo"; Ready = [bool]$cargo; Hint = "Install Rust from https://rustup.rs" },
    [pscustomobject]@{ Name = "MSYS2 UCRT64 GCC"; Ready = [bool]$msysRoot; Hint = "Install MSYS2 UCRT64 or set GEMINI_MSYS2_ROOT" }
)
if ($msysRoot) {
    $checks += @(
        [pscustomobject]@{ Name = "CMake"; Ready = Test-Path (Join-Path $msysRoot "ucrt64\bin\cmake.exe"); Hint = "Install mingw-w64-ucrt-x86_64-cmake" },
        [pscustomobject]@{ Name = "Ninja"; Ready = Test-Path (Join-Path $msysRoot "ucrt64\bin\ninja.exe"); Hint = "Install mingw-w64-ucrt-x86_64-ninja" },
        [pscustomobject]@{ Name = "Clang/libclang"; Ready = Test-Path (Join-Path $msysRoot "ucrt64\bin\libclang.dll"); Hint = "Install mingw-w64-ucrt-x86_64-clang" },
        [pscustomobject]@{ Name = "Perl"; Ready = Test-Path (Join-Path $msysRoot "usr\bin\perl.exe"); Hint = "Install perl in MSYS2" }
    )
}

$checks | Select-Object Name,@{N="Status";E={if ($_.Ready) { "OK" } else { "MISSING" }}},Hint | Format-Table -AutoSize
if ($checks.Ready -contains $false) {
    Write-Error "Build prerequisites are incomplete. Prefer a prebuilt GitHub Release if you do not need to compile."
    exit 1
}
Write-Host "All Windows build prerequisites are ready."

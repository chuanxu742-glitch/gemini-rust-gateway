param(
    [ValidateSet("build", "test", "check", "run", "fmt", "clippy", "clean")]
    [string]$Command = "build",
    [switch]$Release,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$GatewayArgs = @()
)

$rustTools = Join-Path $env:USERPROFILE ".cargo\bin"
$cargoCommand = Get-Command cargo.exe -ErrorAction SilentlyContinue
$cargo = if ($cargoCommand) { $cargoCommand.Source } else { Join-Path $rustTools "cargo.exe" }

$msysRoots = @(
    $env:GEMINI_MSYS2_ROOT,
    $env:MSYS2_ROOT,
    (Join-Path $env:USERPROFILE ".cargo\msys64"),
    "C:\msys64"
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }
$msysRoot = $msysRoots | Where-Object {
    Test-Path -LiteralPath (Join-Path $_ "ucrt64\bin\gcc.exe")
} | Select-Object -First 1

if (-not (Test-Path -LiteralPath $cargo)) {
    throw "cargo.exe was not found. Install Rust with rustup first."
}
if (-not $msysRoot -and $Command -notin @("fmt", "clean")) {
    throw "MSYS2 UCRT64 GCC was not found. Install MSYS2 or set GEMINI_MSYS2_ROOT."
}

if ($msysRoot) {
    $ucrtTools = Join-Path $msysRoot "ucrt64\bin"
    $msysTools = Join-Path $msysRoot "usr\bin"
    $env:Path = "$ucrtTools;$msysTools;$rustTools;$env:Path"
    $env:CMAKE_GENERATOR = "Ninja"
    $env:HOST_CC = Join-Path $ucrtTools "gcc.exe"
    $env:HOST_CXX = Join-Path $ucrtTools "g++.exe"
    $env:LIBCLANG_PATH = $ucrtTools
}
Set-Location -LiteralPath $PSScriptRoot

[string[]]$arguments = switch ($Command) {
    "fmt" { @("fmt", "--check") }
    "clippy" { @("clippy", "--locked", "--all-targets", "--", "-D", "warnings") }
    default { @($Command) }
}
if ($Release -and $Command -in @("build", "run")) {
    $arguments += "--release"
}
if ($Command -eq "run" -and $GatewayArgs.Count -gt 0) {
    $arguments += "--"
    $arguments += $GatewayArgs
}

& $cargo @arguments
$cargoExitCode = $LASTEXITCODE
if ($cargoExitCode -eq 0 -and $IsWindows -and $Command -eq "build" -and $Release -and $msysRoot) {
    $releaseDir = Join-Path $PSScriptRoot "target\release"
    foreach ($runtimeName in @("libstdc++-6.dll", "libgcc_s_seh-1.dll", "libwinpthread-1.dll")) {
        $runtimePath = Join-Path $ucrtTools $runtimeName
        if (Test-Path -LiteralPath $runtimePath) {
            Copy-Item -LiteralPath $runtimePath -Destination $releaseDir -Force
        }
    }
}
exit $cargoExitCode

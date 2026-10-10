<#
.SYNOPSIS
  Build VELA: release exe -> portable zip -> Windows installer.

.EXAMPLE
  .\build.ps1            # build everything into .\dist
  .\build.ps1 -Setup     # first time: download the toolchain pieces into .\.tools
  .\build.ps1 -Run       # build and launch the release exe
#>
param(
  [switch]$Setup,
  [switch]$Run,
  [switch]$NoInstaller
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$tools = Join-Path $root '.tools'
$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value

# Pinned third-party builds (bump deliberately, then re-test)
$MpvDevUrl  = 'https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261010/mpv-dev-x86_64-20261010-git-b2c255c13e.7z'
$DevkitUrl  = 'https://github.com/skeeto/w64devkit/releases/download/v2.10.0/w64devkit-x64-2.10.0.7z.exe'

function Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
# Run a native tool; only its exit code decides success (Windows PowerShell 5.1
# would otherwise treat progress output on stderr as an error).
function Native([string]$what, [scriptblock]$cmd) {
  $prev = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try { & $cmd 2>&1 | ForEach-Object { "$_" } | Out-Host } finally { $ErrorActionPreference = $prev }
  if ($LASTEXITCODE) { throw "$what failed (exit $LASTEXITCODE)" }
}

if ($Setup) {
  New-Item -ItemType Directory -Force (Join-Path $tools 'dl') | Out-Null
  if (-not (Get-Command cargo -ErrorAction SilentlyContinue) -and -not (Test-Path "$env:USERPROFILE\.cargo\bin\cargo.exe")) {
    Step 'Installing Rust (GNU toolchain, user-level)'
    $ri = Join-Path $tools 'dl\rustup-init.exe'
    Invoke-WebRequest 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-gnu/rustup-init.exe' -OutFile $ri
    & $ri -y --default-host x86_64-pc-windows-gnu --default-toolchain stable --profile minimal
  }
  if (-not (Test-Path (Join-Path $tools 'mpv\libmpv-2.dll'))) {
    Step 'Downloading libmpv'
    $f = Join-Path $tools 'dl\mpv-dev.7z'
    Invoke-WebRequest $MpvDevUrl -OutFile $f
    New-Item -ItemType Directory -Force (Join-Path $tools 'mpv') | Out-Null
    tar -xf $f -C (Join-Path $tools 'mpv')
  }
  if (-not (Test-Path (Join-Path $tools 'asbin\as.exe'))) {
    Step 'Downloading assembler/dlltool (w64devkit)'
    $f = Join-Path $tools 'dl\w64devkit.7z.exe'
    Invoke-WebRequest $DevkitUrl -OutFile $f
    & $f -y "-o$tools" | Out-Null
    New-Item -ItemType Directory -Force (Join-Path $tools 'asbin') | Out-Null
    Copy-Item (Join-Path $tools 'w64devkit\bin\as.exe'), (Join-Path $tools 'w64devkit\bin\dlltool.exe') (Join-Path $tools 'asbin')
    # Only those two tools are needed; drop the rest of the ~700 MB kit.
    Remove-Item -Recurse -Force (Join-Path $tools 'w64devkit')
  }
  Remove-Item -Recurse -Force (Join-Path $tools 'dl') -ErrorAction SilentlyContinue
}

# Rust's GNU toolchain needs `as` + `dlltool` for raw-dylib imports; keep its own linker.
$env:PATH = "$(Join-Path $tools 'asbin');$env:USERPROFILE\.cargo\bin;$env:PATH"

Step "Building VELA $version (release)"
Push-Location $root
try { Native 'cargo build' { cargo build --release } } finally { Pop-Location }

$dist = Join-Path $root 'dist'
$stage = Join-Path $dist 'stage'
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item (Join-Path $root 'target\release\vela.exe') (Join-Path $stage 'Vela.exe')
Copy-Item (Join-Path $tools 'mpv\libmpv-2.dll') $stage
Copy-Item (Join-Path $root 'LICENSE') (Join-Path $stage 'LICENSE.txt')
Copy-Item (Join-Path $root 'installer\THIRD-PARTY-NOTICES.txt') $stage

# Optional code signing: set $env:VELA_SIGN to a command line containing {file}
# (signtool with a certificate, Azure Trusted Signing, ...). Unsigned builds
# work, but SmartScreen / Smart App Control will warn or block on other PCs.
function Sign([string]$file) {
  if (-not $env:VELA_SIGN) { return }
  Step "Signing $(Split-Path $file -Leaf)"
  Native 'code signing' { cmd /c ($env:VELA_SIGN -replace '\{file\}', "`"$file`"") }
}
Sign (Join-Path $stage 'Vela.exe')

if ($Run) { & (Join-Path $stage 'Vela.exe'); return }

Step 'Portable zip'
$zip = Join-Path $dist "VELA-$version-windows-portable.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal

if (-not $NoInstaller) {
  Step 'Installer'
  $makensis = @(
    (Get-Command makensis -ErrorAction SilentlyContinue).Source,
    "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
    "$env:ProgramFiles\NSIS\makensis.exe",
    "$env:LOCALAPPDATA\electron-builder\Cache\nsis\nsis-3.0.4.1\Bin\makensis.exe"
  ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
  if (-not $makensis) { throw 'NSIS not found. Install it (winget install NSIS.NSIS) or pass -NoInstaller.' }
  Native 'installer generation' { python (Join-Path $root 'scripts\gen_installer.py') }
  $out = Join-Path $dist "VELA-Setup-$version.exe"
  Native 'makensis' { & $makensis /V2 "/DVERSION=$version" "/DSTAGE=$stage" "/DOUTFILE=$out" (Join-Path $root 'installer\vela.nsi') }
}

Get-ChildItem $dist -File | Format-Table Name, @{n='MB';e={[math]::Round($_.Length/1MB,1)}} -AutoSize

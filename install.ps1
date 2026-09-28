# Install flightlog on Windows: https://github.com/skrcka/flightlog
#
#   irm https://flightlog.sh/install.ps1 | iex
#
# $env:FLIGHTLOG_VERSION = "0.1.0"   pin a version (default: latest release)
# $env:FLIGHTLOG_INSTALL_DIR = "…"   where the binary goes
#                                    (default: %LOCALAPPDATA%\flightlog\bin)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repo = 'skrcka/flightlog'
$version = if ($env:FLIGHTLOG_VERSION) { $env:FLIGHTLOG_VERSION } else { 'latest' }
$binDir = if ($env:FLIGHTLOG_INSTALL_DIR) { $env:FLIGHTLOG_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'flightlog\bin' }

$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ($arch) {
    'X64'   { $target = 'x86_64-pc-windows-msvc' }
    'Arm64' { $target = 'aarch64-pc-windows-msvc' }
    default { throw "flightlog: no prebuilt binary for Windows $arch; build from source: cargo install flightlog" }
}

if ($version -eq 'latest') {
    $base = "https://github.com/$repo/releases/latest/download"
} else {
    $base = "https://github.com/$repo/releases/download/v$($version.TrimStart('v'))"
}
$asset = "flightlog-$target.zip"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("flightlog-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "flightlog: downloading $asset ($version)"
    Invoke-WebRequest -UseBasicParsing "$base/$asset" -OutFile (Join-Path $tmp $asset)
    Invoke-WebRequest -UseBasicParsing "$base/flightlog-$target.sha256" -OutFile (Join-Path $tmp 'sha256')

    $expected = ((Get-Content (Join-Path $tmp 'sha256') -Raw).Trim() -split '\s+')[0].ToLower()
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $asset)).Hash.ToLower()
    if ($expected -ne $actual) { throw "flightlog: checksum mismatch for $asset" }

    Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
    Copy-Item (Join-Path $tmp 'flightlog.exe') (Join-Path $binDir 'flightlog.exe') -Force
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$exe = Join-Path $binDir 'flightlog.exe'
Write-Host "flightlog: installed $(& $exe --version) to $binDir"

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($userPath -split ';') -contains $binDir)) {
    [Environment]::SetEnvironmentVariable('Path', "$binDir;$userPath", 'User')
    $env:Path = "$binDir;$env:Path"
    Write-Host "flightlog: added $binDir to your user PATH (open a new terminal to pick it up)"
}

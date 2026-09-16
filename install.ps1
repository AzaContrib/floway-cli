# floway-cli installer for Windows PowerShell and PowerShell Core.
#
# Usage:
#   irm https://raw.githubusercontent.com/AzaContrib/floway-cli/main/install.ps1 | iex
#   & ./install.ps1 -Endpoint https://gw.example -ApiKey KEY -Agents all
#
# Downloads the floway binary for this platform from floway-cli GitHub
# Releases, verifies its sha256 checksum, installs it to ~/.local/bin (or
# $env:FLOWAY_CLI_INSTALL_DIR), and, when given credentials, runs
# `floway install` non-interactively.

[CmdletBinding()]
param(
  [string]$Version,
  [string]$Endpoint,
  [Alias('Key')]
  [string]$ApiKey,
  [string]$Agents,
  [string]$InstallDir,
  [switch]$NonInteractive,
  [switch]$Help
)

$ErrorActionPreference = 'Stop'

function Write-InstallerError {
  param([string]$Message)
  [Console]::Error.WriteLine("floway-cli installer: $Message")
  exit 1
}

# --- argument and flag normalization -----------------------------------------

$Repo = if ($env:FLOWAY_CLI_REPO) { $env:FLOWAY_CLI_REPO } else { 'AzaContrib/floway-cli' }

# Parse remaining raw arguments if passed (e.g. --endpoint https://... or -h)
$rawArgs = @($args)
$i = 0
while ($i -lt $rawArgs.Count) {
  $arg = [string]$rawArgs[$i]
  switch -Regex ($arg) {
    '^(--help|-h)$' {
      $Help = $true
      $i++
    }
    '^--version=(.+)$' {
      $Version = $Matches[1]
      $i++
    }
    '^--version$' {
      if ($i + 1 -lt $rawArgs.Count) { $Version = $rawArgs[$i + 1]; $i += 2 } else { $i++ }
    }
    '^--endpoint=(.+)$' {
      $Endpoint = $Matches[1]
      $i++
    }
    '^--endpoint$' {
      if ($i + 1 -lt $rawArgs.Count) { $Endpoint = $rawArgs[$i + 1]; $i += 2 } else { $i++ }
    }
    '^(--api-key|--key)=(.+)$' {
      $ApiKey = $Matches[2]
      $i++
    }
    '^(--api-key|--key)$' {
      if ($i + 1 -lt $rawArgs.Count) { $ApiKey = $rawArgs[$i + 1]; $i += 2 } else { $i++ }
    }
    '^--agents=(.+)$' {
      $Agents = $Matches[1]
      $i++
    }
    '^--agents$' {
      if ($i + 1 -lt $rawArgs.Count) { $Agents = $rawArgs[$i + 1]; $i += 2 } else { $i++ }
    }
    '^--install-dir=(.+)$' {
      $InstallDir = $Matches[1]
      $i++
    }
    '^--install-dir$' {
      if ($i + 1 -lt $rawArgs.Count) { $InstallDir = $rawArgs[$i + 1]; $i += 2 } else { $i++ }
    }
    '^--non-interactive$' {
      $NonInteractive = $true
      $i++
    }
    default {
      $i++
    }
  }
}

if ($Help) {
  Write-Host "floway-cli installer"
  Write-Host "Usage: install.ps1 [-Version <vX.Y.Z>] [-Endpoint <url>] [-ApiKey <key>] [-Agents <agents>] [-InstallDir <dir>] [-NonInteractive]"
  exit 0
}

if (-not $Version -and $env:FLOWAY_CLI_VERSION) { $Version = $env:FLOWAY_CLI_VERSION }
if (-not $Endpoint -and $env:SETUP_ENDPOINT) { $Endpoint = $env:SETUP_ENDPOINT }
if (-not $ApiKey -and $env:SETUP_API_KEY) { $ApiKey = $env:SETUP_API_KEY }
if (-not $Agents -and $env:FLOWAY_AGENTS) { $Agents = $env:FLOWAY_AGENTS }
if (-not $Agents) { $Agents = 'all' }

# --- platform detection -----------------------------------------------------

function Test-IsWindows {
  ($PSVersionTable.PSVersion.Major -lt 6) -or $IsWindows
}

$runningOnWindows = Test-IsWindows
$runningOnMacOS = if ($runningOnWindows) { $false } else { [bool]$IsMacOS }
$runningOnLinux = if ($runningOnWindows -or $runningOnMacOS) { $false } else { [bool]$IsLinux }

if ($runningOnWindows) {
  $osPart = 'pc-windows-msvc'
  $binName = 'floway.exe'
} elseif ($runningOnMacOS) {
  $osPart = 'apple-darwin'
  $binName = 'floway'
} elseif ($runningOnLinux) {
  $osPart = 'unknown-linux-musl'
  $binName = 'floway'
} else {
  Write-InstallerError "unsupported operating system."
}

$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLower()
switch ($arch) {
  'x64' { $archPart = 'x86_64' }
  'arm64' {
    if ($runningOnWindows) {
      # Windows on ARM runs x64 binaries via built-in emulation.
      $archPart = 'x86_64'
    } else {
      $archPart = 'aarch64'
    }
  }
  default {
    Write-InstallerError "unsupported architecture: $arch"
  }
}

$Target = "$archPart-$osPart"

# --- install directory ------------------------------------------------------

if (-not $InstallDir) {
  if ($env:FLOWAY_CLI_INSTALL_DIR) {
    $InstallDir = $env:FLOWAY_CLI_INSTALL_DIR
  } else {
    $homeDir = if ($env:USERPROFILE) { $env:USERPROFILE } elseif ($HOME) { $HOME } else { '.' }
    $InstallDir = Join-Path $homeDir '.local/bin'
  }
}

# --- resolve version --------------------------------------------------------

if ($Version) {
  if ($Version -notmatch '^v[0-9]+') {
    Write-InstallerError "invalid version format: $Version (expected v[0-9]*, e.g. v0.1.0)"
  }
} else {
  try {
    $releaseApi = "https://api.github.com/repos/$Repo/releases/latest"
    $headers = @{ 'User-Agent' = 'floway-cli-installer' }
    $release = Invoke-RestMethod -Uri $releaseApi -Headers $headers -TimeoutSec 30
    if ($release -and $release.tag_name) {
      $Version = [string]$release.tag_name
    }
  } catch {
    # Fallback to resolving redirect of /releases/latest
    try {
      $latestUrl = "https://github.com/$Repo/releases/latest"
      $req = [System.Net.WebRequest]::Create($latestUrl)
      $req.AllowAutoRedirect = $false
      $resp = $req.GetResponse()
      $location = $resp.GetResponseHeader('Location')
      $resp.Close()
      if ($location -match '/tag/(v[0-9].*)$') {
        $Version = $Matches[1]
      }
    } catch { }
  }

  if (-not $Version) {
    Write-InstallerError "could not determine the latest release tag for $Repo"
  }
}

# --- download and verify ----------------------------------------------------

$artifact = "floway-$Target.tar.gz"
$baseUrl = "https://github.com/$Repo/releases/download/$Version"
$downloadUrl = "$baseUrl/$artifact"
$checksumUrl = "$downloadUrl.sha256"

$tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("floway-install." + [System.Guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

try {
  $artifactPath = Join-Path $tmpDir $artifact
  Write-Host "Downloading floway $Version for $Target…"
  Invoke-WebRequest -Uri $downloadUrl -OutFile $artifactPath -UseBasicParsing -TimeoutSec 120

  # Checksum verification if sidecar exists
  $checksumPath = Join-Path $tmpDir "$artifact.sha256"
  $hasChecksum = $false
  try {
    Invoke-WebRequest -Uri $checksumUrl -OutFile $checksumPath -UseBasicParsing -TimeoutSec 30
    if ((Test-Path -LiteralPath $checksumPath) -and (Get-Item -LiteralPath $checksumPath).Length -gt 0) {
      $hasChecksum = $true
    }
  } catch { }

  if ($hasChecksum) {
    $checksumContent = (Get-Content -Raw -LiteralPath $checksumPath).Trim()
    $expectedHash = ($checksumContent -split '\s+')[0].ToLower()
    if ($expectedHash -match '^[0-9a-f]{64}$') {
      $actualHash = (Get-FileHash -LiteralPath $artifactPath -Algorithm SHA256).Hash.ToLower()
      if ($actualHash -ne $expectedHash) {
        Write-InstallerError "checksum mismatch for $artifact (expected $expectedHash, got $actualHash)"
      }
      Write-Host "Checksum verified."
    }
  }

  # --- extract --------------------------------------------------------------

  $tarCmd = Get-Command tar -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($tarCmd) {
    & $tarCmd.Source -xzf $artifactPath -C $tmpDir
    if ($LASTEXITCODE -ne 0) {
      Write-InstallerError "could not extract the archive $artifact"
    }
  } else {
    # Fallback to .NET extraction if tar is unavailable
    try {
      $fileStream = [System.IO.File]::OpenRead($artifactPath)
      $gzipStream = New-Object System.IO.Compression.GZipStream($fileStream, [System.IO.Compression.CompressionMode]::Decompress)
      [System.Formats.Tar.TarFile]::ExtractToDirectory($gzipStream, $tmpDir, $true)
      $gzipStream.Dispose()
      $fileStream.Dispose()
    } catch {
      Write-InstallerError "tar command was not found to extract $artifact"
    }
  }

  $extractedDir = Join-Path $tmpDir "floway-$Target"
  $extractedExe = Join-Path $extractedDir $binName
  if (-not (Test-Path -LiteralPath $extractedExe)) {
    # Check directly inside $tmpDir in case directory structure differs
    $extractedExe = Join-Path $tmpDir $binName
  }
  if (-not (Test-Path -LiteralPath $extractedExe)) {
    Write-InstallerError "the archive did not contain a $binName binary"
  }

  # --- install --------------------------------------------------------------

  if (-not (Test-Path -LiteralPath $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
  }

  $targetExe = Join-Path $InstallDir $binName
  Copy-Item -LiteralPath $extractedExe -Destination $targetExe -Force

  if (-not $runningOnWindows) {
    & chmod 755 $targetExe
  }

  # Check PATH
  $pathSeparator = [System.IO.Path]::PathSeparator
  $paths = ($env:PATH -split [regex]::Escape([string]$pathSeparator))
  $onPath = $false
  foreach ($p in $paths) {
    if ($p.TrimEnd('\', '/') -eq $InstallDir.TrimEnd('\', '/')) {
      $onPath = $true
      break
    }
  }
  if (-not $onPath) {
    [Console]::Error.WriteLine("NOTE: $InstallDir is not on your PATH.")
    # Add to current session PATH
    $env:PATH = "$InstallDir$pathSeparator$env:PATH"
  }

  $installedPath = Join-Path $InstallDir $binName
  Write-Host "Installed $installedPath ($Version)."

} finally {
  if (Test-Path -LiteralPath $tmpDir) {
    Remove-Item -LiteralPath $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
  }
}

# --- optional setup ---------------------------------------------------------

if ($Endpoint -and $ApiKey) {
  & $targetExe install --endpoint $Endpoint --api-key $ApiKey --agents $Agents --non-interactive
  exit $LASTEXITCODE
} elseif ($env:SETUP_ENDPOINT -and $env:SETUP_API_KEY) {
  $ep = $env:SETUP_ENDPOINT
  $key = $env:SETUP_API_KEY
  Remove-Item Env:SETUP_API_KEY -ErrorAction SilentlyContinue
  Remove-Item Env:SETUP_ENDPOINT -ErrorAction SilentlyContinue
  & $targetExe install --endpoint $ep --api-key $key --agents $Agents --non-interactive
  exit $LASTEXITCODE
} else {
  Write-Host "Run ``floway install`` to configure your agents."
}

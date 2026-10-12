# Native Windows bootstrap; GitHub Releases are authoritative, no Node/npm.
param(
    [Parameter(Mandatory = $true)][string]$Relay,
    [string]$Token,
    [string]$Code,
    [string]$WebOrigin,
    [ValidateRange(1, 65535)][int]$Port = 8787
)
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
if (([bool]$Token) -eq ([bool]$Code)) { throw 'Provide exactly one device token or setup code.' }
if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64' -and $env:PROCESSOR_ARCHITEW6432 -ne 'AMD64') { throw 'The Windows runtime currently requires x64.' }
$repo = 'https://github.com/dufangshi/pockymoe'
$stage = Join-Path ([IO.Path]::GetTempPath()) ('pockymoe-bootstrap-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    Write-Host 'Checking the latest Pockymoe GitHub release...'
    $version = (Invoke-WebRequest -UseBasicParsing -Uri "$repo/releases/latest/download/runtime-version.txt" -TimeoutSec 30).Content.Trim()
    if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') { throw 'GitHub returned an invalid runtime version.' }
    $asset = 'remote-codex-win32-x64-msvc-cli.exe'
    $base = "$repo/releases/download/v$version"
    $sums = (Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -TimeoutSec 30).Content
    $entries = @($sums -split "`n" | Where-Object { $_ -match "^([0-9a-fA-F]{64})\s+\*?$([regex]::Escape($asset))\s*$" })
    if ($entries.Count -ne 1) { throw 'Missing or ambiguous native runtime checksum.' }
    $expected = ($entries[0] -split '\s+')[0].ToLowerInvariant()
    $binary = Join-Path $stage 'pockymoe.exe'
    Write-Host "Downloading Pockymoe $version (Windows/x64)..."
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $binary -TimeoutSec 300
    if ((Get-FileHash -Algorithm SHA256 -Path $binary).Hash.ToLowerInvariant() -ne $expected) { throw 'Runtime checksum verification failed. Nothing has been installed.' }
    $actual = & $binary version
    if ($LASTEXITCODE -ne 0 -or $actual.Trim() -ne $version) { throw 'Downloaded runtime version verification failed.' }
    $arguments = @('setup', '--relay', $Relay, '--port', "$Port")
    if ($WebOrigin) { $arguments += @('--web-origin', $WebOrigin) }
    if ($Token) { $arguments += @('--token', $Token) } else { $arguments += @('--code', $Code) }
    & $binary @arguments
    if ($LASTEXITCODE -ne 0) { throw "Native setup failed (exit $LASTEXITCODE)." }
} finally {
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}

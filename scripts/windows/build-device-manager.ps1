param([string]$Configuration = 'Release', [string]$Runtime = 'win-x64', [string]$OutputPath, [switch]$SkipPreview)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
if (-not $OutputPath) { $OutputPath = Join-Path $root 'artifacts\windows-device-manager\win-x64' }
$project = Join-Path $root 'apps\windows-device-manager\RemoteCodex.DeviceManager.csproj'
& dotnet publish $project -c $Configuration -r $Runtime --self-contained true -o $OutputPath -p:PublishSingleFile=true
if ($LASTEXITCODE -ne 0) { throw 'Device Manager publish failed.' }
$exe = Join-Path $OutputPath 'RemoteCodex.DeviceManager.exe'
[xml]$metadata = Get-Content $project
$version = [string]$metadata.Project.PropertyGroup.Version
if ([Diagnostics.FileVersionInfo]::GetVersionInfo($exe).ProductVersion.Split('+')[0] -ne $version) { throw 'Manager EXE version mismatch.' }
$profile = Join-Path ([IO.Path]::GetTempPath()) ('pockymoe-manager-test-' + [Guid]::NewGuid().ToString('N'))
$old = $env:POCKYMOE_DEVICE_MANAGER_TEST_ROOT
try {
    $env:POCKYMOE_DEVICE_MANAGER_TEST_ROOT = $profile
    $checks = @(@('--self-test'))
    if (-not $SkipPreview) { $checks += ,@('--render-preview', (Join-Path $OutputPath 'previews\manager.png')) }
    foreach ($arguments in $checks) {
        $p = Start-Process -FilePath $exe -ArgumentList $arguments -Wait -PassThru
        if ($p.ExitCode -ne 0) {
            $log = Join-Path $profile 'local-app-data\RemoteCodex\logs\device-manager.log'
            if (Test-Path $log) { Get-Content $log -Tail 30 }
            throw "Device Manager verification failed: $($arguments[0])"
        }
    }
} finally {
    $env:POCKYMOE_DEVICE_MANAGER_TEST_ROOT = $old
    Remove-Item -LiteralPath $profile -Recurse -Force -ErrorAction SilentlyContinue
}
$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $exe).Hash.ToLowerInvariant()
"$hash  RemoteCodex.DeviceManager.exe" | Set-Content -LiteralPath "$exe.sha256" -Encoding ascii
Write-Host "Verified: $exe"

use std::path::Path;

// Windows PowerShell 5.1 treats redirected native stderr as a PowerShell
// error. Keep setup failures strict, but do not terminate the native worker on
// ordinary tracing output; its exit code remains authoritative.
pub fn source(payload: &Path) -> String {
    let payload = payload.to_string_lossy().replace('\'', "''");
    format!("$ErrorActionPreference='Stop'\n$p=Get-Content -LiteralPath '{payload}' -Raw | ConvertFrom-Json\nRemove-Item -LiteralPath '{payload}', $PSCommandPath -Force\nRemove-Item Env:POCKYMOE_ENROLL_SECRET -ErrorAction SilentlyContinue\nforeach($v in $p.environment.PSObject.Properties){{[Environment]::SetEnvironmentVariable($v.Name,[string]$v.Value,'Process')}}\n$a=@($p.args)\n$ErrorActionPreference='Continue'\n& $p.binary @a *> $p.log\nexit $LASTEXITCODE\n")
}

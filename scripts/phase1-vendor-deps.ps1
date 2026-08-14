# POE-Alarm Phase 1: vendor cargo dependencies so Claude's sandbox can build offline.
# Run AFTER phase0-snapshot-and-cleanup.ps1:
#   powershell -ExecutionPolicy Bypass -File D:\Projects\POE-Alarm\scripts\phase1-vendor-deps.ps1
$ErrorActionPreference = "Continue"
Set-Location "D:\Projects\POE-Alarm\rust"

# Keep the huge vendor dir out of git.
$gi = Get-Content ..\.gitignore -Raw
if ($gi -notmatch [regex]::Escape("rust/vendor/")) {
    Add-Content ..\.gitignore "rust/vendor/"
    Add-Content ..\.gitignore "rust/.cargo-vendor-config.toml"
    Write-Host "added rust/vendor/ to .gitignore"
}

Write-Host "== cargo vendor (downloads all deps incl. gpui-component; takes a few minutes) ==" -ForegroundColor Cyan
cargo vendor vendor | Out-File -Encoding utf8 .cargo-vendor-config.toml
if ($LASTEXITCODE -ne 0) {
    Write-Host "cargo vendor FAILED - copy the error above back to Claude." -ForegroundColor Red
    exit 1
}

Write-Host "== packing vendor + lockfile ==" -ForegroundColor Cyan
New-Item -ItemType Directory -Force ..\artifacts | Out-Null
tar -czf ..\artifacts\rust-vendor.tar.gz vendor .cargo-vendor-config.toml Cargo.lock
if ($LASTEXITCODE -ne 0) {
    Write-Host "tar FAILED - copy the error above back to Claude." -ForegroundColor Red
    exit 1
}
$size = (Get-Item ..\artifacts\rust-vendor.tar.gz).Length / 1MB
Write-Host ("Done: artifacts\rust-vendor.tar.gz ({0:N0} MB). Tell Claude to continue." -f $size) -ForegroundColor Green

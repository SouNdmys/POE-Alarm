# POE-Alarm Phase 0 (v2): snapshot rust work, then retire the .NET implementation.
# Every git step is verified; the script stops loudly on the first failure.
# Run:  powershell -ExecutionPolicy Bypass -File D:\Projects\POE-Alarm\scripts\phase0-snapshot-and-cleanup.ps1
$ErrorActionPreference = "Continue"
Set-Location "D:\Projects\POE-Alarm"

function Step($name, [scriptblock]$block) {
    Write-Host ""
    Write-Host "== $name ==" -ForegroundColor Cyan
    & $block
    if ($LASTEXITCODE -ne 0) {
        Write-Host ""
        Write-Host "FAILED at step: $name (exit $LASTEXITCODE). Nothing after this step was run." -ForegroundColor Red
        Write-Host "Copy the error above back to Claude." -ForegroundColor Red
        exit 1
    }
}

# 0. Ensure a commit identity exists (repo-local; matches existing history).
$email = git config user.email
if (-not $email) {
    Write-Host "No git identity found - setting repo-local identity (SouNdmys / soundmys1994@gmail.com)"
    git config user.name  "SouNdmys"
    git config user.email "soundmys1994@gmail.com"
}

Step "1/6 stage everything" { git add -A }

Step "2/6 commit snapshot"  { git commit -m "chore: snapshot rust workspace and Ledger frontend design spec before GPUI rebuild" }

Step "3/6 push branch"      { git push -u origin codex/rust-native-migration }

Step "4/6 remove .NET implementation" {
    git rm -r -q src tools PoeAlarm.slnx global.json Directory.Build.props
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    git rm -r -q tests/PoeAlarm.Core.Tests tests/PoeAlarm.GuardedPolicy.Tests tests/PoeAlarm.MirrorCorpusProbe tests/PoeAlarm.Poe2CorpusProbe tests/PoeAlarm.Rules.Tests tests/PoeAlarm.RulesUi.Tests
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    git rm -q licenses/DotNet-LICENSE.txt licenses/DotNet-ThirdPartyNotices.txt
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    git rm -q "README-*.txt"
}

Step "5/6 commit removal"   { git commit -m "chore: retire .NET implementation; Rust workspace is the sole codebase going forward" }

Step "6/6 push"             { git push }

Write-Host ""
Write-Host "== cleanup local-only .NET toolchain dirs ==" -ForegroundColor Cyan
Remove-Item -Recurse -Force .dotnet-cli, .dotnet-home, .packages, .tools -ErrorAction SilentlyContinue

Write-Host ""
git log --oneline -3
Write-Host ""
Write-Host "Phase 0 done: the two commits above should be new. Tell Claude to continue." -ForegroundColor Green

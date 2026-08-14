# POE-Alarm Phase 0b: finish the .NET removal (steps 4-6 of phase0).
# Run:  powershell -ExecutionPolicy Bypass -File D:\Projects\POE-Alarm\scripts\phase0b-remove-dotnet.ps1
$ErrorActionPreference = "Continue"
Set-Location "D:\Projects\POE-Alarm"

function Step($name, [scriptblock]$block) {
    Write-Host ""
    Write-Host "== $name ==" -ForegroundColor Cyan
    & $block
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED at step: $name (exit $LASTEXITCODE). Copy the error above back to Claude." -ForegroundColor Red
        exit 1
    }
}

# keep the scratch dir out of git
$gi = Get-Content .gitignore -Raw
if ($gi -notmatch [regex]::Escape("_to_delete/")) { Add-Content .gitignore "_to_delete/" }

Step "1/4 git rm .NET implementation (tolerant)" {
    git rm -r -q --ignore-unmatch src tools PoeAlarm.slnx global.json Directory.Build.props `
        tests/PoeAlarm.Core.Tests tests/PoeAlarm.GuardedPolicy.Tests tests/PoeAlarm.MirrorCorpusProbe `
        tests/PoeAlarm.Poe2CorpusProbe tests/PoeAlarm.Rules.Tests tests/PoeAlarm.RulesUi.Tests `
        licenses/DotNet-LICENSE.txt licenses/DotNet-ThirdPartyNotices.txt "README-*.txt" _to_delete
}

Step "2/4 commit" {
    git add .gitignore
    git commit -m "chore: retire .NET implementation; Rust workspace is the sole codebase going forward"
}

Step "3/4 push" { git push }

Write-Host ""
Write-Host "== 4/4 delete untracked .NET leftovers on disk (keeps tests/fixtures + tests/screenshots) ==" -ForegroundColor Cyan
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue `
    src, tools, PoeAlarm.slnx, global.json, Directory.Build.props, `
    tests\PoeAlarm.Core.Tests, tests\PoeAlarm.GuardedPolicy.Tests, tests\PoeAlarm.MirrorCorpusProbe, `
    tests\PoeAlarm.Poe2CorpusProbe, tests\PoeAlarm.Rules.Tests, tests\PoeAlarm.RulesUi.Tests, `
    .dotnet-cli, .dotnet-home, .packages, .tools, _to_delete

Write-Host ""
git log --oneline -3
Write-Host ""
Write-Host "Phase 0 fully done. Tell Claude." -ForegroundColor Green

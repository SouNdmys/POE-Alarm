[CmdletBinding()]
param(
    [string] $Version = '0.1.0',
    [string] $ExecutablePath = 'rust/target/release/poe-alarm-app.exe',
    [Parameter(Mandatory = $true)]
    [string] $VcRedistDirectory,
    [string] $OutputRoot = 'artifacts/rust-preview',
    [string] $ManifestToolPath,
    [string] $DumpbinPath,
    [switch] $SkipBuild,
    [switch] $SkipExecutableSelfTest,
    [long] $MaximumUnpackedBytes = 12582912,
    [long] $MaximumZipBytes = 6291456
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$VcFiles = @('msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')
$AllowedLicenses = @(
    'MIT', 'MIT OR Apache-2.0', 'Apache-2.0 OR MIT', 'MIT/Apache-2.0',
    'Unlicense OR MIT', 'ISC', 'Zlib OR Apache-2.0 OR MIT',
    'MIT OR Apache-2.0 OR Zlib', '(MIT OR Apache-2.0) AND Unicode-3.0'
)

function Resolve-ExistingFile([string] $Path, [string] $Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "$Label not found: $Path" }
    return (Resolve-Path -LiteralPath $Path).Path
}

function Resolve-ExistingDirectory([string] $Path, [string] $Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { throw "$Label not found: $Path" }
    return (Resolve-Path -LiteralPath $Path).Path
}

function Assert-FileHash([string] $Path, [long] $Bytes, [string] $Sha256, [string] $Label) {
    $item = Get-Item -LiteralPath $Path
    if ($item.Length -ne $Bytes) { throw "$Label byte count is $($item.Length); expected $Bytes" }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    if ($actual -ne $Sha256) { throw "$Label SHA-256 is $actual; expected $Sha256" }
}

function Find-WindowsSdkTool([string] $Name) {
    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
    if (Test-Path -LiteralPath $kits) {
        $candidate = Get-ChildItem -LiteralPath $kits -Filter $Name -File -Recurse |
            Where-Object { $_.FullName -match '\\x64\\' } |
            Sort-Object FullName -Descending | Select-Object -First 1
        if ($candidate) { return $candidate.FullName }
    }
    throw "$Name was not found; pass its path explicitly"
}

function Find-Dumpbin {
    $command = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $roots = @(
        (Join-Path $env:ProgramFiles 'Microsoft Visual Studio'),
        (Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio')
    )
    foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root)) { continue }
        $candidate = Get-ChildItem -LiteralPath $root -Filter dumpbin.exe -File -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\Hostx64\\x64\\' } |
            Sort-Object FullName -Descending | Select-Object -First 1
        if ($candidate) { return $candidate.FullName }
    }
    throw 'dumpbin.exe was not found; pass -DumpbinPath explicitly'
}

function Get-NormalCargoPackages([string] $RepositoryRoot) {
    $cargoLines = & cargo tree --manifest-path (Join-Path $RepositoryRoot 'rust/Cargo.toml') `
        -p poe-alarm-app --target x86_64-pc-windows-msvc -e normal --prefix none `
        --format '{p}|{l}|{r}'
    if ($LASTEXITCODE -ne 0) { throw 'cargo tree failed' }
    $packages = foreach ($line in $cargoLines) {
        $fields = $line -split '\|', 3
        if ($fields.Count -ne 3 -or $fields[0] -like 'poe-alarm-*') { continue }
        $package = $fields[0] -replace ' \(proc-macro\)$', '' -replace ' \(\*\)$', ''
        if ($package -notmatch '^(?<name>\S+) v(?<version>\S+)$') {
            throw "could not parse Cargo package line: $line"
        }
        $repository = $fields[2] -replace ' \(\*\)$', ''
        [pscustomobject]@{ Name=$Matches.name; Version=$Matches.version; License=$fields[1]; Repository=$repository }
    }
    return @($packages | Sort-Object Name,Version -Unique)
}

function Copy-CargoLicenses([object[]] $Packages, [string] $Destination) {
    $metadata = (& cargo metadata --manifest-path (Join-Path $script:RepositoryRoot 'rust/Cargo.toml') `
        --format-version 1 --filter-platform x86_64-pc-windows-msvc | ConvertFrom-Json)
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    New-Item -ItemType Directory -Path $Destination | Out-Null
    foreach ($package in $Packages) {
        if ($AllowedLicenses -notcontains $package.License) {
            throw "unreviewed license expression for $($package.Name) $($package.Version): $($package.License)"
        }
        $metadataPackage = $metadata.packages | Where-Object {
            $_.name -eq $package.Name -and $_.version -eq $package.Version -and $_.source
        } | Select-Object -First 1
        if (-not $metadataPackage) { throw "Cargo metadata missing $($package.Name) $($package.Version)" }
        $crateDirectory = Split-Path -Parent $metadataPackage.manifest_path
        $licenseFiles = Get-ChildItem -LiteralPath $crateDirectory -File | Where-Object {
            $_.Name -match '^(LICENSE|COPYING|UNLICENSE)([-._].*)?$'
        } | Sort-Object Name
        if (-not $licenseFiles) { throw "no upstream license file found for $($package.Name) $($package.Version)" }
        foreach ($license in $licenseFiles) {
            $targetName = "$($package.Name)-$($package.Version)-$($license.Name)"
            Copy-Item -LiteralPath $license.FullName -Destination (Join-Path $Destination $targetName)
        }
    }
}

function Add-DeterministicZip([string] $SourceDirectory, [string] $ZipPath, [string] $TopDirectory) {
    Add-Type -AssemblyName System.IO.Compression
    if (Test-Path -LiteralPath $ZipPath) { Remove-Item -LiteralPath $ZipPath -Force }
    $stream = [System.IO.File]::Open($ZipPath, [System.IO.FileMode]::CreateNew)
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $stream, [System.IO.Compression.ZipArchiveMode]::Create, $false)
        try {
            $fixedTime = [DateTimeOffset]::new(2000, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
            Get-ChildItem -LiteralPath $SourceDirectory -File -Recurse | Sort-Object FullName | ForEach-Object {
                $relative = [System.IO.Path]::GetRelativePath($SourceDirectory, $_.FullName).Replace('\', '/')
                $entry = $archive.CreateEntry("$TopDirectory/$relative", [System.IO.Compression.CompressionLevel]::Optimal)
                $entry.LastWriteTime = $fixedTime
                $input = [System.IO.File]::OpenRead($_.FullName)
                try {
                    $output = $entry.Open()
                    try { $input.CopyTo($output) } finally { $output.Dispose() }
                } finally { $input.Dispose() }
            }
        } finally { $archive.Dispose() }
    } finally { $stream.Dispose() }
}

$script:RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
Push-Location $RepositoryRoot
try {
    if ($Version -ne '0.1.0') { throw 'this script is intentionally pinned to Rust Preview 0.1.0' }
    if (-not $SkipBuild) {
        & cargo build --manifest-path rust/Cargo.toml -p poe-alarm-app --release --locked
        if ($LASTEXITCODE -ne 0) { throw 'release build failed' }
    }

    $exe = Resolve-ExistingFile $ExecutablePath 'release executable'
    $vcDirectory = Resolve-ExistingDirectory $VcRedistDirectory 'VC Redistributable directory'
    if ($vcDirectory -notmatch '(?i)\\VC\\Redist\\MSVC\\[^\\]+\\x64\\Microsoft\.VC\d+\.CRT$') {
        throw 'VC runtime source must be an official x64 Visual Studio VC/Redist/MSVC/.../Microsoft.VC*.CRT directory'
    }
    if ($vcDirectory -match '(?i)\\Windows\\System32($|\\)') { throw 'System32 is not a redistributable source' }


    $vcItems = foreach ($name in $VcFiles) {
        $path = Resolve-ExistingFile (Join-Path $vcDirectory $name) "VC runtime $name"
        $item = Get-Item -LiteralPath $path
        $signature = Get-AuthenticodeSignature -LiteralPath $path
        if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'Microsoft') {
            throw "$name is not validly signed by Microsoft"
        }
        if ($item.VersionInfo.CompanyName -notmatch '^Microsoft Corporation') {
            throw "$name has unexpected company metadata: $($item.VersionInfo.CompanyName)"
        }
        [pscustomobject]@{
            Name=$name; Path=$path; Bytes=$item.Length; Version=$item.VersionInfo.FileVersion;
            Sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash;
            Signer=$signature.SignerCertificate.Subject
        }
    }
    if (@($vcItems.Version | Sort-Object -Unique).Count -ne 1) { throw 'VC runtime DLL versions do not match' }

    $versionInfo = (Get-Item -LiteralPath $exe).VersionInfo
    if ($versionInfo.ProductName -ne 'POE Alarm - Rust Preview' -or $versionInfo.ProductVersion -ne $Version) {
        throw "EXE is not the expected Rust Preview $Version resource build"
    }
    Add-Type -AssemblyName System.Drawing.Common -ErrorAction SilentlyContinue
    $associatedIcon = [System.Drawing.Icon]::ExtractAssociatedIcon($exe)
    if (-not $associatedIcon -or $associatedIcon.Width -lt 16) { throw 'EXE has no usable embedded icon' }
    $associatedIcon.Dispose()

    if (-not $ManifestToolPath) { $ManifestToolPath = Find-WindowsSdkTool 'mt.exe' }
    $ManifestToolPath = Resolve-ExistingFile $ManifestToolPath 'mt.exe'
    $manifestScratch = Join-Path ([System.IO.Path]::GetTempPath()) "poe-alarm-$PID.manifest"
    try {
        & $ManifestToolPath "-inputresource:$exe;#1" "-out:$manifestScratch" | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'could not extract embedded application manifest' }
        $manifestText = Get-Content -LiteralPath $manifestScratch -Raw
        foreach ($required in @('PerMonitorV2', 'longPathAware', '4f476546-937c-4985-931b-35a169c20a36', 'asInvoker')) {
            if ($manifestText -notmatch [regex]::Escape($required)) { throw "embedded manifest lacks $required" }
        }
    } finally {
        if (Test-Path -LiteralPath $manifestScratch) { Remove-Item -LiteralPath $manifestScratch -Force }
    }

    if (-not $DumpbinPath) { $DumpbinPath = Find-Dumpbin }
    $DumpbinPath = Resolve-ExistingFile $DumpbinPath 'dumpbin.exe'
    $imports = & $DumpbinPath /dependents $exe
    if ($LASTEXITCODE -ne 0) { throw 'dumpbin dependency audit failed' }
    $importsText = $imports -join "`n"
    foreach ($required in $VcFiles) {
        if ($importsText -notmatch "(?im)^\s*$([regex]::Escape($required))\s*$") {
            throw "PE dependency audit did not find required $required"
        }
    }
    if ($importsText -match '(?i)(hostfxr|coreclr|webview2|tauri|node\.dll|python\d*\.dll)') {
        throw 'forbidden managed/web/Python runtime import detected'
    }

    $packages = Get-NormalCargoPackages $RepositoryRoot
    $outputCandidate = if ([System.IO.Path]::IsPathRooted($OutputRoot)) {
        $OutputRoot
    } else {
        Join-Path $RepositoryRoot $OutputRoot
    }
    $outputRootResolved = [System.IO.Path]::GetFullPath($outputCandidate)
    if ($outputRootResolved.TrimEnd('\') -eq $RepositoryRoot.TrimEnd('\')) { throw 'output root cannot be the repository root' }
    New-Item -ItemType Directory -Path $outputRootResolved -Force | Out-Null
    $packageName = "POE-Alarm-Rust-Preview-$Version-win-x64"
    $stage = Join-Path $outputRootResolved $packageName
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Path $stage | Out-Null
    $licenseDirectory = Join-Path $stage 'licenses'
    New-Item -ItemType Directory -Path $licenseDirectory | Out-Null

    Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'PoeAlarm.exe')
    foreach ($vc in $vcItems) { Copy-Item -LiteralPath $vc.Path -Destination (Join-Path $stage $vc.Name) }
    Copy-Item -LiteralPath rust/packaging/licenses/POE-Alarm-MIT.txt -Destination $licenseDirectory
    Copy-CargoLicenses $packages (Join-Path $licenseDirectory 'rust')

    $crateRows = $packages | ForEach-Object { "- ``$($_.Name) $($_.Version)`` — $($_.License) — $($_.Repository)" }
    $notice = (Get-Content -LiteralPath rust/packaging/THIRD-PARTY-NOTICES.template.md -Raw).
        Replace('<!-- RUST_CRATE_LIST -->', ($crateRows -join "`r`n"))
    Set-Content -LiteralPath (Join-Path $stage 'THIRD-PARTY-NOTICES.md') -Value $notice -Encoding utf8NoBOM

    $vcSourceLabel = ($vcDirectory -replace '^.*(?=\\VC\\Redist\\MSVC\\)', '').Replace('\', '/')
    $vcProvenance = @(
        'Microsoft Visual C++ Runtime redistribution provenance',
        "Source: official Visual Studio $vcSourceLabel",
        'Redistribution terms: https://aka.ms/vs/18/redistribution',
        '',
        ($vcItems | ForEach-Object { "$($_.Name) | $($_.Version) | $($_.Bytes) bytes | SHA256 $($_.Sha256) | $($_.Signer)" })
    )
    Set-Content -LiteralPath (Join-Path $licenseDirectory 'Microsoft-Visual-Cpp-Runtime-PROVENANCE.txt') `
        -Value $vcProvenance -Encoding utf8NoBOM

    $allowedTopFiles = @(
        'PoeAlarm.exe',
        'msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll',
        'THIRD-PARTY-NOTICES.md'
    )
    $topFiles = Get-ChildItem -LiteralPath $stage -File | Select-Object -ExpandProperty Name
    $unexpected = @($topFiles | Where-Object { $allowedTopFiles -notcontains $_ })
    $missing = @($allowedTopFiles | Where-Object { $topFiles -notcontains $_ })
    if ($unexpected -or $missing) { throw "package allowlist mismatch; unexpected=[$unexpected], missing=[$missing]" }
    $topDirectories = @(Get-ChildItem -LiteralPath $stage -Directory | Select-Object -ExpandProperty Name)
    if ($topDirectories.Count -ne 1 -or $topDirectories[0] -ne 'licenses') {
        throw "package directory allowlist mismatch: [$topDirectories]"
    }
    $licenseDirectories = @(Get-ChildItem -LiteralPath $licenseDirectory -Directory | Select-Object -ExpandProperty Name)
    if ($licenseDirectories.Count -ne 1 -or $licenseDirectories[0] -ne 'rust') {
        throw "license directory allowlist mismatch: [$licenseDirectories]"
    }
    $forbidden = Get-ChildItem -LiteralPath $stage -File -Recurse | Where-Object {
        $_.Name -match '(?i)(\.pdb$|\.lib$|\.exp$|\.deps\.json$|\.runtimeconfig\.json$|hostfxr|coreclr|webview2|tauri|node|python)'
    }
    if ($forbidden) { throw "forbidden release file: $($forbidden.FullName -join ', ')" }

    $payloadFiles = Get-ChildItem -LiteralPath $stage -File -Recurse | Sort-Object FullName
    $unpackedBytes = ($payloadFiles | Measure-Object Length -Sum).Sum
    if ($unpackedBytes -gt $MaximumUnpackedBytes) { throw "unpacked payload is $unpackedBytes bytes; gate is $MaximumUnpackedBytes" }

    $manifest = [ordered]@{
        product='POE Alarm - Rust Preview'; version=$Version; target='win-x64';
        generatedUtc='2000-01-01T00:00:00Z'; payloadBytesBeforeManifests=$unpackedBytes;
        rustCrates=$packages; vcRuntime=$vcItems | Select-Object Name,Version,Bytes,Sha256;
        files=Get-ChildItem -LiteralPath $stage -File -Recurse | Sort-Object FullName | ForEach-Object {
            [ordered]@{ path=[System.IO.Path]::GetRelativePath($stage,$_.FullName).Replace('\','/'); bytes=$_.Length; sha256=(Get-FileHash $_.FullName -Algorithm SHA256).Hash }
        }
    }
    Set-Content -LiteralPath (Join-Path $stage 'PACKAGE-MANIFEST.json') `
        -Value ($manifest | ConvertTo-Json -Depth 6) -Encoding utf8NoBOM

    $hashLines = Get-ChildItem -LiteralPath $stage -File -Recurse | Sort-Object FullName | ForEach-Object {
        $relative = [System.IO.Path]::GetRelativePath($stage, $_.FullName).Replace('\', '/')
        "$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash)  $relative"
    }
    Set-Content -LiteralPath (Join-Path $stage 'SHA256SUMS.txt') -Value $hashLines -Encoding ascii

    $finalUnpackedBytes = (Get-ChildItem -LiteralPath $stage -File -Recurse | Measure-Object Length -Sum).Sum
    if ($finalUnpackedBytes -gt $MaximumUnpackedBytes) {
        throw "final unpacked package is $finalUnpackedBytes bytes; gate is $MaximumUnpackedBytes"
    }
    if (-not $SkipExecutableSelfTest) {
        $selfTest = [System.Diagnostics.Process]::Start((Join-Path $stage 'PoeAlarm.exe'), '--self-test')
        try {
            if (-not $selfTest.WaitForExit(15000)) {
                $selfTest.Kill($true)
                throw 'packaged executable self-test did not exit within 15 seconds'
            }
            if ($selfTest.ExitCode -ne 0) { throw "packaged executable self-test exited with $($selfTest.ExitCode)" }
        } finally {
            $selfTest.Dispose()
        }
    }

    $zip = Join-Path $outputRootResolved "$packageName.zip"
    Add-DeterministicZip $stage $zip $packageName
    $zipBytes = (Get-Item -LiteralPath $zip).Length
    if ($zipBytes -gt $MaximumZipBytes) { throw "ZIP is $zipBytes bytes; gate is $MaximumZipBytes" }

    [pscustomobject]@{
        Stage=$stage; Zip=$zip; Files=(Get-ChildItem $stage -File -Recurse).Count;
        UnpackedBytes=$finalUnpackedBytes;
        ZipBytes=$zipBytes; ZipSha256=(Get-FileHash $zip -Algorithm SHA256).Hash
    } | Format-List
} finally {
    Pop-Location
}

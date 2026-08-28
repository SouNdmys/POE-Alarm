[CmdletBinding()]
param(
    [string] $Version = '1.0.3',
    [string] $ExecutablePath = 'rust/target/release/poe-alarm-app.exe',
    [Parameter(Mandatory = $true)]
    [string] $VcRedistDirectory,
    [string] $OutputRoot = 'artifacts/release',
    [string] $ManifestToolPath,
    [string] $DumpbinPath,
    [switch] $SkipBuild,
    [switch] $SkipExecutableSelfTest,
    # ~11 MB executable plus ~4 MB of upstream license texts for 530 crates.
    [long] $MaximumUnpackedBytes = 20971520,
    [long] $MaximumZipBytes = 10485760
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# The executable imports exactly one of these. msvcp140*, vcruntime140_1 and
# concrt140 came in with onnxruntime.dll, which is no longer shipped; carrying
# them anyway would be four megabytes of DLLs nothing loads.
$VcFiles = @('vcruntime140.dll')
$script:MissingLicenseFiles = @()
# Set only on the build path, read unconditionally in `finally`. Strict mode rejects reading an
# unset variable, so -SkipBuild would throw on the way out without this.
$script:SavedBuildEnv = $null
# Reviewed license identifiers, not whole expressions. The GPUI dependency
# graph resolves to 32 distinct SPDX expressions and grows every time a crate is
# updated; matching them literally meant a new formatting variant of a license
# already approved ('MIT/Apache-2.0' vs 'MIT OR Apache-2.0') failed the build,
# and the fix was always to paste the new string in — which is not review.
#
# Every identifier below is permissive and redistributable with attribution.
# What matters is what is NOT here: no GPL, LGPL, AGPL, MPL, CDDL, EPL, SSPL or
# any other reciprocal license. One of those appearing in the graph must stop
# the release, and with this list it does.
$AllowedLicenseIds = @(
    '0BSD', 'Apache-2.0', 'BSD-2-Clause', 'BSD-3-Clause', 'BSL-1.0', 'CC0-1.0',
    'ISC', 'MIT', 'MIT-0', 'Unicode-3.0', 'Unlicense', 'Zlib',
    'LLVM-exception'
)

# Splits an SPDX expression into the identifiers it names.
function Get-LicenseIdentifiers([string] $Expression) {
    if (-not $Expression) { return @() }
    $Expression -split '(?i)\s+(?:OR|AND|WITH)\s+|[()/]' |
        ForEach-Object { $_.Trim() } |
        Where-Object { $_ }
}

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
        if (-not $package.License) {
            throw "no license expression for $($package.Name) $($package.Version)"
        }
        $unreviewed = @(Get-LicenseIdentifiers $package.License |
            Where-Object { $AllowedLicenseIds -notcontains $_ })
        if ($unreviewed) {
            throw "unreviewed license for $($package.Name) $($package.Version): $($package.License) (unknown: $($unreviewed -join ', '))"
        }
        $metadataPackage = $metadata.packages | Where-Object {
            $_.name -eq $package.Name -and $_.version -eq $package.Version -and $_.source
        } | Select-Object -First 1
        if (-not $metadataPackage) { throw "Cargo metadata missing $($package.Name) $($package.Version)" }
        $crateDirectory = Split-Path -Parent $metadataPackage.manifest_path
        $licenseFiles = Get-ChildItem -LiteralPath $crateDirectory -File | Where-Object {
            $_.Name -match '^(LICENSE|COPYING|UNLICENSE)([-._].*)?$'
        } | Sort-Object Name
        if (-not $licenseFiles) {
            # The crate declares a license but did not publish the text with it.
            # That is upstream sloppiness rather than a licensing problem — the
            # declaration in Cargo.toml is the grant — but it must not vanish:
            # recorded here and written into the package so the gap is
            # auditable instead of invisible.
            $script:MissingLicenseFiles += "$($package.Name) $($package.Version) — declared $($package.License), no license file published"
            continue
        }
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
    if ($Version -ne '1.0.3') { throw 'this script is intentionally pinned to 1.0.3' }
    if (-not $SkipBuild) {
        # Rust records file!() for every dependency, so an unremapped release build ships several
        # hundred absolute paths rooted at the builder's home directory. The remaps are computed
        # from the environment rather than written down, so they stay correct on any machine.
        # CARGO_ENCODED_RUSTFLAGS, not RUSTFLAGS: the latter splits on whitespace and these are paths.
        $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
        $rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
        # The cc crate splits CFLAGS on whitespace unless CC_SHELL_ESCAPED_FLAGS is set, and a
        # quoted value ending in a backslash is its own trap. Refusing up front beats letting
        # cl.exe fail on a half-path, or worse, silently skipping the trim so the gate below
        # blames the Rust flags for a C problem.
        if ($cargoHome -match '\s') {
            throw "CARGO_HOME contains a space ('$cargoHome'); the C compiler flag cannot be passed safely. Set CARGO_HOME to a path without spaces for release builds."
        }
        $script:SavedBuildEnv = @{
            CARGO_ENCODED_RUSTFLAGS = $env:CARGO_ENCODED_RUSTFLAGS
            CFLAGS                  = $env:CFLAGS
            CXXFLAGS                = $env:CXXFLAGS
        }
        $env:CARGO_ENCODED_RUSTFLAGS = @(
            "--remap-path-prefix=$cargoHome=/cargo"
            "--remap-path-prefix=$rustupHome=/rustup"
            "--remap-path-prefix=$RepositoryRoot=/poe-alarm"
        ) -join [char]0x1f
        # --remap-path-prefix is a rustc flag and does not reach the C sources the cc crate builds
        # (tree-sitter, pulled in by gpui, bakes __FILE__ into its assertion messages as UTF-16).
        # /d1trimfile is MSVC's equivalent. Note this is graph-wide, not tree-sitter-only: every
        # cc-built crate here (ring, psm, stacker, tree-sitter*, vswhom-sys, embed-resource) gets
        # it, and cc treats a set CFLAGS as a signal to stop adding its own warning defaults.
        $env:CFLAGS = "/d1trimfile:$cargoHome\registry\src\"
        $env:CXXFLAGS = $env:CFLAGS
        & cargo build --manifest-path rust/Cargo.toml -p poe-alarm-app --release --locked
        if ($LASTEXITCODE -ne 0) { throw 'release build failed' }
    }

    $exe = Resolve-ExistingFile $ExecutablePath 'release executable'

    # Read the shipped bytes back rather than trusting that the remaps above were applied — a stale
    # -SkipBuild, an edited flag or a future toolchain change would all pass silently otherwise.
    # Three passes, because the leak this was written for was UTF-16: Rust emits path metadata as
    # ASCII, the resource compiler as UTF-16LE, and a UTF-16 string starting at an odd byte offset
    # is invisible to a decode that begins at zero.
    # The version resource's "(c) SouNd" is deliberate authorship, so only path shapes are rejected.
    $exeBytes = [System.IO.File]::ReadAllBytes($exe)
    $exeText = @(
        [System.Text.Encoding]::ASCII.GetString($exeBytes)
        [System.Text.Encoding]::Unicode.GetString($exeBytes)
        [System.Text.Encoding]::Unicode.GetString($exeBytes, 1, $exeBytes.Length - 1)
    )
    foreach ($needle in @(':\Users\', $RepositoryRoot, '.cargo\registry', '.rustup\toolchains')) {
        foreach ($text in $exeText) {
            if ($text.Contains($needle)) {
                $why = if ($SkipBuild) {
                    'the build was skipped, so nothing remapped these paths — rerun without -SkipBuild'
                } else {
                    'the --remap-path-prefix / /d1trimfile flags did not take'
                }
                throw "the built executable still embeds '$needle' — $why"
            }
        }
    }
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
    if ($versionInfo.ProductName -ne 'POE Alarm' -or $versionInfo.ProductVersion -ne $Version) {
        throw "EXE is not the expected POE Alarm $Version resource build; got '$($versionInfo.ProductName)' $($versionInfo.ProductVersion)"
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
        # What the GPUI build actually embeds. longPathAware, an explicit
        # asInvoker and a supportedOS entry are worth adding, but winresource
        # and link.exe both claim resource 1 type 24 and cvtres rejects the
        # duplicate (CVT1100), so that is a separate piece of work. Elevation is
        # not requested either way: no requestedExecutionLevel means asInvoker,
        # and the app only ever elevates through an explicit user action.
        foreach ($required in @('PerMonitorV2', 'Microsoft.Windows.Common-Controls')) {
            if ($manifestText -notmatch [regex]::Escape($required)) { throw "embedded manifest lacks $required" }
        }
        if ($manifestText -match 'requireAdministrator|highestAvailable') {
            throw 'the embedded manifest asks for elevation at launch'
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
    $packageName = "POE-Alarm-$Version-win-x64"
    $stage = Join-Path $outputRootResolved $packageName
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Path $stage | Out-Null
    $licenseDirectory = Join-Path $stage 'licenses'
    New-Item -ItemType Directory -Path $licenseDirectory | Out-Null

    Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'PoeAlarm.exe')
    foreach ($vc in $vcItems) { Copy-Item -LiteralPath $vc.Path -Destination (Join-Path $stage $vc.Name) }
    # The project's own licence, and the ONLY statement of it a downloader sees, so it has to match
    # LICENSE.md at the repo root. Through 1.0.1 this shipped MIT by accident: the MIT file predated
    # LICENSE.md and nobody updated the copy when the project settled on PolyForm Noncommercial.
    Copy-Item -LiteralPath LICENSE.md -Destination (Join-Path $licenseDirectory 'POE-Alarm-LICENSE.md')
    Copy-CargoLicenses $packages (Join-Path $licenseDirectory 'rust')
    if ($script:MissingLicenseFiles) {
        $gap = @(
            'Crates that declare a license but publish no license file.',
            '',
            'Their grant is the license expression in their own Cargo.toml, reproduced in',
            'THIRD-PARTY-NOTICES.md beside every other crate. Listed separately so the gap',
            'is visible rather than silently absent from this directory.',
            ''
        ) + ($script:MissingLicenseFiles | Sort-Object)
        Set-Content -LiteralPath (Join-Path $licenseDirectory 'rust/NO-UPSTREAM-LICENSE-FILE.txt') `
            -Value $gap -Encoding utf8NoBOM
    }

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

    $allowedTopFiles = @('PoeAlarm.exe') + $VcFiles + @('THIRD-PARTY-NOTICES.md')
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
        product='POE Alarm'; version=$Version; target='win-x64';
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
    # Restore rather than leak: these are process-scoped, and a .ps1 run in an interactive
    # session would otherwise leave every later cargo build in that window rebuilding the
    # whole graph with remapped paths and cc's warning defaults switched off.
    if ($script:SavedBuildEnv) {
        $env:CARGO_ENCODED_RUSTFLAGS = $script:SavedBuildEnv.CARGO_ENCODED_RUSTFLAGS
        $env:CFLAGS = $script:SavedBuildEnv.CFLAGS
        $env:CXXFLAGS = $script:SavedBuildEnv.CXXFLAGS
    }
    Pop-Location
}

# Native Windows preview packaging

`package-release.ps1` builds and assembles one clean Windows x64 ZIP. It intentionally
requires an official Visual Studio VC Redistributable directory instead of copying DLLs from
`System32` or silently depending on a machine-wide runtime installation.

Example with Visual Studio 18 Preview/Insiders installed:

```powershell
.\rust\packaging\package-release.ps1 `
  -VcRedistDirectory 'C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Redist\MSVC\14.50.35710\x64\Microsoft.VC145.CRT'
```

Inputs for the EXE, the VC redistributable directory, output root, `mt.exe` and `dumpbin.exe`
are parameters; repository-relative defaults are provided where the repository already owns the
input. The script verifies Microsoft signatures, PE imports and resources, the runtime Cargo
licence graph, an exact file allowlist, and unpacked/ZIP size gates. It also reads the finished
executable back and refuses to package if any build-machine path survived the remapping. It
writes `PACKAGE-MANIFEST.json` and `SHA256SUMS.txt` before producing a timestamp-normalized ZIP.

The default gates are 20 MiB unpacked and 10 MiB zipped, and the script is pinned to the version
it was last released with, so bumping a release is a deliberate edit rather than a flag.

After packaging, the following smoke test starts the real executable with an empty isolated user
profile and a minimal system `PATH`, finds the real configuration window, closes it normally, and
verifies that the released .NET settings file was not changed:

```powershell
.\rust\packaging\test-rust-preview-isolated.ps1
```

This checks side-by-side native loading, settings isolation, and graceful startup/shutdown on the
current Windows installation. It deliberately does not claim to replace a clean-Windows field test.

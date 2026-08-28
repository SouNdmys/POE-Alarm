# Third-Party Notices

The source tree bundles no third-party binaries. Every dependency is a Rust crate
resolved by Cargo from `rust/Cargo.lock`.

The release archive is a different matter, and its notices are generated at packaging
time rather than maintained here: `rust/packaging/package-release.ps1` copies each
crate's own upstream licence file into `licenses/rust/` and fails the build if a crate
declares a licence but ships no text for it, or declares one that has not been reviewed.
The archive also carries `vcruntime140.dll`, taken only from an official Microsoft
Visual Studio redistributable directory, with its source, version, signature and hashes
recorded in `licenses/Microsoft-Visual-Cpp-Runtime-PROVENANCE.txt`.

POE Alarm has not bundled PaddleOCR, the ONNX Runtime, or the .NET desktop runtime
since the OCR pipeline was removed. Affixes are read from the item text the game client
itself writes to the clipboard, so there is no recognition model to ship.

All product names and trademarks are the property of their respective owners.

# POE Alarm Rust Preview — Third-Party Notices

This archive is the native Rust preview of POE Alarm. It does not contain the .NET desktop
runtime, Tauri, WebView, Node.js, Python, a machine-learning runtime, or any recognition
model. Affixes are read from the item text the game client itself writes to the clipboard,
so there is nothing to recognise and nothing to ship for it.

## Microsoft Visual C++ runtime

The four Microsoft-signed runtime DLLs beside the executable are copied only from an official
Visual Studio `VC/Redist/MSVC/.../x64/Microsoft.VC*.CRT` directory supplied to the packaging
script. Exact source, version, signatures, and hashes are recorded in
`licenses/Microsoft-Visual-Cpp-Runtime-PROVENANCE.txt`. Microsoft's current redistribution
terms are published at https://aka.ms/vs/18/redistribution.

## Rust crates

The list below is generated from Cargo's actual normal dependency graph for the Windows x64
application. Exact upstream license files are copied to `licenses/rust/`; the packaging step
fails if a resolved registry crate has no license file or an unreviewed license expression.

<!-- RUST_CRATE_LIST -->

All product names and trademarks are the property of their respective owners.

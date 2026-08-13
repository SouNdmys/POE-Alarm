# POE Alarm Rust Preview — Third-Party Notices

This archive is the native Rust preview of POE Alarm. It does not contain the .NET desktop
runtime, Tauri, WebView, Node.js, Python, or the Paddle framework.

## PP-OCRv5 mobile recognition model and dictionary

- Project: PaddleOCR / PaddlePaddle
- Source: https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_rec_onnx
- License: Apache License 2.0
- Included license: `licenses/PaddlePaddle-Apache-2.0.txt`

The model and dictionary are used locally for bounded OCR verification. POE Alarm is not
affiliated with or endorsed by PaddlePaddle.

## ONNX Runtime 1.28.0

- Project: Microsoft ONNX Runtime
- Source: https://github.com/microsoft/onnxruntime
- License: MIT, with upstream third-party notices
- Included files: `licenses/ONNX-Runtime-MIT.txt` and
  `licenses/ONNX-Runtime-ThirdPartyNotices.txt`

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

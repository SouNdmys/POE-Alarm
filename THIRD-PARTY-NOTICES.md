# Third-Party Notices

POE Alarm includes or depends on the following third-party components.
Complete license and third-party notice texts are stored in the repository's `licenses`
directory and are included beside the executable in the release archive.

## PaddleOCR / PP-OCRv5 mobile recognition model

- Project: PaddleOCR, Copyright PaddlePaddle Authors
- Model: PP-OCRv5 mobile recognition ONNX
- Source: https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_rec_onnx
- License: Apache License 2.0
- License text: https://www.apache.org/licenses/LICENSE-2.0
- Included copy: `licenses/PaddlePaddle-Apache-2.0.txt`

The packaged model and character dictionary are used locally for targeted verification and as
an offline compatibility recognizer when Windows Chinese OCR is unavailable. POE Alarm is not
affiliated with or endorsed by PaddlePaddle.

## Microsoft.ML.OnnxRuntime

- Project: ONNX Runtime, Copyright Microsoft Corporation
- Source: https://github.com/microsoft/onnxruntime
- License: MIT License
- License text: https://github.com/microsoft/onnxruntime/blob/main/LICENSE
- Included copies: `licenses/ONNX-Runtime-MIT.txt` and
  `licenses/ONNX-Runtime-ThirdPartyNotices.txt`

## .NET Windows Desktop Runtime

- Project: .NET, Copyright Microsoft Corporation and contributors
- Source: https://github.com/dotnet/runtime
- Included copies: `licenses/DotNet-LICENSE.txt` and
  `licenses/DotNet-ThirdPartyNotices.txt`

All product names and trademarks are the property of their respective owners.

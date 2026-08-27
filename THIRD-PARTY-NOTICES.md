# Third-Party Notices

POE Alarm includes or depends on the following third-party components.
Complete license and third-party notice texts are stored in the repository's `licenses`
directory and are included beside the executable in the release archive.

## .NET Windows Desktop Runtime

- Project: .NET, Copyright Microsoft Corporation and contributors
- Source: https://github.com/dotnet/runtime
- Included copies: `licenses/DotNet-LICENSE.txt` and
  `licenses/DotNet-ThirdPartyNotices.txt`

All product names and trademarks are the property of their respective owners.

---

POE Alarm no longer bundles PaddleOCR's PP-OCRv5 recognition model or the ONNX Runtime.
Affixes are read from the item text the client itself writes to the clipboard, so there is
no recognition model to ship, and both components were removed along with the OCR pipeline.

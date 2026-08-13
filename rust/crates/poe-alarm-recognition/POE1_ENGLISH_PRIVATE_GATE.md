# POE1 English private tooltip gate
This is the release-gate contract for real Path of Exile 1 English item-tooltip screenshots. The
images remain private and outside Git. The checked-in JSON fixture demonstrates the schema only;
it is not recognition evidence.

## Minimum private corpus

- Two or more independent, native-resolution POE1 English screenshots with the item tooltip open.
- One fixed affix-area ROI per layout, or a per-screenshot ROI when the tooltip moved.
- At least four positive full-line modifier templates across the corpus.
- At least two `semantic-neighbor` negatives. Each names a visually/semantically close positive on
  the same screenshot through `evidenceTemplate`, for example attack speed versus cast speed or
  global critical chance versus spell critical chance.
- At least two `cross-screenshot` negatives. Each modifier is a declared positive on
  `positiveImage` but must be absent from the screenshot where the negative is declared.

Use `tests/fixtures/poe1_english_negative_contract.valid.json` as the manifest shape. Never copy the
private PNG files into this repository. Put private run output below an ignored `rust/target/...`
directory.

## Commands

Configure `POE_ALARM_ONNX_RUNTIME` to the packaged ONNX Runtime DLL and point `IMAGE_ROOT` at the
read-only private image directory. Run both modes; neither is a substitute for the other.

```powershell
cargo run --manifest-path rust/Cargo.toml --release --locked \
  -p poe-alarm-recognition --bin recognition-manifest-probe -- \
  --manifest C:/private/poe1-ocr-manifest.en.json \
  --image-root $IMAGE_ROOT --game poe1 --language en --mode quick \
  --require-negative-contract --onnx-runtime $env:POE_ALARM_ONNX_RUNTIME \
  --csv rust/target/recognition-evidence/poe1-english/quick.csv

cargo run --manifest-path rust/Cargo.toml --release --locked \
  -p poe-alarm-recognition --bin recognition-manifest-probe -- \
  --manifest C:/private/poe1-ocr-manifest.en.json \
  --image-root $IMAGE_ROOT --game poe1 --language en --mode structured \
  --require-negative-contract --onnx-runtime $env:POE_ALARM_ONNX_RUNTIME \
  --csv rust/target/recognition-evidence/poe1-english/structured.csv
```

A passing summary must show zero for `failed`, `negative_failed`,
`semantic_negative_failed`, `cross_negative_failed`, `assisted_sibling_collisions`,
`detailed_row_failed`, and `detailed_cross_negative_alerts`. Repeat each mode in five independent
processes before using it as formal replacement evidence; report accuracy and warm p50/p95 from the
combined CSVs.

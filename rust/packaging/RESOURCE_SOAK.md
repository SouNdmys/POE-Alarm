# Production runtime resource soak

This gate measures the Rust production composition without adding counters or
branches to the monitoring hot path.

`poe-alarm-runtime-soak live` uses the real runtime actor, native alert owner,
GDI desktop capture, WinRT OCR and packaged Paddle/ONNX fallback. Select a
desktop region that remains visually unchanged and use a deliberately impossible
target so the blocking alert cannot appear during the resource run.

`screenshot` replaces only the unavailable live game frame source with a fixed
real PNG/JPEG. It still uses production WIC decoding, runtime scheduling, WinRT
and Paddle. It is useful for accelerated lifecycle/workload screening, but it is
not a live game soak and never counts as the two-hour wall-clock field gate.
The runtime keeps one screenshot thread, WIC decoder and profile-keyed ONNX
recognizer warm, but clears request-scoped evidence and fingerprint caches before
every screenshot request. Always inspect `cycles` and `recognition_seconds` in
stdout: a high cycle count with near-zero recognition time is a cache-validation
failure and must not be accepted as a production OCR resource result.

Build and run the 60-second gate:

```powershell
$env:CARGO_TARGET_DIR = 'rust\target-soak'
cargo build --manifest-path rust\Cargo.toml -p poe-alarm-runtime `
  --bin poe-alarm-runtime-soak --release --locked

.\rust\packaging\measure-runtime-soak.ps1 `
  -Mode live `
  -DurationSeconds 60 `
  -Region '0,0,1433,1117'
```

The script samples process CPU, private bytes, working set, handles and threads,
then writes ignored CSV/log/JSON evidence under
`artifacts/rust-validation/resource-soak/`. The default growth gates match the
migration plan: at most 5 MiB private-byte growth and 2% handle growth. Thread
growth is limited to two after warm-up. CPU defaults to a conservative 25% of
one machine's total logical capacity; comparison against the .NET 1.0 process
should still be recorded before final replacement.

The current hard gate deliberately retains its original warm first/last sample
definition. Native OCR and allocator memory has a repeatable sawtooth over each
request, so review the CSV windows and robust trend alongside that binary gate;
do not relabel a failed JSON report as passed. A longer field run may add
windowed statistics, but it must preserve the original endpoint result for
comparison.

The 2026-08-13 fixed POE2 Traditional Chinese screenshot precheck produced the
following evidence after request-scoped cache reset was verified:

- 60 seconds: 630 cycles / 34.554 seconds of OCR, private bytes
  `70,451,200 -> 71,618,560` (+1,167,360), handles `218 -> 217`, threads
  `11 -> 13`, result **pass**. Evidence prefix:
  `artifacts/rust-validation/resource-soak/20260813-153727-screenshot-`.
- 300 seconds: 3,157 cycles / 172.785 seconds of OCR. The existing first/last
  rule measured `74,481,664 -> 83,013,632` (+8,531,968), so the JSON result is
  correctly **fail** even though handles fell `220 -> 217`, threads fell
  `11 -> 8`, and rolling windows approached a roughly 72-74 MiB plateau.
  Evidence prefix:
  `artifacts/rust-validation/resource-soak/20260813-153926-screenshot-`.

The 300-second run used `-SkipThresholds` only to preserve the complete report;
that switch does not turn a failed threshold into a pass. A prior 60-second
diagnostic (`20260813-153154`) is excluded because 1,711 cycles reported only
0.074 seconds of recognition, proving it reused request-scoped evidence instead
of exercising OCR on every request.

A follow-up 900-second run of the same executable completed 9,556 real OCR
cycles (460.178 seconds of reported recognition time) and shut down in 2.465
ms. The unchanged endpoint gate passed: private bytes
`74,067,968 -> 60,698,624`, handles `220 -> 216`, and threads `11 -> 8`.
After excluding the first 180 seconds, the first and last five-minute
private-byte medians were 70.195 and 70.191 MiB; the 180-900 second OLS slope
was +0.0402 MiB/min with R² 0.00073. This supports a bounded native
allocator/ONNX plateau, not completion of the required two-hour live-game
field gate. Evidence prefix:
`artifacts/rust-validation/resource-soak/20260814-000418-screenshot-`.

For the actual two-hour wall-clock gate, set `-DurationSeconds 7200` and do not
set `-MaximumCycles`. The JSON report sets `wallClockTwoHourGate=true` only in
that case. Any shorter or cycle-limited run is explicitly classified as a
precheck and must not be reported as a completed two-hour soak.

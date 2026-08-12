# Transient target-frame replay

This tool drives the real `AffixMonitor` against real screenshot ROIs while physical roll
attempts occur at 30/40/50/60/80/100 ms intervals.

It replaces the selected target row with a different real affix row for the absent state. Every
phase trial receives fresh absent/present fingerprints; by default each fingerprint then remains
byte-identical while that state is displayed, proving that progressive OCR continues across
successive captures without leaking completed caches between trials.

From the repository root:

```powershell
$env:DOTNET_CLI_HOME = "$PWD\.dotnet-home"
$env:NUGET_PACKAGES = "$PWD\.packages"
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.TransientReplay --configuration Release -- `
  --case weapon-attack-speed `
  --durations 30,40,50,60,80,100 `
  --trials 20 `
  --mode sliced `
  --roll-model guarded `
  --fingerprints stable-state `
  --seed 20260811 `
  --csv artifacts\benchmarks\transient-replay.csv
```

## Roll models

- `guarded` (default) models production. Physical clicks occur every dwell interval. An
  `InputGuardRequested` event holds the current tooltip; clicks during the hold are counted as
  `swallowed` and never replayed. `InputGuardReleased` permits the next physical click. The first
  accepted click changes absent to target; a second accepted click before detection is `overroll`.
- `fixed-dwell` is the deliberately unguarded diagnostic model. It changes to target at the
  scheduled onset and back to absent exactly one dwell later, even while production says the
  input guard is held. Use it to measure raw OCR exposure, not final guarded behaviour.

`baseline` exhausts every deferred row batch before allowing another capture. It is a
conservative **upper-bound approximation** of the 0.4.1 capture gap, not an exact historical
binary replay: it repeats preprocessing and async-call overhead on the current recognizer.
Use `--mode sliced` for current production acceptance.

Fingerprint modes:

- `stable-state` (default): new fingerprints per trial, stable across all captures within each
  state. This is the continuation test.
- `cold-each-capture`: moves two inconsequential real blue edge pixels per row on every capture.
  This explicit stress mode disables continuation and every exact band-cache shortcut.

`--wrap-case id` creates a real-pixel-derived wrapped target. It splits the original manifest
glyphs at an inter-character quiet column, moves the remaining pixels to an adjacent second row,
and shifts lower screenshot rows intact. It never renders or synthesizes text.

`--trace dwell:trial` prints a chronological capture, guard, physical-click, OCR-decision and
match timeline. Each decision includes source state and OCR slice duration.

## Metrics and exit status

- `hit`: detection came from a capture whose source state contained the target.
- `timely`: the target was detected before any accepted click rolled past it. In guarded mode
  this may legitimately take longer than one physical dwell because intervening clicks are
  swallowed.
- `overroll`: an accepted click changed target back to absent before detection.
- `accepted` / `swallowed`: physical click disposition.
- `guard-held`: total guard duration in the trial.
- `captured-miss`: a target frame was captured but never detected.
- `early-false`: an absent-source alert completed before target onset; it never counts as timely.
- `false-alert`: any alert whose source capture was absent, regardless of completion time.
- `onset->decision`: actual accepted target click to match.
- `capture->decision`: matching scan's target capture to match.

Preflight recognizes the exact same frame repeatedly until a match or `RequiresRescan=false`.
Timed trials use a separate recognizer so preflight caches cannot contaminate results.

Guarded acceptance requires final hit on every trial, `overroll=0`, `false-alert=0`, and
`captured-miss=0`. The dwell thresholds are: 40 ms hit 100% / timely at least 95%; 50 ms hit
100% / timely at least 95%; and 60/80/100 ms hit and timely 100%. An exercised failure returns
exit code `3`; a completed failing measurement is intentionally non-zero.

## Required path coverage

- Ordinary top-1: `weapon-attack-speed`.
- Multi-line: `weapon-shrine-on-kill --wrap-case weapon-shrine-on-kill`; preflight reports
  `physical-lines=2`.
- CTC-assisted: `rare-belt-stun-block-recovery` from `traditional-ocr-new-a.json`; preflight
  reports `route=ctc-assisted`.
- Outside top two: `cluster-sadist`; estimated scheduling rank is 3.

## 2026-08-11 guarded 40 ms smoke

All guarded runs had 100% hit/timely, zero overroll, zero captured misses, and zero false alerts.

| Case | Trials | Physical | Accepted | Swallowed | Onset-to-decision mean | Result |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| `weapon-attack-speed` | 20 | 41 | 20 | 21 | 25.7 ms | PASS |
| wrapped `weapon-shrine-on-kill` | 5 | 19 | 5 | 14 | 98.5 ms | PASS |
| `rare-belt-stun-block-recovery` | 5 | 31 | 5 | 26 | 132.1 ms | PASS |
| rank-3 `cluster-sadist` | 5 | 19 | 5 | 14 | 62.5 ms | PASS |

CSV files:

- `artifacts/benchmarks/guarded-top1-40ms-final.csv`
- `artifacts/benchmarks/guarded-multiline-40ms-final.csv`
- `artifacts/benchmarks/guarded-ctc-40ms-final.csv`
- `artifacts/benchmarks/guarded-rank3-40ms-final.csv`

## Fixed-dwell diagnosis

A traced 100 ms wrapped multi-line trial started normally: the first capture occurred at
1.53 ms. The target was captured from 142.90 through 220.75 ms and received four OCR decisions
(25.70, 15.66, 16.90, and 14.84 ms). Preflight requires five progressive slices. At 235.68 ms
the fixed model had already changed back to absent, so progress reset and the trial missed. This
proves the earlier 120/120 captured misses were caused by deliberately bypassing the production
guard, not by a replay clock starting after the target window.

The same wrapped target under guarded 40 ms replay held the target for all five slices. Two
physical clicks were swallowed during target recognition, and the match completed at 316.00 ms
without overroll.

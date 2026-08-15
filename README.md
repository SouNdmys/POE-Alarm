# POE Alarm

A local OCR crafting alarm for Path of Exile 1 & 2. It watches a screen region you select, reads the blue affix lines inside the item tooltip, and the moment your target affix combination appears it stops scanning, loops an alert sound, and throws up a red lock screen that blocks further mouse clicks — so a fast crafting hand cannot click away the roll you just hit.

Current release: **1.0.0**, a fully native Rust build (no .NET, no Tauri, no WebView). Windows 10/11 x64. Supports the English and Traditional Chinese clients of both POE 1 and POE 2. No network access, no accounts, no telemetry: the app only reads screen pixels and never synthesizes or replays any input into the game.

The UI ships in English and 简体中文 — switch instantly in Settings; the UI language is independent from the affix OCR language.

> This repository previously hosted a .NET (WPF) implementation of 1.0. It has been retired and fully replaced by this Rust build; the history remains in git.

## Download & run

Grab the ZIP from [Releases](https://github.com/SouNdmys/POE-Alarm/releases), extract, and run `poe-alarm-app.exe`. No runtime installation required. Windows may show an unsigned-binary warning on first run.

Build from source:

```powershell
cargo build --manifest-path rust\Cargo.toml --release -p poe-alarm-app
```

The binary lands in `rust\target\release\poe-alarm-app.exe`.

## How to use

1. In the title bar, pick the game (POE 1 / POE 2) and the affix language matching your client (Traditional Chinese / English). Rules, capture regions, and languages are stored per game.
2. Copy a complete affix from the game or PoEDB / PoE2DB and paste it into **Complete affix template**. Numbers become value slots automatically; numeric rules default to **unlimited** — switch a row to Range / ≥ / ≤ / = only when the value matters.
3. Need multiple acceptable outcomes? Use **+Option** (options are alternatives — any one of them triggers the alert), add affixes within an option with **+Affix**, and choose **Alert when**: any / all / a chosen count. Every edit saves automatically.
4. Back in game, press `Ctrl+Shift+F11` and select only the affix block of the item tooltip — the smaller the region, the faster the recognition.
5. Press `Ctrl+Shift+F10` (configurable in Settings) and craft normally. While recognition is undecided your mouse stays fully passed-through — no clicks are delayed or eaten.
6. On a match, the red lock screen takes over the mouse: everything outside the center card is transparent but still intercepts clicks. Check the item, then click **Confirm** (or press `Ctrl+Shift+F12` while the card is up). Clicks within ~300 ms after confirming are absorbed too. Press F10 again for the next round.

The in-app **User guide** tab carries the same walkthrough, the Traditional Chinese OCR setup, and contact info. **Analyze screenshot** replays the whole pipeline on a saved screenshot — useful for validating templates before you play.

### Global hotkeys

| Hotkey | Action |
| --- | --- |
| `Ctrl+Shift+F10` | Start monitoring (three combinations selectable in Settings) |
| `Ctrl+Shift+F11` | Select the capture region (Esc cancels) |

Stopping is deliberate: use the **Stop monitoring** button in the UI, and release a match via the red card. There is no global stop hotkey to fat-finger.

### Status overlay

A small always-on-top card shows the monitoring state and elapsed time. Drag it anywhere while idle (the position is remembered); while monitoring it becomes click-through and never steals focus. Overlay visibility, screen-capture visibility (OBS etc.), the alert sound (local WAV), and the UI language live in **Settings**.

## Traditional Chinese OCR (recommended)

For Traditional Chinese clients, install the Windows `zh-TW` OCR capability so recognition takes the fastest, most accurate system path. In an elevated PowerShell:

```powershell
Add-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0"
```

Verify — `State : Installed` means success, then restart POE Alarm:

```powershell
Get-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0"
```

If installation fails, check that Windows Update has not been disabled. Alternatively: Settings → Time & Language → Language & Region → add "中文(台灣)" with its language features. Without the capability the app automatically falls back to the bundled offline PP-OCRv5 engine — fully functional, somewhat slower. Neither path needs Python, a Paddle installation, or the internet. Production details and measurements: [Traditional Chinese OCR notes](docs/traditional-chinese-ocr.md).

## Matching rules

The app does not guess keywords and needs no built-in affix database. The complete affix you paste *is* the rule. For example:

```text
(6—8)% increased Attack Speed if you've dealt a Critical Strike Recently
```

normalizes to:

```text
<PCT> increased attack speed if you've dealt a critical strike recently
```

`#`, actual rolls, fixed numbers, ranges, and advanced-description forms like `8(6-8)%` all map to typed value slots; percent-vs-plain and sign are kept as structure. Everything else must match the whole line exactly — semantic neighbours like Attack/Cast, Cold/Fire, dealt/killed never cross-match, and a line with an OCR dropout never false-alarms (the next scan recovers it). Numeric rules compare the value **shown on screen** — catalysts, quality, and special effects change shown values, so configure what you see. A logical affix may span 1–4 physical lines in POE 1 and up to 8 in POE 2 (long tablet mods). One physical affix line counts at most once per match option.

## Safety boundaries

- Screen pixels in, nothing out: no input synthesis, replay, or queuing — ever.
- No low-level mouse guard is armed before a confirmed match; every click passes straight to the game while recognition runs.
- After a strict match the app first presents and *verifies* that the red lock layer is visible, clickable, and covers the whole virtual desktop before it intercepts input. If it cannot present reliably, it reports the error and stops instead of pretending a hidden window protects you.
- Settings live at `%LOCALAPPDATA%/PoeAlarm/settings.json`. Upgrading from the Rust preview migrates its settings automatically on first launch; a .NET-era file is kept as `settings.json.dotnet-1.0.bak`.

## Project layout

The `rust/` workspace is layered:

- `poe-alarm-core` — affix normalization, whole-line matching, the structured rule engine, and numeric constraints.
- `poe-alarm-vision` / `poe-alarm-ocr-win` / `poe-alarm-ocr-paddle` — screenshot decoding, blue-mask banding, Windows OCR, and the bundled PP-OCRv5 fallback.
- `poe-alarm-recognition` / `poe-alarm-monitoring` / `poe-alarm-runtime` — recognition orchestration, the monitor loop, and the production runtime.
- `poe-alarm-platform-win` / `poe-alarm-alert-win` — hotkeys, the status overlay, region selection, WAV playback, the red lock layer, and the mouse guard.
- `poe-alarm-app` — the GPUI front end (Ledger design), a single workbench window.
- `poe-alarm-settings` — the settings model, schema compatibility, and migration.

Verification entry points:

```powershell
cargo fmt --manifest-path rust\Cargo.toml --all -- --check
cargo clippy --manifest-path rust\Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo test --manifest-path rust\Cargo.toml --workspace --all-targets --locked --release --no-fail-fast
```

Screenshot regression tools (`recognition-manifest-probe`, `recognition-screenshot-probe`) batch-verify positives and semantic-neighbour negatives against real game screenshots; the repository only carries the JSON manifests — raw screenshots stay out of git for size and privacy.

Release ZIPs include `THIRD-PARTY-NOTICES.md` and `licenses/` covering the bundled offline OCR runtime and model; do not strip them from redistributed packages.

## License

**PolyForm Noncommercial 1.0.0** — see [LICENSE.md](LICENSE.md). You may use, modify, and share this software for any **noncommercial** purpose; **commercial use is not permitted**. Third-party components (ONNX Runtime, the PP-OCRv5 model, and vendored Rust crates) remain under their own licenses in `THIRD-PARTY-NOTICES.md` and `licenses/`.

## Author & support

- Author: **SouNd**
- Contact: [soundmys1994@gmail.com](mailto:soundmys1994@gmail.com)
- Project home: [SouNdmys/POE-Alarm](https://github.com/SouNdmys/POE-Alarm)

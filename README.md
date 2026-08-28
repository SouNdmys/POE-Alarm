# POE Alarm

A local crafting alarm for Path of Exile 1 & 2. It reads the item under your cursor by asking the game client for it — the same text you get with Ctrl+C — and the moment your target affix combination appears it loops an alert sound and throws up a red lock screen that blocks further mouse clicks, so a fast crafting hand cannot click away the roll you just hit.

Current release: **1.0.2**, a fully native Rust build (no .NET, no Tauri, no WebView). Windows 10/11 x64. Supports the English and Traditional Chinese clients of both POE 1 and POE 2. No network access, no accounts, no telemetry.

It does send one thing into the game: a `Ctrl+C` keystroke, to make the client write the hovered item to the clipboard. That is the whole of it — see [Safety boundaries](#safety-boundaries).

Between a click and the new affixes appearing sits a server round trip, and nothing here can outrun it. Measured against a live client, a click every 50ms is still caught reliably — roughly 1500 crafts at that cadence produced one that got away. Clicking faster than that risks the next click landing before the lock screen does.

The UI ships in English and 简体中文 — switch instantly in Settings; the UI language is independent from the affix language of your game client.

> This repository previously hosted a .NET (WPF) implementation of 1.0. It has been retired and fully replaced by this Rust build; the history remains in git.

## Download & run

Grab the ZIP from [Releases](https://github.com/SouNdmys/POE-Alarm/releases), extract, and run `PoeAlarm.exe`. No runtime installation required. Windows may show an unsigned-binary warning on first run.

Build from source:

```powershell
cargo build --manifest-path rust\Cargo.toml --release -p poe-alarm-app
```

The binary lands in `rust\target\release\poe-alarm-app.exe`.

## How to use

1. In the title bar, pick the game (POE 1 / POE 2) and the affix language matching your client (Traditional Chinese / English). Rules are stored per game *and* per language, so switching never loses the other set.
2. Copy a complete affix from the game or PoEDB / PoE2DB and paste it into **Complete affix template**. Numbers become value slots automatically; numeric rules default to **unlimited** — switch a row to Range / ≥ / ≤ / = only when the value matters.
3. Need multiple acceptable outcomes? Use **+Option** (options are alternatives — any one of them triggers the alert), add affixes within an option with **+Affix**, and choose **Alert when**: any / all / a chosen count. Every edit saves automatically.
4. Check the rule before you trust it: in game, rest the cursor on an item you already own and press `Ctrl+Shift+F11`. The verdict comes straight back, along with every modifier the rules were shown. (You can also press `Ctrl+C` yourself and paste into the box at the bottom right.)
5. Press `Ctrl+Shift+F10` (configurable in Settings) and craft normally. Until a match is confirmed your mouse stays fully passed-through — no clicks are delayed or eaten.
6. On a match, the red lock screen takes over the mouse: everything outside the center card is transparent but still intercepts clicks. Check the item, then click **Confirm** (or press `Ctrl+Shift+F12` while the card is up). Clicks within ~300 ms after confirming are absorbed too. Press F10 again for the next round.

The in-app **User guide** tab carries the same walkthrough and contact info.

### Global hotkeys

| Hotkey | Action |
| --- | --- |
| `Ctrl+Shift+F10` | Start monitoring (three combinations selectable in Settings) |
| `Ctrl+Shift+F11` | Check the item under the cursor right now |
| `Ctrl+Shift+F12` | Release the red lock screen |

Stopping is deliberate: use the **Stop monitoring** button in the UI, and release a match via the red card. There is no global stop hotkey to fat-finger.

### Status overlay

A small always-on-top card shows the monitoring state and elapsed time. Drag it anywhere while idle (the position is remembered); while monitoring it becomes click-through and never steals focus. Overlay visibility, screen-capture visibility (OBS etc.), the alert sound (local WAV), and the UI language live in **Settings**.

## Matching rules

The app does not guess keywords and needs no built-in affix database. The complete affix you paste *is* the rule. For example:

```text
(6—8)% increased Attack Speed if you've dealt a Critical Strike Recently
```

normalizes to:

```text
<PCT> increased attack speed if you've dealt a critical strike recently
```

`#`, actual rolls, fixed numbers, ranges, and advanced-description forms like `8(6-8)%` all map to typed value slots; percent-vs-plain and sign are kept as structure. Everything else must match the whole line exactly — semantic neighbours like Attack/Cast, Cold/Fire, dealt/killed never cross-match. Numeric rules compare the value the client writes, which is the rolled value, catalysts and quality included. A logical affix may span 1–4 physical lines in POE 1 and up to 8 in POE 2 (long tablet mods). One physical affix line counts at most once per match option.

No modifier is excluded for what produced it. A prefix, an implicit, an enchantment and a bench craft are all matched the same way, because deciding otherwise would mean recognising the word the client uses for each source — and a source this app has not seen would silently stop an alarm from firing. The consequence worth knowing: if a rule happens to match an implicit, it will fire on every roll, since implicits never change. Edit the rule.

The app keeps no affix database and needs no updates when GGG adds modifiers. The template you paste is the rule.

## Safety boundaries

- **One keystroke goes in, and only that one.** To read an item the app sends `Ctrl+C` to the focused game window, which makes the client copy the hovered item to the clipboard. It is sent only while a POE window is focused, and it is the only input this app ever synthesizes. It does not click, move the mouse, press anything else, record macros, or replay what you did.
- **Nothing else touches the game.** No memory reads or writes, no DLL injection, no packet inspection, no overlay hooked into the renderer, no network traffic of any kind. The app talks to Windows and to the clipboard.
- The one call site is `send_ctrl_c` in `rust/crates/poe-alarm-platform-win/src/win32/clipboard.rs`. `grep -rn SendInput rust/crates` returns five lines: that call, its `use` import, and three mentions in the doc comment and error text describing it.
- Reading the clipboard means overwriting whatever you had on it. That is a real cost of this design and there is no way around it while the client only offers text this way.
- No low-level mouse guard is armed before a confirmed match; every click passes straight to the game while the app is reading.
- At the instant of a confirmed match a hook-level click block arms, so the very next click cannot take the roll away while the red layer is still appearing. The layer is then presented and *verified* visible, clickable, and covering the whole virtual desktop; the block hands over to the verified layer, or fails open on a bounded timeout and reports the error instead of pretending a hidden window protects you. No block of any kind exists before a confirmed match.
- **Administrator rights are not requested at launch**, and the app runs unelevated for almost everyone. Windows refuses to deliver synthesized input from a lower-integrity process to a higher-integrity window, so if a launcher or accelerator started the game *as administrator*, the `Ctrl+C` is silently discarded and monitoring would run forever without ever alarming. The app detects that case and says so, and **Settings → Privileges → Restart as administrator** relaunches it. Elevation is used for nothing else: it does not unlock any extra capability, it only lets that one keystroke reach an elevated window.
- Settings live at `%LOCALAPPDATA%/PoeAlarm/settings.json`. Upgrading from the Rust preview migrates its settings automatically on first launch; a .NET-era file is kept as `settings.json.dotnet-1.0.bak`.

## Project layout

The `rust/` workspace is layered:

- `poe-alarm-core` — affix normalization, whole-line matching, the structured rule engine, and numeric constraints.
- `poe-alarm-clipboard` — turns the client's item text into modifiers, keeping each one whole so a hybrid affix cannot satisfy two conditions at once.
- `poe-alarm-monitoring` / `poe-alarm-runtime` — the monitor loop and the production runtime.
- `poe-alarm-platform-win` / `poe-alarm-alert-win` — hotkeys, the clipboard capture, the status overlay, WAV playback, the red lock layer, and the mouse guard.
- `poe-alarm-app` — the GPUI front end (Ledger design), a single workbench window.
- `poe-alarm-settings` — the settings model, schema compatibility, and migration.

Verification entry points:

```powershell
cargo fmt --manifest-path rust\Cargo.toml --all -- --check
cargo clippy --manifest-path rust\Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo test --manifest-path rust\Cargo.toml --workspace --all-targets --locked --release --no-fail-fast
```

The parser is held to real client output: `tests/corpus/` carries 50 items copied byte-for-byte out of both games in both languages, and the corpus tests assert that every modifier the client annotated reaches the rules. `rust/crates/poe-alarm-clip-only` is a console harness that drives the shipped code against a running client, which is the one thing a unit test cannot do.

Release ZIPs include `THIRD-PARTY-NOTICES.md` and `licenses/`; do not strip them from redistributed packages.

## License

**PolyForm Noncommercial 1.0.0** — see [LICENSE.md](LICENSE.md). You may use, modify, and share this software for any **noncommercial** purpose; **commercial use is not permitted**. Third-party components remain under their own licenses in `THIRD-PARTY-NOTICES.md` and `licenses/`.

## Author & support

- Author: **SouNd**
- Contact: [soundmys1994@gmail.com](mailto:soundmys1994@gmail.com)
- Project home: [SouNdmys/POE-Alarm](https://github.com/SouNdmys/POE-Alarm)

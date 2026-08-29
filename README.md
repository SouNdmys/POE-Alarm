# POE Alarm

*This product isn’t affiliated with or endorsed by Grinding Gear Games in any way.*

> ## This branch: click-invoked copies, no timer
>
> GGG's [developer documentation](https://www.pathofexile.com/developer/docs/index) requires
> any synthesized input that affects the game to be invoked manually, naming timers among the
> disallowed triggers — which is what the timed monitoring in releases up to 1.0.6 was. On
> this branch **every copy is invoked by a press of yours**: a pass-through hook counts your
> clicks (it suppresses and synthesizes nothing), and each click is followed by one `Ctrl+C`
> after a delay for the server round trip — at most three chords per click when the client
> answers late, never on a timer, never while you are idle. A manual `Ctrl+C` works too.
>
> Stated rather than argued: the invoking press is the crafting click itself, doing double
> duty. Whether that satisfies "invoked manually" is GGG's call — but a timer it is not,
> and zero input of any kind is synthesized unless you act.

A local crafting alarm for Path of Exile 1 & 2. It reads the item under your cursor by asking the game client for it — the same text you get with Ctrl+C — and the moment your target affix combination appears it loops an alert sound and throws up a red lock screen that blocks further mouse clicks, so a fast crafting hand cannot click away the roll you just hit.

Current build on this branch: **1.1.0** (passive monitoring, unreleased), a fully native Rust build (no .NET, no Tauri, no WebView). Windows 10/11 x64. Supports the English and Traditional Chinese clients of both POE 1 and POE 2. No network access, no accounts, no telemetry.

Monitoring synthesizes input only in answer to your own presses: one `Ctrl+C` follows each click you make (with a bounded retry when the client answers late), and a manual `Ctrl+C` is honored as well. Nothing is sent on a timer and nothing is sent while you are idle — see [Safety boundaries](#safety-boundaries).

**It does make crafting faster, and that is the point.** Without it, every roll costs you a look at the tooltip and a decision about what you are seeing. With it you can click straight through a stack of currency without reading anything, and be interrupted only when the combination you asked for actually appears. Not having to read is the whole gain, and in practice it is a large one.

**The timing that decides a catch is now yours.** The app's own side is fast — once your copy lands on the clipboard, noticing, judging and arming the click block take on the order of twenty milliseconds. What decides whether a winning roll survives is when you copy: the new affixes only exist after a server round trip, so a `Ctrl+C` pressed too soon after the click captures the old item, and that roll is only judged at your next copy. Leave room between the click and the copy, and room after the copy before the next click.

**Suggested use:** start monitoring, copy the item once to set the baseline, then roll in a steady rhythm of click, copy, pause. If rolls start getting past the alarm, widen the gaps — that is the signal to slow down, not a reason to trust it further. When an alert fails to block the next click for a reason other than timing, the app says so explicitly in its log, so a silent failure and bad timing do not look alike.

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

- **Every chord answers a press of yours.** A pass-through `WH_MOUSE_LL` hook counts your left clicks — it suppresses nothing and synthesizes nothing, and every event passes straight through. Each click is followed by one `Ctrl+C` after ~80ms (`POE_ALARM_COPY_DELAY_MS` adjusts it); if the client answers with stale text or not at all, at most two spaced retries follow, then the click is given up. Hard ceiling: three chords per click, enforced by a pinned constant. No clicks, no chords — idle monitoring sends nothing, ever.
- **The one synthesized input in the whole app is the manual check hotkey.** `Ctrl+Shift+F11` sends a single `Ctrl+C` chord per press — one `SendInput` call carrying four key events (three when you already hold Ctrl, because releasing a key you are physically pressing would corrupt Windows' key state). One manual press, one fixed function, one action. The GUI framework this links (GPUI) carries its own Alt-key `SendInput` in a window-activation path this app never calls, so the precise claim is that `Ctrl+C` is the only input **this project's own code** synthesizes.
- **The clipboard is read only while the game is in the foreground.** That gate is a privacy boundary: tab out, and the app stops reading clipboard content entirely — a copy made in another program is never read, and cannot be misread as a roll when you tab back in. Clicks made outside the game are written off the same way.
- **Nothing else touches the game.** No memory reads or writes, no DLL injection, no packet inspection, no overlay hooked into the renderer, no network traffic of any kind. The app talks to Windows and to the clipboard.
- The one call site is `send_ctrl_c` in `rust/crates/poe-alarm-platform-win/src/win32/clipboard.rs:126`. Check it yourself: `grep -rn SendInput rust/crates` returns eight lines — that call, its `use` import, four mentions in doc comments and error text, and two in the `poe-alarm-clip-only` README. Exactly one of the eight executes anything.
- Reading the clipboard means overwriting whatever you had on it. That is a real cost of this design and there is no way around it while the client only offers text this way.
- No low-level mouse guard is armed before a confirmed match; every click passes straight to the game while the app is reading.
- At the instant of a confirmed match a hook-level click block arms, so the very next click cannot take the roll away while the red layer is still appearing. The layer is then presented and *verified* visible, clickable, and covering the whole virtual desktop; the block hands over to the verified layer, or fails open on a bounded timeout and reports the error instead of pretending a hidden window protects you. No block of any kind exists before a confirmed match.
- **Administrator rights are not requested at launch.** If a launcher started the game *as administrator*, Windows both hides your clicks from the app's unelevated hook and discards its copies — so monitoring detects that case at start and says so immediately, and **Settings → Privileges → Restart as administrator** relaunches it. Elevation is used for nothing else.
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

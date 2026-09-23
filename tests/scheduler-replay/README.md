# Offline production-source replay

Requires Git history containing commit `1e161bf0c27cc19182f816ce0dd99f46bb3bc946` (1.1.4). The build script reads that source and the current source, substituting only platform functions and the clock. It never reads the system clipboard, sends input, or registers a hook. The parser and matcher are the actual workspace crates.

```powershell
$env:SCHEDULER_EXTENDED = '1'
cargo run --manifest-path tests/scheduler-replay/Cargo.toml --release --locked
Remove-Item Env:SCHEDULER_EXTENDED
```

Extended mode runs 252 combinations of click interval, text availability delay, clipboard response time, observation phase and periodic jitter. Without the environment variable it runs 72 combinations and three focused overlap/baseline cases. Both sides retain 1.1.4's 10ms poll pacing. Each sequence contains 30 clicks and drains for 1000ms after the last click.

`observed` counts unique rolls that reach evaluation; `before next click` counts results received strictly before the next click. The final roll has no next-click deadline. Reported latency is click-to-evidence, excluding native input blocking. A game ping is not equivalent to the model's text delay.

See [the release validation report](../../docs/validation-1.2.0.md) for assumptions, rejected experiments and limitations. This model is a regression probe, not a claim of game accuracy or guaranteed interception.

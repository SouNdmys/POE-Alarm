# Windows OCR probe

Small command-line harness for measuring `Windows.Media.Ocr` against POE screenshots.

```powershell
dotnet run --project tools/PoeAlarm.OcrProbe -- "C:\path\screenshot.png" --roi 50,440,870,50 --scale 1 --repeat 5
```

Options:

- `--roi x,y,width,height` crops in source-image pixels.
- `--scale n` applies decoder scaling before OCR.
- `--pixel-mode gray|bgra` chooses the OCR bitmap format.
- `--language en-US` chooses an installed Windows OCR language.
- `--repeat n` reuses the prepared bitmap so recognition latency can be measured without file/decode time.

With no image argument, the probe uses the original screenshot supplied for this project if the temporary file still exists.

## Baseline from the supplied screenshot

Machine-specific measurements on the `1023 x 860` screenshot:

| Crop | Processing | Result | Warm OCR latency |
| --- | --- | --- | --- |
| Full image | Gray8, 1x | Target line exact; 23 lines total | 47 ms |
| Modifier area `56,260,856,380` | Gray8, 1x | Target line exact; 12 lines total | 29 ms |
| Target row `50,440,870,50` | Gray8, 1x | Exact in 5/5 runs | 3.1 ms |
| Target row `50,440,870,50` | BGRA8, 1x | Exact in 5/5 runs | 3.4 ms |
| Target row `50,440,870,50` | Gray8, 2x | Exact in 5/5 runs | 6.1 ms |
| Target row `50,440,870,50` | Gray8, 0.75x | No line detected | under 1 ms |

The exact recognized target row was:

```text
8(6-8)% INCREASED ATTACK SPEED IF YOU'VE DEALT A CRITICAL STRIKE RECENTLY
```

The 1x result is both faster and at least as accurate as the upscaled variants for this screenshot. Downscaling below the native text size crosses the Windows OCR detector's practical threshold.

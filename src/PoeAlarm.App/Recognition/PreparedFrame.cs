namespace PoeAlarm.App.Recognition;

public sealed record PreparedFrame(
    int Width,
    int Height,
    int Stride,
    byte[] BgraPixels,
    int ByteLength,
    ulong ContentFingerprint,
    int SourceTop = 0,
    int SourceBottom = 0,
    bool IsFallback = false);

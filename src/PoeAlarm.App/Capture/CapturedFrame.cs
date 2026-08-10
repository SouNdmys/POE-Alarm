namespace PoeAlarm.App.Capture;

public sealed record CapturedFrame(
    int Width,
    int Height,
    int Stride,
    byte[] BgraPixels,
    DateTimeOffset CapturedAt);

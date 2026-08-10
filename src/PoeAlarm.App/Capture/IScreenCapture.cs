namespace PoeAlarm.App.Capture;

public interface IScreenCapture : IDisposable
{
    CapturedFrame Capture(ScreenRegion region);
}

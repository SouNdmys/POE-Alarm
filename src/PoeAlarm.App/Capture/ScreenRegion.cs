namespace PoeAlarm.App.Capture;

public readonly record struct ScreenRegion(int X, int Y, int Width, int Height)
{
    public bool IsValid => Width > 0 && Height > 0;

    public override string ToString() => $"X={X}, Y={Y}, {Width} × {Height}";
}

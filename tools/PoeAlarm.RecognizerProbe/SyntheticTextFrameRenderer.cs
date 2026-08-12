using System.Runtime.ExceptionServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using PoeAlarm.App.Capture;

namespace PoeAlarm.RecognizerProbe;

internal static class SyntheticTextFrameRenderer
{
    public static CapturedFrame Render(string text, string fontFamily, double fontSize)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(text);
        ArgumentException.ThrowIfNullOrWhiteSpace(fontFamily);
        if (!double.IsFinite(fontSize) || fontSize is < 10 or > 96)
        {
            throw new ArgumentOutOfRangeException(nameof(fontSize));
        }

        CapturedFrame? result = null;
        ExceptionDispatchInfo? failure = null;
        var thread = new Thread(() =>
        {
            try
            {
                result = RenderOnStaThread(text, fontFamily, fontSize);
            }
            catch (Exception exception)
            {
                failure = ExceptionDispatchInfo.Capture(exception);
            }
        });
        thread.SetApartmentState(ApartmentState.STA);
        thread.Start();
        thread.Join();
        failure?.Throw();
        return result ?? throw new InvalidOperationException("Synthetic text rendering produced no frame.");
    }

    private static CapturedFrame RenderOnStaThread(string text, string fontFamily, double fontSize)
    {
        var textBlock = new TextBlock
        {
            Text = text,
            FontFamily = new FontFamily(fontFamily),
            FontSize = fontSize,
            FontWeight = FontWeights.Normal,
            Foreground = new SolidColorBrush(Color.FromRgb(135, 135, 255)),
            Background = Brushes.Black,
            Padding = new Thickness(18, 14, 18, 14),
            TextWrapping = TextWrapping.NoWrap,
            UseLayoutRounding = true,
        };
        TextOptions.SetTextFormattingMode(textBlock, TextFormattingMode.Display);
        TextOptions.SetTextRenderingMode(textBlock, TextRenderingMode.Grayscale);

        textBlock.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
        var width = Math.Max(1, (int)Math.Ceiling(textBlock.DesiredSize.Width));
        var height = Math.Max(1, (int)Math.Ceiling(textBlock.DesiredSize.Height));
        textBlock.Arrange(new Rect(0, 0, width, height));
        textBlock.UpdateLayout();

        var bitmap = new RenderTargetBitmap(width, height, 96, 96, PixelFormats.Pbgra32);
        bitmap.Render(textBlock);
        var stride = checked(width * 4);
        var pixels = new byte[checked(stride * height)];
        bitmap.CopyPixels(pixels, stride, 0);
        return new CapturedFrame(width, height, stride, pixels, DateTimeOffset.UtcNow);
    }
}

using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Selection;

public partial class RegionSelectionWindow : Window
{
    private Point? _dragOrigin;
    private Point _screenOrigin;

    public RegionSelectionWindow()
    {
        InitializeComponent();

        Left = SystemParameters.VirtualScreenLeft;
        Top = SystemParameters.VirtualScreenTop;
        Width = SystemParameters.VirtualScreenWidth;
        Height = SystemParameters.VirtualScreenHeight;
    }

    public ScreenRegion? SelectedRegion { get; private set; }

    private void OnMouseLeftButtonDown(object sender, MouseButtonEventArgs e)
    {
        _dragOrigin = e.GetPosition(SelectionCanvas);
        _screenOrigin = PointToScreen(_dragOrigin.Value);
        SelectionRectangle.Visibility = Visibility.Visible;
        SelectionCanvas.CaptureMouse();
        UpdateRectangle(_dragOrigin.Value);
    }

    private void OnMouseMove(object sender, MouseEventArgs e)
    {
        if (_dragOrigin is null || e.LeftButton != MouseButtonState.Pressed)
        {
            return;
        }

        UpdateRectangle(e.GetPosition(SelectionCanvas));
    }

    private void OnMouseLeftButtonUp(object sender, MouseButtonEventArgs e)
    {
        if (_dragOrigin is null)
        {
            return;
        }

        var end = e.GetPosition(SelectionCanvas);
        var screenEnd = PointToScreen(end);
        SelectionCanvas.ReleaseMouseCapture();

        var x = (int)Math.Round(Math.Min(_screenOrigin.X, screenEnd.X));
        var y = (int)Math.Round(Math.Min(_screenOrigin.Y, screenEnd.Y));
        var width = (int)Math.Round(Math.Abs(screenEnd.X - _screenOrigin.X));
        var height = (int)Math.Round(Math.Abs(screenEnd.Y - _screenOrigin.Y));

        _dragOrigin = null;

        if (width < 24 || height < 24)
        {
            SelectionRectangle.Visibility = Visibility.Collapsed;
            return;
        }

        SelectedRegion = new ScreenRegion(x, y, width, height);
        DialogResult = true;
    }

    private void OnKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Escape)
        {
            DialogResult = false;
        }
    }

    private void UpdateRectangle(Point current)
    {
        if (_dragOrigin is not { } origin)
        {
            return;
        }

        var x = Math.Min(origin.X, current.X);
        var y = Math.Min(origin.Y, current.Y);
        var width = Math.Abs(current.X - origin.X);
        var height = Math.Abs(current.Y - origin.Y);

        Canvas.SetLeft(SelectionRectangle, x);
        Canvas.SetTop(SelectionRectangle, y);
        SelectionRectangle.Width = width;
        SelectionRectangle.Height = height;
    }
}

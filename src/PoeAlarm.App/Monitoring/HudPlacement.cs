using System.Text.Json.Serialization;

namespace PoeAlarm.App.Monitoring;

/// <summary>
/// Persisted placement for the monitoring HUD. Relative coordinates are measured inside the
/// selected monitor's work area after subtracting the HUD size: 0 is the left/top edge and 1 is
/// the right/bottom edge. This survives DPI changes and most monitor-layout rearrangements better
/// than storing WPF device-independent coordinates or virtual-desktop pixels.
/// </summary>
public sealed record HudPlacement
{
    /// <summary>Uses the HUD's automatic capture-region-aware corner placement.</summary>
    public static HudPlacement Default { get; } = new();

    /// <summary>
    /// Optional Win32 display device name (for example <c>\\.\DISPLAY1</c>). When that display is
    /// no longer connected, callers should use the capture-region monitor or nearest monitor.
    /// </summary>
    public string? MonitorDeviceName { get; init; }

    /// <summary>Horizontal position in the usable work area, in the inclusive range 0–1.</summary>
    public double? RelativeX { get; init; }

    /// <summary>Vertical position in the usable work area, in the inclusive range 0–1.</summary>
    public double? RelativeY { get; init; }

    [JsonIgnore]
    public bool IsAutomatic => RelativeX is null && RelativeY is null;

    [JsonIgnore]
    public bool IsManual => RelativeX is not null && RelativeY is not null && IsValid;

    /// <summary>
    /// Gets whether this is either the empty automatic placement or a complete finite normalized
    /// manual placement. A half-written or malformed JSON value is deliberately invalid.
    /// </summary>
    [JsonIgnore]
    public bool IsValid =>
        IsAutomatic ||
        (RelativeX is { } x && RelativeY is { } y &&
         double.IsFinite(x) && double.IsFinite(y) &&
         x is >= 0 and <= 1 && y is >= 0 and <= 1);

    /// <summary>Returns a safe persistable value, falling back to automatic placement.</summary>
    public HudPlacement Sanitize()
    {
        if (!IsValid)
        {
            return Default;
        }

        var monitorName = string.IsNullOrWhiteSpace(MonitorDeviceName)
            ? null
            : MonitorDeviceName.Trim();
        return this with { MonitorDeviceName = monitorName };
    }

    public static HudPlacement CreateManual(
        double relativeX,
        double relativeY,
        string? monitorDeviceName = null)
    {
        var placement = new HudPlacement
        {
            MonitorDeviceName = monitorDeviceName,
            RelativeX = relativeX,
            RelativeY = relativeY,
        }.Sanitize();

        if (!placement.IsManual)
        {
            throw new ArgumentOutOfRangeException(
                nameof(relativeX),
                "HUD relative coordinates must both be finite values between 0 and 1.");
        }

        return placement;
    }
}

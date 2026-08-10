using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Alerts;

/// <summary>
/// A latched user alert. Once triggered it remains active until explicitly acknowledged.
/// </summary>
public interface IAlertService : IDisposable
{
    /// <summary>Raised after an active alert has been explicitly acknowledged.</summary>
    event EventHandler? Acknowledged;

    /// <summary>Gets whether an alert is currently latched.</summary>
    bool IsActive { get; }

    /// <summary>
    /// Displays and sounds the alert. Calls made while the alert is active are ignored.
    /// This method may be called from any thread.
    /// </summary>
    /// <param name="detectedText">Optional OCR text to show in the alert.</param>
    /// <param name="anchorRegion">Optional screen region whose monitor hosts the alert card.</param>
    void Trigger(string? detectedText = null, ScreenRegion? anchorRegion = null);

    /// <summary>
    /// Clears the latched alert, closes the overlay, and stops its sound.
    /// This method may be called from any thread.
    /// </summary>
    void Acknowledge();
}

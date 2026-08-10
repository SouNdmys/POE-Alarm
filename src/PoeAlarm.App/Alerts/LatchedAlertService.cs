using System.Windows;
using System.Windows.Threading;
using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Alerts;

/// <summary>
/// Shows a foreground red input shield and loops an audible alarm until acknowledged.
/// </summary>
public sealed class LatchedAlertService : IAlertService
{
    private const string DefaultDetectedText = "目标词缀已命中";

    private readonly object stateGate = new();
    private readonly Dispatcher dispatcher;
    private readonly LoopingAlertSound sound = new();

    // Accessed only on the WPF dispatcher.
    private AffixHitOverlayWindow? overlay;

    private bool isActive;
    private bool isDisposed;
    private int generation;

    /// <summary>Creates an alert service attached to the current WPF application.</summary>
    public LatchedAlertService()
        : this(Application.Current?.Dispatcher
            ?? throw new InvalidOperationException(
                "Create LatchedAlertService after the WPF Application has been initialized."))
    {
    }

    /// <summary>Creates an alert service attached to a specific UI dispatcher.</summary>
    public LatchedAlertService(Dispatcher dispatcher)
    {
        ArgumentNullException.ThrowIfNull(dispatcher);
        this.dispatcher = dispatcher;
    }

    public bool IsActive
    {
        get
        {
            lock (stateGate)
            {
                return isActive;
            }
        }
    }

    public event EventHandler? Acknowledged;

    public void Trigger(string? detectedText = null, ScreenRegion? anchorRegion = null)
    {
        int triggerGeneration;

        lock (stateGate)
        {
            if (isDisposed || isActive)
            {
                return;
            }

            isActive = true;
            triggerGeneration = ++generation;

            // Start audio before queueing the window so a busy UI thread cannot delay the warning.
            sound.Start();
        }

        var displayText = string.IsNullOrWhiteSpace(detectedText)
            ? DefaultDetectedText
            : detectedText.Trim();

        if (!TryDispatch(() => ShowOverlay(displayText, triggerGeneration, anchorRegion)))
        {
            CancelTrigger(triggerGeneration);
        }
    }

    public void Acknowledge()
    {
        int acknowledgementGeneration;
        var transitionedToAcknowledged = false;
        lock (stateGate)
        {
            if (isDisposed)
            {
                return;
            }

            if (isActive)
            {
                isActive = false;
                generation++;
                sound.Stop();
                transitionedToAcknowledged = true;
            }

            acknowledgementGeneration = generation;
        }

        // Closing is harmless when no window exists, so an acknowledgement also repairs any
        // stale overlay left behind after an unexpected presentation error.
        _ = TryDispatch(() => CompleteAcknowledgement(
            acknowledgementGeneration,
            transitionedToAcknowledged));
    }

    public void Dispose()
    {
        lock (stateGate)
        {
            if (isDisposed)
            {
                return;
            }

            isDisposed = true;
            isActive = false;
            generation++;
            sound.Stop();
            sound.Dispose();
        }

        _ = TryDispatch(CloseOverlay);
        GC.SuppressFinalize(this);
    }

    private void ShowOverlay(
        string detectedText,
        int triggerGeneration,
        ScreenRegion? anchorRegion)
    {
        lock (stateGate)
        {
            if (isDisposed || !isActive || generation != triggerGeneration)
            {
                return;
            }
        }

        try
        {
            overlay ??= CreateOverlay();
            overlay.Arm(detectedText, triggerGeneration, anchorRegion);

            if (!overlay.IsVisible)
            {
                overlay.Show();
            }

            overlay.ReassertTopmostPosition();
        }
        catch (InvalidOperationException)
        {
            // A WPF window can no longer be shown after it has closed. Recreate it once before
            // treating presentation as failed.
            try
            {
                overlay = CreateOverlay();
                overlay.Arm(detectedText, triggerGeneration, anchorRegion);
                overlay.Show();
                overlay.ReassertTopmostPosition();
            }
            catch (InvalidOperationException)
            {
                CloseOverlay();
                CancelTrigger(triggerGeneration);
            }
        }
    }

    private void CancelTrigger(int triggerGeneration)
    {
        lock (stateGate)
        {
            if (!isDisposed && isActive && generation == triggerGeneration)
            {
                isActive = false;
                generation++;
                sound.Stop();
            }
        }
    }

    private void CloseOverlay()
    {
        if (overlay is null)
        {
            return;
        }

        var window = overlay;
        try
        {
            window.CloseFromService();
        }
        finally
        {
            if (ReferenceEquals(overlay, window))
            {
                overlay = null;
            }
        }
    }

    private void CompleteAcknowledgement(
        int acknowledgementGeneration,
        bool raiseAcknowledged)
    {
        lock (stateGate)
        {
            if (isDisposed || isActive || generation != acknowledgementGeneration)
            {
                return;
            }
        }

        CloseOverlay();
        if (raiseAcknowledged)
        {
            Acknowledged?.Invoke(this, EventArgs.Empty);
        }
    }

    private AffixHitOverlayWindow CreateOverlay()
    {
        var window = new AffixHitOverlayWindow();
        window.AcknowledgeRequested += OnOverlayAcknowledgementRequested;
        return window;
    }

    private void OnOverlayAcknowledgementRequested(
        object? sender,
        OverlayAcknowledgementRequestedEventArgs e)
    {
        if (!ReferenceEquals(sender, overlay))
        {
            return;
        }

        Acknowledge(e.Generation);
    }

    private void Acknowledge(int expectedGeneration)
    {
        int acknowledgementGeneration;
        lock (stateGate)
        {
            if (isDisposed || !isActive || generation != expectedGeneration)
            {
                return;
            }

            isActive = false;
            generation++;
            sound.Stop();
            acknowledgementGeneration = generation;
        }

        _ = TryDispatch(() => CompleteAcknowledgement(
            acknowledgementGeneration,
            raiseAcknowledged: true));
    }

    private bool TryDispatch(Action action)
    {
        if (dispatcher.HasShutdownStarted || dispatcher.HasShutdownFinished)
        {
            return false;
        }

        try
        {
            if (dispatcher.CheckAccess())
            {
                action();
            }
            else
            {
                _ = dispatcher.BeginInvoke(DispatcherPriority.Send, action);
            }

            return true;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
        catch (TaskCanceledException)
        {
            return false;
        }
    }
}

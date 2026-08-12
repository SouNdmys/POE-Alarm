using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Recognition;

/// <summary>
/// Supplies a cheap, OCR-semantic fingerprint before recognition starts. Recognizers that
/// implement this contract opt into the monitor's transient input guard for changed frames.
/// </summary>
public interface IFrameFingerprintProvider
{
    ulong ComputeFrameFingerprint(CapturedFrame frame);
}

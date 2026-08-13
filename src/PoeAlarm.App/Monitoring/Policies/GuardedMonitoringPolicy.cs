namespace PoeAlarm.App.Monitoring.Policies;

/// <summary>
/// Mirror-tier policy: every changed semantic tooltip is held behind an input gate until a
/// stable, explainable decision has been reached.
/// </summary>
public sealed class GuardedMonitoringPolicy : IMonitoringPolicy
{
    public GuardedMonitoringPolicy(GuardedPolicyOptions? options = null)
    {
        Options = (options ?? new GuardedPolicyOptions()).Validate();
    }

    public MonitoringPolicyKind Kind => MonitoringPolicyKind.Guarded;

    public GuardedPolicyOptions Options { get; }
}

public sealed record GuardedPolicyOptions
{
    /// <summary>Number of identical semantic fingerprints required before OCR starts.</summary>
    public int RequiredStableFrames { get; init; } = 2;

    /// <summary>
    /// Number of identical target-relevant OCR decisions required before release. The production
    /// default is one strict decision after two stable frames; recognizers may internally require
    /// progressive or independent confirmation. Higher values remain available for replay tests,
    /// but are not advertised as independent OCR when a recognizer serves a cached observation.
    /// </summary>
    public int RequiredConsistentRecognitions { get; init; } = 1;

    /// <summary>
    /// Maximum changed fingerprints observed while looking for stability. This is a count, not a
    /// fixed sleep: reaching it yields an explainable yellow pause.
    /// </summary>
    public int MaximumUnstableTransitions { get; init; } = 8;

    /// <summary>Maximum recognizer-requested progressive rescans before a yellow pause.</summary>
    public int MaximumRecognitionRescans { get; init; } = 8;

    /// <summary>
    /// Hard fail-open watchdog for capture/OCR/decision work. It stops the Guarded session rather
    /// than continuing after protection can no longer be guaranteed.
    /// </summary>
    public TimeSpan DecisionTimeout { get; init; } = TimeSpan.FromMilliseconds(1500);

    internal GuardedPolicyOptions Validate()
    {
        if (RequiredStableFrames is < 1 or > 8)
        {
            throw new ArgumentOutOfRangeException(
                nameof(RequiredStableFrames),
                "Required stable frames must be between 1 and 8.");
        }

        if (RequiredConsistentRecognitions is < 1 or > 4)
        {
            throw new ArgumentOutOfRangeException(
                nameof(RequiredConsistentRecognitions),
                "Required consistent recognitions must be between 1 and 4.");
        }

        if (MaximumUnstableTransitions is < 1 or > 64)
        {
            throw new ArgumentOutOfRangeException(
                nameof(MaximumUnstableTransitions),
                "Maximum unstable transitions must be between 1 and 64.");
        }

        if (MaximumRecognitionRescans is < 1 or > 64)
        {
            throw new ArgumentOutOfRangeException(
                nameof(MaximumRecognitionRescans),
                "Maximum recognition rescans must be between 1 and 64.");
        }

        if (DecisionTimeout < TimeSpan.FromMilliseconds(50) ||
            DecisionTimeout > TimeSpan.FromSeconds(5))
        {
            throw new ArgumentOutOfRangeException(
                nameof(DecisionTimeout),
                "The Guarded decision watchdog must be between 50 ms and 5 seconds.");
        }

        return this;
    }
}

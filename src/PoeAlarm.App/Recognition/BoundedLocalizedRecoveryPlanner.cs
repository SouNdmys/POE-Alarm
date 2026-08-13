namespace PoeAlarm.App.Recognition;

/// <summary>
/// Selects the small, deterministic set of localized candidates that may spend expensive OCR
/// work in this scan. The work limit is independent of the number of configured rule targets.
/// </summary>
internal static class BoundedLocalizedRecoveryPlanner
{
    public static BoundedLocalizedRecoveryPlan<T> Plan<T>(
        IReadOnlyList<T> candidates,
        int maximumWorkUnits)
    {
        ArgumentNullException.ThrowIfNull(candidates);
        if (maximumWorkUnits < 0)
        {
            throw new ArgumentOutOfRangeException(nameof(maximumWorkUnits));
        }

        var workCount = Math.Min(candidates.Count, maximumWorkUnits);
        return new BoundedLocalizedRecoveryPlan<T>(
            candidates.Take(workCount).ToArray());
    }
}

internal sealed record BoundedLocalizedRecoveryPlan<T>(
    IReadOnlyList<T> WorkItems);

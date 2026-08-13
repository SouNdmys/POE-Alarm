namespace PoeAlarm.App.Recognition;

/// <summary>
/// Splits conservative localized candidates into the small set that may spend expensive OCR work
/// in this scan and the remainder that must stay visible as unresolved Guarded evidence.
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
            candidates.Take(workCount).ToArray(),
            candidates.Skip(workCount).ToArray());
    }
}

internal sealed record BoundedLocalizedRecoveryPlan<T>(
    IReadOnlyList<T> WorkItems,
    IReadOnlyList<T> DeferredItems);

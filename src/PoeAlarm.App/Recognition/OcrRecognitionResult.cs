using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Recognition;

public sealed record OcrRecognitionResult(
    IReadOnlyList<string> Lines,
    TimeSpan PreprocessingElapsed,
    TimeSpan RecognitionElapsed,
    bool WasCached = false,
    LogicalAffixMatch? TargetAssistedMatch = null,
    bool RequiresRescan = false,
    IReadOnlyList<OcrAssistedObservation>? AssistedObservations = null,
    IReadOnlyList<OcrPhysicalLine>? PhysicalLines = null)
{
    public TimeSpan TotalElapsed => PreprocessingElapsed + RecognitionElapsed;

    /// <summary>
    /// Target-conditioned transcripts produced from localized, independently verified physical
    /// rows.  Unlike <see cref="TargetAssistedMatch"/>, a batch may carry several observations
    /// while retaining the source-band identity needed for one-to-one rule assignment.
    /// </summary>
    public IReadOnlyList<OcrAssistedObservation> BatchAssistedObservations =>
        AssistedObservations ?? [];

    /// <summary>
    /// Optional identity-bearing projection of <see cref="Lines"/>. Batch recognizers populate
    /// this so strict primary and assisted transcripts from one row share a physical component.
    /// Empty logical-boundary entries intentionally have no projection.
    /// </summary>
    public IReadOnlyList<OcrPhysicalLine> BatchPhysicalLines => PhysicalLines ?? [];
}

/// <summary>
/// Maps one non-empty entry in <see cref="OcrRecognitionResult.Lines"/> to its source row. The
/// id is frame-local and is shared by every assisted alternative derived from that row.
/// </summary>
public sealed record OcrPhysicalLine(
    int LineIndex,
    string PhysicalBandId,
    int SourceTop,
    int SourceBottom);

/// <param name="PhysicalBandId">
/// Stable within this recognition result. Equal ids mean that two target-conditioned transcripts
/// describe the same physical tooltip row and therefore must not satisfy two rule conditions.
/// </param>
public sealed record OcrAssistedObservation(
    string PhysicalBandId,
    string OriginalText,
    string CanonicalTarget,
    int SourceTop,
    int SourceBottom,
    IReadOnlyList<string>? RelatedPhysicalBandIds = null)
{
    public IReadOnlyList<string> PhysicalBandIds =>
        RelatedPhysicalBandIds is { Count: > 0 }
            ? RelatedPhysicalBandIds
            : [PhysicalBandId];
}

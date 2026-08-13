using PoeAlarm.Core.Matching;

namespace PoeAlarm.Core.Rules;

/// <summary>A strict localized transcript tied to one physical tooltip band.</summary>
public sealed record AssistedModifierObservation(
    string PhysicalBandId,
    string OriginalText,
    string CanonicalTarget,
    IReadOnlyList<string>? RelatedPhysicalBandIds = null)
{
    public IReadOnlyList<string> PhysicalBandIds =>
        RelatedPhysicalBandIds is { Count: > 0 }
            ? RelatedPhysicalBandIds
            : [PhysicalBandId];
}

/// <summary>Maps one primary OCR line back to its physical tooltip band.</summary>
public sealed record PhysicalLineIdentity(
    int LineIndex,
    string PhysicalBandId);

public sealed record ModifierObservation(
    int StartLineIndex,
    int PhysicalLineCount,
    string OriginalText,
    string CanonicalText,
    IReadOnlyList<decimal?> NumericValues,
    string? PhysicalBandId = null,
    IReadOnlyList<string>? PhysicalBandIds = null);

public sealed record NumericSlotEvidence(
    int SlotIndex,
    decimal? ActualValue,
    NumericConstraint Constraint,
    bool IsSatisfied)
{
    public bool IsMatch => IsSatisfied;
}

public sealed record ConditionEvaluation(
    int ConditionIndex,
    string Name,
    string Template,
    bool IsMatched,
    ModifierObservation? Observation,
    IReadOnlyList<NumericSlotEvidence> NumericSlots)
{
    public string ConditionId => Name;

    public bool IsMatch => IsMatched;

    public bool TextMatched => Observation is not null;

    public bool NumericConstraintsMatched =>
        TextMatched && NumericSlots.All(static slot => slot.IsSatisfied);

    /// <summary>
    /// The complete target text was identified, but OCR did not produce a value for one of the
    /// slots that the rule actually constrains. Fast monitoring treats this as a miss; guarded
    /// monitoring may pause for manual confirmation. Ignored slots never make evidence uncertain.
    /// </summary>
    public bool HasMissingRequiredNumericValue =>
        TextMatched && NumericSlots.Any(static slot =>
            slot.Constraint.Mode != NumericConstraintMode.Ignore && slot.ActualValue is null);

    public int? StartLineIndex => Observation?.StartLineIndex;

    public int? PhysicalLineCount => Observation?.PhysicalLineCount;

    public string? ObservedText => Observation?.OriginalText;

    public IReadOnlyList<decimal?> ActualValues => Observation?.NumericValues ?? [];

    public string? FailureReason => IsMatched
        ? null
        : !TextMatched
            ? "No complete affix text matched."
            : !NumericConstraintsMatched
                ? "The affix text matched, but one or more numeric constraints failed."
                : "The matching modifier was already assigned to another condition in this result.";
}

public sealed record ResultGroupEvaluation(
    int GroupIndex,
    string Name,
    ResultGroupMode Mode,
    int RequiredCount,
    int MatchedCount,
    bool IsMatch,
    IReadOnlyList<ConditionEvaluation> Conditions)
{
    public string GroupId => Name;

    public int MatchedConditionCount => MatchedCount;
}

public sealed record RuleEvaluationResult(
    bool IsMatch,
    int? MatchedGroupIndex,
    IReadOnlyList<ResultGroupEvaluation> Groups)
{
    public ResultGroupEvaluation? MatchedGroup =>
        MatchedGroupIndex is int index ? Groups[index] : null;

    public string? MatchedGroupId => MatchedGroup?.Name;
}

public sealed class RuleValidationException : ArgumentException
{
    public RuleValidationException(IReadOnlyList<string> errors, Exception? innerException = null)
        : base(string.Join(Environment.NewLine, errors), innerException)
    {
        Errors = errors;
    }

    public IReadOnlyList<string> Errors { get; }
}

public static class RuleCompiler
{
    public static CompiledRuleSet Compile(
        RuleSetDefinition definition,
        int maximumPhysicalLineSpan = FullLineAffixMatcher.MaximumPhysicalLineSpan) =>
        CompileCore(definition, maximumPhysicalLineSpan);

    private static CompiledRuleSet CompileCore(
        RuleSetDefinition definition,
        int maximumPhysicalLineSpan)
    {
        try
        {
            return new CompiledRuleSet(definition, maximumPhysicalLineSpan);
        }
        catch (RuleValidationException)
        {
            throw;
        }
        catch (ArgumentException exception)
        {
            throw new RuleValidationException([exception.Message], exception);
        }
    }
}

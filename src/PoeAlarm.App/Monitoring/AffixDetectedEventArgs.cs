using PoeAlarm.Core.Matching;
using PoeAlarm.Core.Rules;

namespace PoeAlarm.App.Monitoring;

public sealed class AffixDetectedEventArgs(
    LogicalAffixMatch match,
    MonitorSnapshot snapshot) : EventArgs
{
    public LogicalAffixMatch Match { get; } = match;

    public string DetectedText { get; } = match.OriginalText;

    public RuleEvaluationResult? RuleEvaluation { get; }

    public MonitorSnapshot Snapshot { get; } = snapshot;

    public AffixDetectedEventArgs(
        RuleEvaluationResult ruleEvaluation,
        string detectedText,
        MonitorSnapshot snapshot)
        : this(CreateCompatibilityMatch(ruleEvaluation, detectedText), snapshot)
    {
        ArgumentNullException.ThrowIfNull(ruleEvaluation);
        RuleEvaluation = ruleEvaluation;
        DetectedText = detectedText;
    }

    private static LogicalAffixMatch CreateCompatibilityMatch(
        RuleEvaluationResult ruleEvaluation,
        string detectedText)
    {
        ArgumentNullException.ThrowIfNull(ruleEvaluation);
        var condition = ruleEvaluation.MatchedGroup?.Conditions
            .FirstOrDefault(static item => item.IsMatched && item.Observation is not null);
        return new LogicalAffixMatch(
            condition?.Observation?.StartLineIndex ?? 0,
            condition?.Observation?.PhysicalLineCount ?? 1,
            detectedText,
            condition?.Observation?.CanonicalText ?? string.Empty);
    }
}

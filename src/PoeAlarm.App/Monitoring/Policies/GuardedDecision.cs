using PoeAlarm.Core.Rules;

namespace PoeAlarm.App.Monitoring.Policies;

public enum GuardedSessionState
{
    Stopped,
    EstablishingBaseline,
    WaitingForChange,
    WaitingForStableFrame,
    Evaluating,
    PausedUncertain,
    MatchFound,
    Faulted,
}

public enum GuardedDecisionKind
{
    NoMatch,
    Match,
    Uncertain,
    Faulted,
}

public enum GuardedUncertaintyReason
{
    TargetLexicalCandidateUnverified,
    TargetNumericValueMissing,
    TargetNumericValueConflict,
    FrameUnstable,
}

public enum GuardedFaultReason
{
    InputGuardUnavailable,
    DecisionTimedOut,
    CaptureOrRecognitionFailed,
}

public sealed class GuardedDecisionEventArgs : EventArgs
{
    public GuardedDecisionEventArgs(
        GuardedDecisionKind decision,
        GuardedSessionState state,
        string detail,
        GuardedUncertaintyReason? uncertaintyReason = null,
        GuardedFaultReason? faultReason = null,
        RuleEvaluationResult? evaluation = null,
        MonitorSnapshot? snapshot = null)
    {
        Decision = decision;
        State = state;
        Detail = detail;
        UncertaintyReason = uncertaintyReason;
        FaultReason = faultReason;
        Evaluation = evaluation;
        Snapshot = snapshot;
    }

    public GuardedDecisionKind Decision { get; }

    public GuardedSessionState State { get; }

    public string Detail { get; }

    public GuardedUncertaintyReason? UncertaintyReason { get; }

    public GuardedFaultReason? FaultReason { get; }

    public RuleEvaluationResult? Evaluation { get; }

    public MonitorSnapshot? Snapshot { get; }
}

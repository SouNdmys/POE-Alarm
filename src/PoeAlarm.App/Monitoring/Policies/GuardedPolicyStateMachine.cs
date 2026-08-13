namespace PoeAlarm.App.Monitoring.Policies;

/// <summary>
/// Deterministic, clock-agnostic Guarded transition core. Native input and OCR live outside this
/// type; tests can drive frames and time directly without sleeping.
/// </summary>
internal sealed class GuardedPolicyStateMachine
{
    private readonly GuardedPolicyOptions _options;
    private ulong? _acceptedFingerprint;
    private ulong? _candidateFingerprint;
    private int _stableFrameCount;
    private int _unstableTransitions;
    private int _recognitionRescans;
    private string? _previousEvidenceSignature;
    private bool _previousHadTargetCandidate;
    private int _consistentRecognitionCount;
    private DateTimeOffset? _decisionStartedAt;

    public GuardedPolicyStateMachine(GuardedPolicyOptions options)
    {
        _options = options.Validate();
    }

    public GuardedSessionState State { get; private set; } = GuardedSessionState.WaitingForChange;

    public bool IsInputGuardHeld { get; private set; }

    public ulong? CandidateFingerprint => _candidateFingerprint;

    public GuardedFrameTransition ObserveFrame(ulong fingerprint, DateTimeOffset now)
    {
        if (State is GuardedSessionState.PausedUncertain or
            GuardedSessionState.MatchFound or
            GuardedSessionState.Faulted or
            GuardedSessionState.Stopped)
        {
            return GuardedFrameTransition.None(State);
        }

        if (HasTimedOut(now))
        {
            return Fault(
                "Guarded decision watchdog expired.",
                GuardedFaultReason.DecisionTimedOut);
        }

        if (!IsInputGuardHeld)
        {
            if (_acceptedFingerprint == fingerprint)
            {
                State = GuardedSessionState.WaitingForChange;
                return GuardedFrameTransition.None(State);
            }

            BeginDecision(fingerprint, now);
            return new GuardedFrameTransition(
                State,
                RequestInputGuard: true,
                ReleaseInputGuard: false,
                ShouldRecognize: State == GuardedSessionState.Evaluating,
                Decision: null);
        }

        if (_candidateFingerprint != fingerprint)
        {
            _candidateFingerprint = fingerprint;
            _stableFrameCount = 1;
            _recognitionRescans = 0;
            _previousEvidenceSignature = null;
            _previousHadTargetCandidate = false;
            _consistentRecognitionCount = 0;
            _unstableTransitions++;
            if (_unstableTransitions >= _options.MaximumUnstableTransitions)
            {
                return Pause(
                    GuardedUncertaintyReason.FrameUnstable,
                    "The tooltip fingerprint did not stabilize before the Guarded limit.");
            }

            State = _stableFrameCount >= _options.RequiredStableFrames
                ? GuardedSessionState.Evaluating
                : GuardedSessionState.WaitingForStableFrame;
            return new GuardedFrameTransition(
                State,
                RequestInputGuard: true,
                ReleaseInputGuard: false,
                ShouldRecognize: State == GuardedSessionState.Evaluating,
                Decision: null);
        }

        if (State == GuardedSessionState.WaitingForStableFrame)
        {
            _stableFrameCount++;
            if (_stableFrameCount >= _options.RequiredStableFrames)
            {
                State = GuardedSessionState.Evaluating;
            }
        }

        return new GuardedFrameTransition(
            State,
            RequestInputGuard: true,
            ReleaseInputGuard: false,
            ShouldRecognize: State == GuardedSessionState.Evaluating,
            Decision: null);
    }

    public GuardedFrameTransition ObserveRecognition(GuardedRuleEvidence evidence)
    {
        ArgumentNullException.ThrowIfNull(evidence);
        if (State != GuardedSessionState.Evaluating || !IsInputGuardHeld)
        {
            throw new InvalidOperationException("Guarded is not waiting for a recognition result.");
        }

        if (evidence.RequiresRescan)
        {
            _recognitionRescans++;
            if (_recognitionRescans >= _options.MaximumRecognitionRescans)
            {
                return Pause(
                    GuardedUncertaintyReason.FrameUnstable,
                    "OCR did not produce a final result before the Guarded rescan limit.");
            }

            return new GuardedFrameTransition(
                State,
                RequestInputGuard: true,
                ReleaseInputGuard: false,
                ShouldRecognize: false,
                Decision: null);
        }

        _recognitionRescans = 0;
        // A final OCR result is valid only for the exact stable semantic frame that entered the
        // evaluator. The monitor captures again before every independent confirmation.
        if (evidence.HasConflictingTargetNumericEvidence)
        {
            return Pause(
                GuardedUncertaintyReason.TargetNumericValueConflict,
                "Target-related OCR passes reported conflicting numeric values.");
        }

        if (evidence.HasUnverifiedTargetCandidate)
        {
            return Pause(
                GuardedUncertaintyReason.TargetLexicalCandidateUnverified,
                "A localized target-like modifier could not be independently verified.");
        }

        if (evidence.HasMissingRequiredNumericValue)
        {
            return Pause(
                GuardedUncertaintyReason.TargetNumericValueMissing,
                "A target modifier candidate was found, but a constrained value is unreadable.");
        }

        if (_previousEvidenceSignature is not null &&
            !string.Equals(
                _previousEvidenceSignature,
                evidence.TargetEvidenceSignature,
                StringComparison.Ordinal) &&
            (_previousHadTargetCandidate || evidence.HasTargetCandidate))
        {
            return Pause(
                GuardedUncertaintyReason.TargetNumericValueConflict,
                "Stable-frame OCR produced conflicting target-related evidence.");
        }

        if (string.Equals(
                _previousEvidenceSignature,
                evidence.TargetEvidenceSignature,
                StringComparison.Ordinal))
        {
            _consistentRecognitionCount++;
        }
        else
        {
            _previousEvidenceSignature = evidence.TargetEvidenceSignature;
            _previousHadTargetCandidate = evidence.HasTargetCandidate;
            _consistentRecognitionCount = 1;
        }

        if (_consistentRecognitionCount < _options.RequiredConsistentRecognitions)
        {
            State = GuardedSessionState.WaitingForStableFrame;
            _stableFrameCount = 0;
            return new GuardedFrameTransition(
                State,
                RequestInputGuard: true,
                ReleaseInputGuard: false,
                ShouldRecognize: false,
                Decision: null);
        }

        if (evidence.IsMatch)
        {
            State = GuardedSessionState.MatchFound;
            return new GuardedFrameTransition(
                State,
                RequestInputGuard: false,
                ReleaseInputGuard: false,
                ShouldRecognize: false,
                Decision: new GuardedDecision(
                    GuardedDecisionKind.Match,
                    null,
                    "A stable tooltip state matched an acceptable result."));
        }

        _acceptedFingerprint = _candidateFingerprint;
        ResetDecision();
        State = GuardedSessionState.WaitingForChange;
        return new GuardedFrameTransition(
            State,
            RequestInputGuard: false,
            ReleaseInputGuard: false,
            ShouldRecognize: false,
            Decision: new GuardedDecision(
                GuardedDecisionKind.NoMatch,
                null,
                "The stable tooltip state did not match any acceptable result."),
            AwaitNextCausativeClick: true);
    }

    public GuardedFrameTransition ContinueAfterUncertain()
    {
        if (State != GuardedSessionState.PausedUncertain)
        {
            return GuardedFrameTransition.None(State);
        }

        _acceptedFingerprint = _candidateFingerprint;
        ResetDecision();
        State = GuardedSessionState.WaitingForChange;
        return new GuardedFrameTransition(
            State,
            RequestInputGuard: false,
            ReleaseInputGuard: false,
            ShouldRecognize: false,
            Decision: null,
            AwaitNextCausativeClick: true);
    }

    public GuardedFrameTransition Stop()
    {
        ResetDecision();
        State = GuardedSessionState.Stopped;
        return new GuardedFrameTransition(
            State,
            RequestInputGuard: false,
            // Stop is terminal for both a held decision gate and the pass-one-click phase.
            ReleaseInputGuard: true,
            ShouldRecognize: false,
            Decision: null);
    }

    public GuardedFrameTransition Fault(
        string detail,
        GuardedFaultReason reason = GuardedFaultReason.CaptureOrRecognitionFailed)
    {
        ResetDecision();
        State = GuardedSessionState.Faulted;
        return new GuardedFrameTransition(
            State,
            RequestInputGuard: false,
            // Fault is terminal for both a held decision gate and the pass-one-click phase.
            ReleaseInputGuard: true,
            ShouldRecognize: false,
            Decision: new GuardedDecision(
                GuardedDecisionKind.Faulted,
                null,
                detail,
                reason));
    }

    public TimeSpan RemainingDecisionTime(DateTimeOffset now)
    {
        if (_decisionStartedAt is null)
        {
            return _options.DecisionTimeout;
        }

        var remaining = _options.DecisionTimeout - (now - _decisionStartedAt.Value);
        return remaining > TimeSpan.Zero ? remaining : TimeSpan.Zero;
    }

    private void BeginDecision(ulong fingerprint, DateTimeOffset now)
    {
        _candidateFingerprint = fingerprint;
        _stableFrameCount = 1;
        _unstableTransitions = 0;
        _recognitionRescans = 0;
        _previousEvidenceSignature = null;
        _previousHadTargetCandidate = false;
        _consistentRecognitionCount = 0;
        _decisionStartedAt = now;
        IsInputGuardHeld = true;
        State = _stableFrameCount >= _options.RequiredStableFrames
            ? GuardedSessionState.Evaluating
            : GuardedSessionState.WaitingForStableFrame;
    }

    private GuardedFrameTransition Pause(
        GuardedUncertaintyReason reason,
        string detail)
    {
        State = GuardedSessionState.PausedUncertain;
        return new GuardedFrameTransition(
            State,
            RequestInputGuard: false,
            ReleaseInputGuard: false,
            ShouldRecognize: false,
            Decision: new GuardedDecision(GuardedDecisionKind.Uncertain, reason, detail));
    }

    private bool HasTimedOut(DateTimeOffset now) =>
        IsInputGuardHeld &&
        _decisionStartedAt is not null &&
        now - _decisionStartedAt.Value >= _options.DecisionTimeout;

    private void ResetDecision()
    {
        _candidateFingerprint = null;
        _stableFrameCount = 0;
        _unstableTransitions = 0;
        _recognitionRescans = 0;
        _previousEvidenceSignature = null;
        _previousHadTargetCandidate = false;
        _consistentRecognitionCount = 0;
        _decisionStartedAt = null;
        IsInputGuardHeld = false;
    }
}

internal sealed record GuardedRuleEvidence(
    bool IsMatch,
    bool RequiresRescan,
    bool HasTargetCandidate,
    bool HasUnverifiedTargetCandidate,
    bool HasMissingRequiredNumericValue,
    bool HasConflictingTargetNumericEvidence,
    string TargetEvidenceSignature);

internal sealed record GuardedDecision(
    GuardedDecisionKind Kind,
    GuardedUncertaintyReason? UncertaintyReason,
    string Detail,
    GuardedFaultReason? FaultReason = null);

internal readonly record struct GuardedFrameTransition(
    GuardedSessionState State,
    bool RequestInputGuard,
    bool ReleaseInputGuard,
    bool ShouldRecognize,
    GuardedDecision? Decision,
    bool AwaitNextCausativeClick = false)
{
    public static GuardedFrameTransition None(GuardedSessionState state) =>
        new(state, false, false, false, null);
}

using System.Reflection;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Monitoring;
using PoeAlarm.App.Monitoring.Policies;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;
using PoeAlarm.Core.Rules;

var tests = new (string Name, Action Run)[]
{
    ("changed frame waits for stability behind guard", ChangedFrameWaitsForStability),
    ("stable no-match releases without fixed delay", StableNoMatchReleases),
    ("stable match remains guarded", StableMatchRemainsGuarded),
    ("missing target numeric value pauses", MissingNumericPauses),
    ("unverified target lexical candidate pauses", UnverifiedLexicalCandidatePauses),
    ("conflicting target numeric evidence pauses", ConflictingNumericPauses),
    ("recognition confirmation rechecks frame stability", ConfirmationRechecksFrame),
    ("unrelated OCR noise does not pause", UnrelatedNoiseDoesNotPause),
    ("explicit out-of-range value is a no-match", OutOfRangeIsNoMatch),
    ("unstable frames pause with evidence", UnstableFramesPause),
    ("progressive rescan has bounded pause", ProgressiveRescanIsBounded),
    ("manual continue releases and advances baseline", ManualContinueReleases),
    ("manual stop always releases", ManualStopReleases),
    ("watchdog faults and fails open", WatchdogFaultsOpen),
    ("exception fault transition releases", ExceptionFaultReleases),
    ("match handoff failure faults and cancels the guarded session",
        () => MatchHandoffFailureFaultsAndCancelsAsync().GetAwaiter().GetResult()),
    ("guard state machine drains suppressed input pairs", InputPairsDrainWithoutReplay),
    ("guard waits for click causing change to release", ExistingClickCompletesBeforeBlocking),
    ("guard passes exactly one complete causative click", OneCausativeClickPassesThenGuards),
    ("mid-click miss awaits a fresh causative cycle", MidCausativeMissAwaitsFreshClick),
    ("await drains every overlapping suppressed pair", DrainingToAwaitHandlesOverlap),
    ("initial held input cannot complete a causative cycle", InitialHeldInputDoesNotCompleteCycle),
};

var passed = 0;
foreach (var test in tests)
{
    try
    {
        test.Run();
        passed++;
        Console.WriteLine($"PASS {test.Name}");
    }
    catch (Exception exception)
    {
        Console.Error.WriteLine($"FAIL {test.Name}: {exception.Message}");
    }
}

Console.WriteLine($"{passed}/{tests.Length} Guarded policy tests passed.");
return passed == tests.Length ? 0 : 1;

static void ChangedFrameWaitsForStability()
{
    var (machine, now) = Machine(stableFrames: 2, recognitions: 2);
    var first = machine.ObserveFrame(10, now);
    Assert(first.RequestInputGuard, "Changed frame did not synchronously request the guard.");
    Assert(!first.ShouldRecognize && machine.State == GuardedSessionState.WaitingForStableFrame,
        "OCR ran before the semantic frame was stable.");

    var second = machine.ObserveFrame(10, now.AddMilliseconds(1));
    Assert(second.RequestInputGuard && second.ShouldRecognize,
        "Second identical frame did not refresh the guard and start OCR.");
}

static void StableNoMatchReleases()
{
    var (machine, now) = Machine(stableFrames: 1, recognitions: 2);
    Assert(machine.ObserveFrame(10, now).ShouldRecognize, "First stable frame should recognize.");
    var first = machine.ObserveRecognition(Evidence(false, "none"));
    Assert(first.Decision is null && !first.ReleaseInputGuard,
        "One recognition released before the consistency threshold.");
    Assert(machine.ObserveFrame(10, now.AddMilliseconds(1)).ShouldRecognize,
        "Same stable frame did not request the confirming OCR.");
    var second = machine.ObserveRecognition(Evidence(false, "none"));
    Assert(second.Decision?.Kind == GuardedDecisionKind.NoMatch &&
           second.AwaitNextCausativeClick && !second.ReleaseInputGuard,
        "Confirmed no-match did not enter the one-click Guarded pass cycle.");
    Assert(machine.State == GuardedSessionState.WaitingForChange && !machine.IsInputGuardHeld,
        "No-match did not return to the change wait state.");
}

static void StableMatchRemainsGuarded()
{
    var (machine, now) = Machine(stableFrames: 1, recognitions: 1);
    _ = machine.ObserveFrame(10, now);
    var result = machine.ObserveRecognition(Evidence(true, "crit:50", candidate: true));
    Assert(result.Decision?.Kind == GuardedDecisionKind.Match,
        "A stable matching roll was not classified as Match.");
    Assert(!result.ReleaseInputGuard && machine.IsInputGuardHeld,
        "Match released the guard before the red overlay could take ownership.");
}

static void MissingNumericPauses()
{
    var (machine, now) = Machine(stableFrames: 1, recognitions: 1);
    _ = machine.ObserveFrame(10, now);
    var result = machine.ObserveRecognition(Evidence(
        false,
        "crit:?",
        candidate: true,
        missing: true));
    Assert(result.Decision?.Kind == GuardedDecisionKind.Uncertain &&
           result.Decision.UncertaintyReason == GuardedUncertaintyReason.TargetNumericValueMissing,
        "Missing constrained target value was not explainably paused.");
    Assert(machine.IsInputGuardHeld && !result.ReleaseInputGuard,
        "Uncertain result released input before a human decision.");
}

static void UnverifiedLexicalCandidatePauses()
{
    var (machine, now) = Machine(stableFrames: 1, recognitions: 1);
    _ = machine.ObserveFrame(10, now);
    var result = machine.ObserveRecognition(new GuardedRuleEvidence(
        IsMatch: false,
        RequiresRescan: false,
        HasTargetCandidate: true,
        HasUnverifiedTargetCandidate: true,
        HasMissingRequiredNumericValue: false,
        HasConflictingTargetNumericEvidence: false,
        TargetEvidenceSignature: "candidate"));
    Assert(result.Decision?.UncertaintyReason ==
           GuardedUncertaintyReason.TargetLexicalCandidateUnverified,
        "Unverified localized target candidate was allowed through as NoMatch.");
}

static void ConflictingNumericPauses()
{
    var (machine, now) = Machine(stableFrames: 1, recognitions: 2);
    _ = machine.ObserveFrame(10, now);
    _ = machine.ObserveRecognition(Evidence(false, "crit:45", candidate: true));
    _ = machine.ObserveFrame(10, now.AddMilliseconds(1));
    var result = machine.ObserveRecognition(Evidence(false, "crit:54", candidate: true));
    Assert(result.Decision?.UncertaintyReason ==
           GuardedUncertaintyReason.TargetNumericValueConflict,
        "Conflicting target values were not paused.");
}

static void ConfirmationRechecksFrame()
{
    var (machine, now) = Machine(stableFrames: 2, recognitions: 2);
    _ = machine.ObserveFrame(10, now);
    _ = machine.ObserveFrame(10, now.AddMilliseconds(1));
    _ = machine.ObserveRecognition(Evidence(false, "none"));
    var changed = machine.ObserveFrame(11, now.AddMilliseconds(2));
    Assert(!changed.ShouldRecognize &&
           machine.State == GuardedSessionState.WaitingForStableFrame,
        "A changed frame reused the previous recognition as confirmation.");
    var stable = machine.ObserveFrame(11, now.AddMilliseconds(3));
    Assert(stable.ShouldRecognize,
        "The replacement frame did not regain stability for recognition.");
}

static void UnrelatedNoiseDoesNotPause()
{
    var (machine, now) = Machine(stableFrames: 1, recognitions: 2);
    _ = machine.ObserveFrame(10, now);
    _ = machine.ObserveRecognition(Evidence(false, "none"));
    _ = machine.ObserveFrame(10, now.AddMilliseconds(1));
    // Complete OCR output may differ, but the Guarded adapter deliberately reduces it to the
    // same target-only "none" signature.
    var result = machine.ObserveRecognition(Evidence(false, "none"));
    Assert(result.Decision?.Kind == GuardedDecisionKind.NoMatch &&
           result.AwaitNextCausativeClick && !result.ReleaseInputGuard,
        "Non-target OCR noise did not enter the safe one-click pass cycle.");
}

static void OutOfRangeIsNoMatch()
{
    var (machine, now) = Machine(stableFrames: 1, recognitions: 1);
    _ = machine.ObserveFrame(10, now);
    var result = machine.ObserveRecognition(Evidence(
        false,
        "crit:42:false",
        candidate: true));
    Assert(result.Decision?.Kind == GuardedDecisionKind.NoMatch,
        "A readable but out-of-range number was incorrectly classified as uncertain.");
}

static void UnstableFramesPause()
{
    var options = Options(stableFrames: 2, recognitions: 1) with
    {
        MaximumUnstableTransitions = 3,
    };
    var machine = new GuardedPolicyStateMachine(options);
    var now = DateTimeOffset.UnixEpoch;
    _ = machine.ObserveFrame(10, now);
    _ = machine.ObserveFrame(11, now.AddMilliseconds(1));
    _ = machine.ObserveFrame(12, now.AddMilliseconds(2));
    var result = machine.ObserveFrame(13, now.AddMilliseconds(3));
    Assert(result.Decision?.UncertaintyReason == GuardedUncertaintyReason.FrameUnstable,
        "Repeated semantic changes did not become an explainable unstable pause.");
}

static void ProgressiveRescanIsBounded()
{
    var options = Options(stableFrames: 1, recognitions: 1) with
    {
        MaximumRecognitionRescans = 2,
    };
    var machine = new GuardedPolicyStateMachine(options);
    _ = machine.ObserveFrame(10, DateTimeOffset.UnixEpoch);
    var first = machine.ObserveRecognition(Evidence(false, "none", rescan: true));
    Assert(first.Decision is null, "One progressive slice paused too early.");
    var second = machine.ObserveRecognition(Evidence(false, "none", rescan: true));
    Assert(second.Decision?.UncertaintyReason == GuardedUncertaintyReason.FrameUnstable,
        "The progressive rescan cap did not pause.");
}

static void ManualContinueReleases()
{
    var (machine, now) = Machine(stableFrames: 1, recognitions: 1);
    _ = machine.ObserveFrame(10, now);
    _ = machine.ObserveRecognition(Evidence(false, "crit:?", true, missing: true));
    var continued = machine.ContinueAfterUncertain();
    Assert(continued.AwaitNextCausativeClick && !continued.ReleaseInputGuard &&
           !machine.IsInputGuardHeld,
        "Continue did not request the safe one-click Guarded pass cycle.");
    Assert(machine.ObserveFrame(10, now.AddMilliseconds(1)).State ==
           GuardedSessionState.WaitingForChange,
        "Continue did not accept the manually reviewed fingerprint baseline.");
}

static void ManualStopReleases()
{
    var (machine, now) = Machine(stableFrames: 2, recognitions: 1);
    _ = machine.ObserveFrame(10, now);
    var stop = machine.Stop();
    Assert(stop.ReleaseInputGuard && machine.State == GuardedSessionState.Stopped &&
           !machine.IsInputGuardHeld,
        "Stop left Guarded input held.");
}

static void WatchdogFaultsOpen()
{
    var options = Options(2, 1) with { DecisionTimeout = TimeSpan.FromMilliseconds(100) };
    var machine = new GuardedPolicyStateMachine(options);
    var now = DateTimeOffset.UnixEpoch;
    _ = machine.ObserveFrame(10, now);
    var result = machine.ObserveFrame(10, now.AddMilliseconds(100));
    Assert(result.Decision?.Kind == GuardedDecisionKind.Faulted &&
           result.Decision.FaultReason == GuardedFaultReason.DecisionTimedOut &&
           result.ReleaseInputGuard && !machine.IsInputGuardHeld,
        "Watchdog timeout did not stop and fail open.");
}

static void ExceptionFaultReleases()
{
    var (machine, now) = Machine(2, 1);
    _ = machine.ObserveFrame(10, now);
    var result = machine.Fault("capture failed");
    Assert(result.Decision?.FaultReason == GuardedFaultReason.CaptureOrRecognitionFailed &&
           result.ReleaseInputGuard && machine.State == GuardedSessionState.Faulted,
        "Capture/OCR fault did not stop and release.");
}

static async Task MatchHandoffFailureFaultsAndCancelsAsync()
{
    using var capture = new GuardedMatchCapture();
    using var recognizer = new GuardedMatchRecognizer();
    await using var monitor = new AffixMonitor(capture, recognizer);
    var matchObserved = new TaskCompletionSource(
        TaskCreationOptions.RunContinuationsAsynchronously);
    var decisions = new List<GuardedDecisionEventArgs>();
    monitor.GuardedDecisionChanged += (_, decision) =>
    {
        lock (decisions)
        {
            decisions.Add(decision);
        }

        if (decision.Decision == GuardedDecisionKind.Match)
        {
            matchObserved.TrySetResult();
        }
    };

    var rules = RuleCompiler.Compile(new RuleSetDefinition(
        "guarded handoff failure",
        [new AcceptableResultGroup(
            "target",
            ResultGroupMode.Any,
            [new AffixCondition("life", "+# to maximum Life")])]));
    monitor.Start(
        rules,
        new ScreenRegion(0, 0, 1, 1),
        new GuardedMonitoringPolicy(new GuardedPolicyOptions
        {
            RequiredStableFrames = 1,
            RequiredConsistentRecognitions = 1,
            DecisionTimeout = TimeSpan.FromMilliseconds(500),
        }));

    await matchObserved.Task.WaitAsync(TimeSpan.FromSeconds(2));
    Assert(monitor.State == MonitorState.MatchFound &&
           monitor.GuardedState == GuardedSessionState.MatchFound,
        "The integration fixture did not reach MatchFound before hand-off failure.");

    var faulted = monitor.FailGuardedSession(
        "red overlay hand-off timed out",
        GuardedFaultReason.InputGuardUnavailable);
    Assert(faulted,
        "FailGuardedSession rejected the terminal red hand-off failure after MatchFound.");

    var cancellationField = typeof(AffixMonitor).GetField(
        "_loopCancellation",
        BindingFlags.Instance | BindingFlags.NonPublic)
        ?? throw new InvalidOperationException("AffixMonitor cancellation field is missing.");
    var cancellation = cancellationField.GetValue(monitor) as CancellationTokenSource
        ?? throw new InvalidOperationException("AffixMonitor cancellation source is missing.");
    Assert(cancellation.IsCancellationRequested,
        "A MatchFound hand-off failure did not cancel the Guarded monitor lifetime.");
    Assert(monitor.State == MonitorState.Faulted &&
           monitor.GuardedState == GuardedSessionState.Faulted,
        "A MatchFound hand-off failure did not transition both monitor layers to Faulted.");

    GuardedDecisionEventArgs[] observed;
    lock (decisions)
    {
        observed = [.. decisions];
    }

    Assert(observed.Count(decision => decision.Decision == GuardedDecisionKind.Match) == 1,
        "The Guarded monitor did not publish exactly one Match decision.");
    var terminal = observed.LastOrDefault();
    Assert(terminal?.Decision == GuardedDecisionKind.Faulted &&
           terminal.State == GuardedSessionState.Faulted &&
           terminal.FaultReason == GuardedFaultReason.InputGuardUnavailable &&
           terminal.Detail.Contains("hand-off timed out", StringComparison.Ordinal),
        "The post-match protection failure did not publish an explainable terminal Faulted decision.");
}

static void InputPairsDrainWithoutReplay()
{
    var buttons = new[]
    {
        PoeAlarm.App.Alerts.MouseButtonBits.Left,
        PoeAlarm.App.Alerts.MouseButtonBits.Right,
        PoeAlarm.App.Alerts.MouseButtonBits.Middle,
        PoeAlarm.App.Alerts.MouseButtonBits.X1,
        PoeAlarm.App.Alerts.MouseButtonBits.X2,
    };
    foreach (var button in buttons)
    {
        var state = new PoeAlarm.App.Alerts.MouseInputGuardStateMachine();
        state.InitializeReleased(0);
        state.Arm();
        var down = state.ProcessButton(button, isDown: true);
        Assert(down.Suppress && state.SuppressedButtons == button,
            $"Guard did not consume button-down bit {button}.");
        Assert(!state.Release(),
            $"Release did not retain button-up drain for bit {button}.");
        var up = state.ProcessButton(button, isDown: false);
        Assert(up.Suppress && up.BecameReleased,
            $"The paired button-up bit {button} was leaked or not drained.");
        Assert(state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.Released,
            $"Input bit {button} did not return to pass-through after its pair drained.");
    }
}

static void ExistingClickCompletesBeforeBlocking()
{
    var state = new PoeAlarm.App.Alerts.MouseInputGuardStateMachine();
    state.InitializeReleased(PoeAlarm.App.Alerts.MouseButtonBits.Left);
    state.Arm();
    Assert(state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.WaitingForExistingRelease,
        "Guard blocked the click that caused the changed frame.");
    var currentUp = state.ProcessButton(PoeAlarm.App.Alerts.MouseButtonBits.Left, isDown: false);
    Assert(!currentUp.Suppress && state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.Guarding,
        "The causative click did not complete before Guarded began blocking.");
}

static void OneCausativeClickPassesThenGuards()
{
    foreach (var button in new[]
             {
                 PoeAlarm.App.Alerts.MouseButtonBits.Left,
                 PoeAlarm.App.Alerts.MouseButtonBits.Right,
                 PoeAlarm.App.Alerts.MouseButtonBits.Middle,
                 PoeAlarm.App.Alerts.MouseButtonBits.X1,
                 PoeAlarm.App.Alerts.MouseButtonBits.X2,
             })
    {
        var state = new PoeAlarm.App.Alerts.MouseInputGuardStateMachine();
        state.InitializeReleased(0);
        state.Arm();
        state.AwaitNextCausativeClick();
        Assert(state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.AwaitingCausativeClick,
            $"Button bit {button} did not enter the one-shot pass-through state.");

        var allowedDown = state.ProcessButton(button, isDown: true);
        Assert(!allowedDown.Suppress &&
               state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.WaitingForExistingRelease,
            $"The one allowed Down for bit {button} was suppressed.");
        var allowedUp = state.ProcessButton(button, isDown: false);
        Assert(!allowedUp.Suppress && allowedUp.CausativeClickCompleted &&
               state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.Guarding,
            $"The one allowed Up for bit {button} did not atomically re-arm Guarding.");

        var nextDown = state.ProcessButton(button, isDown: true);
        Assert(nextDown.Suppress && state.SuppressedButtons == button,
            $"A second click for bit {button} slipped through the re-armed Guarded gate.");
        _ = state.ProcessButton(button, isDown: false);
    }
}

static void MidCausativeMissAwaitsFreshClick()
{
    var state = new PoeAlarm.App.Alerts.MouseInputGuardStateMachine();
    state.InitializeReleased(0);
    state.Arm();
    state.AwaitNextCausativeClick();

    var inFlightDown = state.ProcessButton(PoeAlarm.App.Alerts.MouseButtonBits.Left, isDown: true);
    Assert(!inFlightDown.Suppress &&
           state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.WaitingForExistingRelease,
        "The in-flight causative Down was not allowed through.");

    // A conclusive miss can race the Up of the click whose frame it evaluated. That unfinished
    // click must drain, but it cannot also consume the pass cycle for the next decision.
    state.AwaitNextCausativeClick();
    Assert(state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.WaitingForExistingReleaseThenAwait,
        "A miss during the causative click did not reserve a fresh pass cycle.");
    var inFlightUp = state.ProcessButton(PoeAlarm.App.Alerts.MouseButtonBits.Left, isDown: false);
    Assert(!inFlightUp.Suppress && !inFlightUp.CausativeClickCompleted &&
           state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.AwaitingCausativeClick,
        "The old click either leaked a completion or skipped the fresh await state.");

    var freshDown = state.ProcessButton(PoeAlarm.App.Alerts.MouseButtonBits.Left, isDown: true);
    var freshUp = state.ProcessButton(PoeAlarm.App.Alerts.MouseButtonBits.Left, isDown: false);
    Assert(!freshDown.Suppress && !freshUp.Suppress && freshUp.CausativeClickCompleted &&
           state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.Guarding,
        "The fresh causative cycle did not pass exactly once and re-arm Guarding.");
}

static void DrainingToAwaitHandlesOverlap()
{
    var state = new PoeAlarm.App.Alerts.MouseInputGuardStateMachine();
    state.InitializeReleased(0);
    state.Arm();

    var leftDown = state.ProcessButton(PoeAlarm.App.Alerts.MouseButtonBits.Left, isDown: true);
    Assert(leftDown.Suppress, "Guarding did not suppress the first Down before await.");
    state.AwaitNextCausativeClick();
    Assert(state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.DrainingToAwait,
        "Await did not retain the already-suppressed pair for draining.");

    var rightDown = state.ProcessButton(PoeAlarm.App.Alerts.MouseButtonBits.Right, isDown: true);
    Assert(rightDown.Suppress &&
           state.SuppressedButtons ==
               (PoeAlarm.App.Alerts.MouseButtonBits.Left |
                PoeAlarm.App.Alerts.MouseButtonBits.Right),
        "An overlapping Down escaped or was not added to the drain set.");
    var leftUp = state.ProcessButton(PoeAlarm.App.Alerts.MouseButtonBits.Left, isDown: false);
    Assert(leftUp.Suppress &&
           state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.DrainingToAwait &&
           state.PhysicalButtons == PoeAlarm.App.Alerts.MouseButtonBits.Right &&
           state.SuppressedButtons == PoeAlarm.App.Alerts.MouseButtonBits.Right,
        "Draining entered Awaiting while an overlapping physical pair remained.");
    var rightUp = state.ProcessButton(PoeAlarm.App.Alerts.MouseButtonBits.Right, isDown: false);
    Assert(rightUp.Suppress && !rightUp.CausativeClickCompleted &&
           state.PhysicalButtons == 0 && state.SuppressedButtons == 0 &&
           state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.AwaitingCausativeClick,
        "The final suppressed pair did not drain cleanly into Awaiting.");

    var allowedDown = state.ProcessButton(PoeAlarm.App.Alerts.MouseButtonBits.Left, isDown: true);
    Assert(!allowedDown.Suppress,
        "The first fresh Down remained blocked after every overlapping pair drained.");
}

static void InitialHeldInputDoesNotCompleteCycle()
{
    var initiallyHeld = PoeAlarm.App.Alerts.MouseButtonBits.Left;

    var armingState = new PoeAlarm.App.Alerts.MouseInputGuardStateMachine();
    armingState.InitializeReleased(initiallyHeld);
    armingState.Arm();
    var startupUp = armingState.ProcessButton(initiallyHeld, isDown: false);
    Assert(!startupUp.Suppress && !startupUp.CausativeClickCompleted &&
           armingState.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.Guarding,
        "An initially held button was misreported as a completed causative cycle while arming.");

    var awaitingState = new PoeAlarm.App.Alerts.MouseInputGuardStateMachine();
    awaitingState.InitializeReleased(initiallyHeld);
    awaitingState.AwaitNextCausativeClick();
    Assert(awaitingState.Mode ==
           PoeAlarm.App.Alerts.MouseInputGuardMode.WaitingForExistingReleaseThenAwait,
        "Await treated the initially held button as the new causative cycle.");
    var existingUp = awaitingState.ProcessButton(initiallyHeld, isDown: false);
    Assert(!existingUp.Suppress && !existingUp.CausativeClickCompleted &&
           awaitingState.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.AwaitingCausativeClick,
        "The initial button release emitted a false completion instead of awaiting a fresh click.");
}

static (GuardedPolicyStateMachine Machine, DateTimeOffset Now) Machine(
    int stableFrames,
    int recognitions) =>
    (new GuardedPolicyStateMachine(Options(stableFrames, recognitions)),
        DateTimeOffset.UnixEpoch);

static GuardedPolicyOptions Options(int stableFrames, int recognitions) => new()
{
    RequiredStableFrames = stableFrames,
    RequiredConsistentRecognitions = recognitions,
    MaximumUnstableTransitions = 8,
    MaximumRecognitionRescans = 8,
    DecisionTimeout = TimeSpan.FromMilliseconds(500),
};

static GuardedRuleEvidence Evidence(
    bool match,
    string signature,
    bool candidate = false,
    bool missing = false,
    bool conflict = false,
    bool rescan = false) =>
    new(match, rescan, candidate, false, missing, conflict, signature);

static void Assert(bool condition, string message)
{
    if (!condition)
    {
        throw new InvalidOperationException(message);
    }
}

sealed class GuardedMatchCapture : IScreenCapture
{
    private static readonly CapturedFrame Frame = new(
        1,
        1,
        4,
        [0, 0, 0, 255],
        DateTimeOffset.UnixEpoch);

    public CapturedFrame Capture(ScreenRegion region) => Frame;

    public void Dispose()
    {
    }
}

sealed class GuardedMatchRecognizer : IOcrRecognizer, IFrameFingerprintProvider
{
    public StructuredRuleOcrSupport StructuredRuleSupport =>
        StructuredRuleOcrSupport.StrictBatch;

    public ulong ComputeFrameFingerprint(CapturedFrame frame) => 0xA11CEUL;

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default) =>
        throw new InvalidOperationException("Guarded fixture expected batch recognition.");

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        IReadOnlyList<FullLineAffixMatcher> targets,
        CancellationToken cancellationToken = default) =>
        Task.FromResult(new OcrRecognitionResult(
            ["+70 to maximum Life"],
            TimeSpan.Zero,
            TimeSpan.Zero));

    public void Dispose()
    {
    }
}

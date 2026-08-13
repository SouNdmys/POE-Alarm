using System.Reflection;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Monitoring;
using PoeAlarm.App.Monitoring.Policies;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;
using PoeAlarm.Core.Rules;

var tests = new (string Name, Action Run)[]
{
    ("startup establishes baseline without guard or OCR", StartupEstablishesBaseline),
    ("idle visual changes update baseline until causative click", IdleChangesWaitForClick),
    ("causative down freezes baseline before button-up", CausativeDownFreezesBaseline),
    ("manual continue ignores overlay changes until causative click", ManualContinueIgnoresOverlayChange),
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
    ("causative click during capture cannot replace the baseline",
        () => CausativeClickDuringCaptureIsNotAbsorbedAsync().GetAwaiter().GetResult()),
    ("complete click at Continue handoff survives paused state",
        () => PausedWindowClickSurvivesContinueAsync().GetAwaiter().GetResult()),
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
    var first = machine.ObserveFrame(10, now.AddMilliseconds(3));
    Assert(first.RequestInputGuard, "Changed frame did not synchronously request the guard.");
    Assert(!first.ShouldRecognize && machine.State == GuardedSessionState.WaitingForStableFrame,
        "OCR ran before the semantic frame was stable.");

    var second = machine.ObserveFrame(10, now.AddMilliseconds(4));
    Assert(second.RequestInputGuard && second.ShouldRecognize,
        "Second identical frame did not refresh the guard and start OCR.");
}

static void StartupEstablishesBaseline()
{
    var machine = new GuardedPolicyStateMachine(Options(2, 1));
    var now = DateTimeOffset.UnixEpoch;
    var first = machine.ObserveFrame(10, now);
    Assert(machine.State == GuardedSessionState.EstablishingBaseline &&
           !first.RequestInputGuard && !first.ShouldRecognize && first.Decision is null,
        "First startup frame armed or evaluated Guarded instead of sampling a baseline.");
    var second = machine.ObserveFrame(10, now.AddMilliseconds(1));
    Assert(machine.State == GuardedSessionState.WaitingForChange &&
           second.AwaitNextCausativeClick && !second.RequestInputGuard &&
           !second.ShouldRecognize && second.Decision is null,
        "Stable startup baseline did not enter the unbounded one-click wait.");
}

static void IdleChangesWaitForClick()
{
    var machine = new GuardedPolicyStateMachine(Options(1, 1));
    var now = DateTimeOffset.UnixEpoch;
    EstablishBaseline(machine, now, 10);
    var overlayGone = machine.ObserveFrame(20, now.AddSeconds(30));
    var hoverSettled = machine.ObserveFrame(30, now.AddMinutes(5));
    Assert(machine.State == GuardedSessionState.WaitingForChange &&
           !machine.IsInputGuardHeld && overlayGone.Decision is null &&
           hoverSettled.Decision is null && !overlayGone.RequestInputGuard,
        "A pre-click visual change started a decision or timed out.");
    Assert(machine.NotifyCausativeClickCompleted(now.AddMinutes(5).AddMilliseconds(1)),
        "Native click completion was not accepted.");
    var unchanged = machine.ObserveFrame(30, now.AddMinutes(5).AddMilliseconds(2));
    Assert(!unchanged.RequestInputGuard && !unchanged.ShouldRecognize,
        "The current pre-click baseline was incorrectly classified as a crafted change.");
    var crafted = machine.ObserveFrame(31, now.AddMinutes(5).AddMilliseconds(3));
    Assert(crafted.RequestInputGuard,
        "A post-click fingerprint change did not start the guarded decision.");
}

static void ManualContinueIgnoresOverlayChange()
{
    var machine = new GuardedPolicyStateMachine(Options(1, 1));
    var now = DateTimeOffset.UnixEpoch;
    EstablishBaseline(machine, now, 10);
    Assert(machine.NotifyCausativeClickCompleted(now.AddMilliseconds(1)),
        "Initial causative click was not accepted.");
    _ = machine.ObserveFrame(11, now.AddMilliseconds(2));
    _ = machine.ObserveRecognition(Evidence(false, "crit:?", true, missing: true));
    var continued = machine.ContinueAfterUncertain();
    Assert(continued.AwaitNextCausativeClick,
        "Continue did not return to one-click wait.");
    var overlayRemoved = machine.ObserveFrame(99, now.AddSeconds(2));
    Assert(overlayRemoved.Decision is null && !overlayRemoved.RequestInputGuard &&
           machine.State == GuardedSessionState.WaitingForChange,
        "The yellow overlay disappearing immediately retriggered Guarded.");
    Assert(machine.NotifyCausativeClickCompleted(now.AddSeconds(10)),
        "Continue's next causative click was not accepted after an unbounded wait.");
    var crafted = machine.ObserveFrame(100, now.AddSeconds(10).AddMilliseconds(1));
    Assert(crafted.RequestInputGuard,
        "The first real post-Continue crafting change was not guarded.");
}

static void CausativeDownFreezesBaseline()
{
    var machine = new GuardedPolicyStateMachine(Options(1, 1));
    var now = DateTimeOffset.UnixEpoch;
    EstablishBaseline(machine, now, 10);
    Assert(machine.NotifyCausativeClickStarted(), "Causative Down was not accepted.");
    var changedBeforeUp = machine.ObserveFrame(11, now.AddMilliseconds(1));
    Assert(!changedBeforeUp.RequestInputGuard && machine.State == GuardedSessionState.WaitingForChange,
        "Frame capture before Up should remain pass-through while retaining the old baseline.");
    Assert(machine.NotifyCausativeClickCompleted(now.AddMilliseconds(2)),
        "Causative Up was not accepted.");
    var afterUp = machine.ObserveFrame(11, now.AddMilliseconds(3));
    Assert(afterUp.RequestInputGuard && afterUp.ShouldRecognize,
        "The roll visible between Down and Up was absorbed into the baseline and missed.");
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
    EstablishBaseline(machine, now, 9, stableFrames: 2);
    Assert(machine.NotifyCausativeClickCompleted(now.AddTicks(1)),
        "Unstable-frame fixture did not enter a causative decision.");
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
    EstablishBaseline(machine, DateTimeOffset.UnixEpoch, 9);
    Assert(machine.NotifyCausativeClickCompleted(DateTimeOffset.UnixEpoch.AddTicks(1)),
        "Progressive fixture did not enter a causative decision.");
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
    EstablishBaseline(machine, now, 9, stableFrames: 2);
    Assert(machine.NotifyCausativeClickCompleted(now.AddTicks(1)),
        "Watchdog fixture did not enter a causative decision.");
    _ = machine.ObserveFrame(10, now.AddTicks(2));
    var result = machine.ObserveFrame(10, now.AddMilliseconds(101));
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
    monitor.GuardedPassCycleRequested += (_, _) =>
    {
        recognizer.AdvanceFingerprint();
        Assert(monitor.NotifyGuardedCausativeClickCompleted(),
            "Integration fixture did not deliver its simulated causative click.");
    };
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

static async Task CausativeClickDuringCaptureIsNotAbsorbedAsync()
{
    using var capture = new GuardedInjectingCapture();
    using var recognizer = new GuardedMatchRecognizer();
    await using var monitor = new AffixMonitor(capture, recognizer);
    var detected = new TaskCompletionSource(
        TaskCreationOptions.RunContinuationsAsynchronously);
    monitor.AffixDetected += (_, _) => detected.TrySetResult();
    monitor.GuardedPassCycleRequested += (_, _) =>
    {
        capture.InjectOnNextCapture(() =>
        {
            recognizer.AdvanceFingerprint();
            Assert(monitor.NotifyGuardedCausativeClickStarted(),
                "Causative Down delivered during Capture was rejected.");
            Assert(monitor.NotifyGuardedCausativeClickCompleted(),
                "Causative Up delivered during Capture was rejected.");
        });
    };

    monitor.Start(
        GuardedLifeRules(),
        new ScreenRegion(0, 0, 1, 1),
        new GuardedMonitoringPolicy(new GuardedPolicyOptions
        {
            RequiredStableFrames = 1,
            RequiredConsistentRecognitions = 1,
            DecisionTimeout = TimeSpan.FromMilliseconds(500),
        }));

    await detected.Task.WaitAsync(TimeSpan.FromSeconds(2));
    Assert(monitor.State == MonitorState.MatchFound,
        "A click completed during Capture was absorbed into the idle baseline.");
}

static async Task PausedWindowClickSurvivesContinueAsync()
{
    using var capture = new GuardedMatchCapture();
    using var recognizer = new GuardedPauseThenMatchRecognizer();
    await using var monitor = new AffixMonitor(capture, recognizer);
    var paused = new TaskCompletionSource(
        TaskCreationOptions.RunContinuationsAsynchronously);
    var detected = new TaskCompletionSource(
        TaskCreationOptions.RunContinuationsAsynchronously);
    var passCycleCount = 0;
    monitor.GuardedDecisionChanged += (_, decision) =>
    {
        if (decision.Decision == GuardedDecisionKind.Uncertain)
        {
            paused.TrySetResult();
        }
    };
    monitor.AffixDetected += (_, _) => detected.TrySetResult();
    monitor.GuardedPassCycleRequested += (_, _) =>
    {
        if (Interlocked.Increment(ref passCycleCount) != 1)
        {
            return;
        }

        recognizer.AdvanceFingerprint();
        Assert(monitor.NotifyGuardedCausativeClickStarted(),
            "Initial causative Down was rejected.");
        Assert(monitor.NotifyGuardedCausativeClickCompleted(),
            "Initial causative Up was rejected.");
    };

    monitor.Start(
        GuardedLifeRules(),
        new ScreenRegion(0, 0, 1, 1),
        new GuardedMonitoringPolicy(new GuardedPolicyOptions
        {
            RequiredStableFrames = 1,
            RequiredConsistentRecognitions = 1,
            DecisionTimeout = TimeSpan.FromMilliseconds(500),
        }));

    await paused.Task.WaitAsync(TimeSpan.FromSeconds(2));
    Assert(monitor.GuardedState == GuardedSessionState.PausedUncertain,
        "Fixture did not enter the yellow review state.");

    // Reproduce the old overlay-release window: the native one-shot click completes while the
    // application callback has not yet moved the monitor out of PausedUncertain.
    Assert(monitor.NotifyGuardedCausativeClickStarted(),
        "A fast Down at the Continue handoff was dropped while paused.");
    recognizer.AdvanceFingerprintAndMatch();
    Assert(monitor.NotifyGuardedCausativeClickCompleted(),
        "A fast Up at the Continue handoff was dropped while paused.");
    Assert(monitor.ContinueGuarded(),
        "Continue did not resume after the handoff click was buffered.");

    await detected.Task.WaitAsync(TimeSpan.FromSeconds(2));
    Assert(monitor.State == MonitorState.MatchFound && passCycleCount == 2,
        "The buffered Continue-handoff click did not drive the next guarded decision.");
}

static CompiledRuleSet GuardedLifeRules() => RuleCompiler.Compile(new RuleSetDefinition(
    "guarded life",
    [new AcceptableResultGroup(
        "target",
        ResultGroupMode.Any,
        [new AffixCondition("life", "+# to maximum Life")])]));

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
        Assert(!allowedDown.Suppress && allowedDown.CausativeClickStarted &&
               state.Mode == PoeAlarm.App.Alerts.MouseInputGuardMode.WaitingForExistingRelease,
            $"The one allowed Down for bit {button} was suppressed or not reported.");
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
    int recognitions)
{
    var machine = new GuardedPolicyStateMachine(Options(stableFrames, recognitions));
    var now = DateTimeOffset.UnixEpoch;
    EstablishBaseline(machine, now, fingerprint: 9, stableFrames);
    Assert(machine.NotifyCausativeClickCompleted(now.AddTicks(1)),
        "Fixture did not enter a causative decision after establishing its baseline.");
    return (machine, now);
}

static void EstablishBaseline(
    GuardedPolicyStateMachine machine,
    DateTimeOffset now,
    ulong fingerprint,
    int stableFrames = 1)
{
    GuardedFrameTransition transition = default;
    for (var index = 0; index < stableFrames; index++)
    {
        transition = machine.ObserveFrame(fingerprint, now.AddTicks(index));
    }

    Assert(machine.State == GuardedSessionState.WaitingForChange &&
           transition.AwaitNextCausativeClick,
        "Fixture could not establish the Guarded startup baseline.");
}

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

sealed class GuardedInjectingCapture : IScreenCapture
{
    private static readonly CapturedFrame Frame = new(
        1,
        1,
        4,
        [0, 0, 0, 255],
        DateTimeOffset.UnixEpoch);
    private Action? _nextCapture;

    public void InjectOnNextCapture(Action action) =>
        Interlocked.Exchange(ref _nextCapture, action);

    public CapturedFrame Capture(ScreenRegion region)
    {
        Interlocked.Exchange(ref _nextCapture, null)?.Invoke();
        return Frame;
    }

    public void Dispose()
    {
    }
}

sealed class GuardedMatchRecognizer : IOcrRecognizer, IFrameFingerprintProvider
{
    private long _fingerprint = 0xA11CE;

    public StructuredRuleOcrSupport StructuredRuleSupport =>
        StructuredRuleOcrSupport.StrictBatch;

    public ulong ComputeFrameFingerprint(CapturedFrame frame) =>
        unchecked((ulong)Interlocked.Read(ref _fingerprint));

    public void AdvanceFingerprint() => Interlocked.Increment(ref _fingerprint);

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

sealed class GuardedPauseThenMatchRecognizer : IOcrRecognizer, IFrameFingerprintProvider
{
    private long _fingerprint = 0xBEE;
    private int _returnMatch;

    public StructuredRuleOcrSupport StructuredRuleSupport =>
        StructuredRuleOcrSupport.StrictBatch;

    public ulong ComputeFrameFingerprint(CapturedFrame frame) =>
        unchecked((ulong)Interlocked.Read(ref _fingerprint));

    public void AdvanceFingerprint() => Interlocked.Increment(ref _fingerprint);

    public void AdvanceFingerprintAndMatch()
    {
        Interlocked.Exchange(ref _returnMatch, 1);
        AdvanceFingerprint();
    }

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default) =>
        throw new InvalidOperationException("Guarded fixture expected batch recognition.");

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        IReadOnlyList<FullLineAffixMatcher> targets,
        CancellationToken cancellationToken = default)
    {
        if (Volatile.Read(ref _returnMatch) != 0)
        {
            return Task.FromResult(new OcrRecognitionResult(
                ["+70 to maximum Life"],
                TimeSpan.Zero,
                TimeSpan.Zero));
        }

        return Task.FromResult(new OcrRecognitionResult(
            ["unrelated"],
            TimeSpan.Zero,
            TimeSpan.Zero,
            CandidateEvidence:
            [
                new OcrCandidateEvidence(
                    "life-row",
                    "+? to maximum Life",
                    targets[0].Template.Text,
                    OcrCandidateEvidenceKind.LocalizedLexicalCandidate,
                    0,
                    1),
            ]));
    }

    public void Dispose()
    {
    }
}

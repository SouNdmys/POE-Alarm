using System.ComponentModel;
using System.Runtime.InteropServices;

namespace PoeAlarm.App.Alerts;

/// <summary>
/// A short-lived, fail-open mouse gate used while Traditional Chinese OCR decides a changed
/// tooltip. The hook never injects, queues, or replays input. It only consumes button/wheel
/// messages after the physical click that produced the current frame has completely released.
/// </summary>
internal sealed class PendingMouseInputGuard : IDisposable
{
    private static readonly TimeSpan StartupTimeout = TimeSpan.FromMilliseconds(250);
    private static readonly TimeSpan DrainTimeout = TimeSpan.FromMilliseconds(750);
    private static readonly TimeSpan IdleUninstallDelay = TimeSpan.FromMilliseconds(1000);

    private const int WhMouseLl = 14;
    private const uint WmQuit = 0x0012;
    private const uint PmNoRemove = 0x0000;

    private const int VkLeftButton = 0x01;
    private const int VkRightButton = 0x02;
    private const int VkMiddleButton = 0x04;
    private const int VkXButton1 = 0x05;
    private const int VkXButton2 = 0x06;

    private const int WmLButtonDown = 0x0201;
    private const int WmLButtonUp = 0x0202;
    private const int WmLButtonDoubleClick = 0x0203;
    private const int WmRButtonDown = 0x0204;
    private const int WmRButtonUp = 0x0205;
    private const int WmRButtonDoubleClick = 0x0206;
    private const int WmMButtonDown = 0x0207;
    private const int WmMButtonUp = 0x0208;
    private const int WmMButtonDoubleClick = 0x0209;
    private const int WmMouseWheel = 0x020A;
    private const int WmXButtonDown = 0x020B;
    private const int WmXButtonUp = 0x020C;
    private const int WmXButtonDoubleClick = 0x020D;
    private const int WmMouseHorizontalWheel = 0x020E;

    private const int XButton1 = 0x0001;
    private const int XButton2 = 0x0002;

    private readonly object _lifecycleGate = new();
    private readonly MouseInputGuardStateMachine _state = new();
    private readonly LowLevelMouseProcedure _hookProcedure;
    private Thread? _hookThread;
    private IntPtr _hookHandle;
    private uint _hookThreadId;
    private int _lifecycleGeneration;
    private int _stopAfterDrain;
    private int _stopRequested;
    private bool _disposed;

    public PendingMouseInputGuard()
    {
        // Keep the delegate rooted for the complete native hook lifetime.
        _hookProcedure = HookProcedure;
    }

    /// <summary>
    /// Raised on the low-level hook thread after the one deliberately pass-through causative
    /// click has delivered its final button-up and the gate has atomically returned to Guarding.
    /// Handlers must return promptly. One failing handler is isolated from every other handler
    /// and from the native hook callback.
    /// </summary>
    public event EventHandler? CausativeClickCompleted;

    internal bool IsInstalled => Volatile.Read(ref _hookHandle) != IntPtr.Zero;

    internal MouseInputGuardMode Mode => _state.Mode;

    /// <summary>
    /// Pre-installs the pass-through hook while the user is still starting monitoring. In the
    /// released state it only tracks button pairing; it never consumes an input message.
    /// </summary>
    public bool Prepare()
    {
        lock (_lifecycleGate)
        {
            return !_disposed && EnsureInstalledLocked();
        }
    }

    /// <summary>
    /// Arms synchronously on the caller thread. Because a prepared hook continuously tracks the
    /// physical button mask, this transition does not depend on WPF dispatcher timing.
    /// </summary>
    public bool Arm()
    {
        lock (_lifecycleGate)
        {
            if (_disposed || !EnsureInstalledLocked())
            {
                return false;
            }

            _lifecycleGeneration++;
            Volatile.Write(ref _stopAfterDrain, 0);
            _state.Arm();
            return true;
        }
    }

    /// <summary>
    /// Keeps the hook installed while allowing exactly one future physical button cycle through
    /// to the game. The final button-up of that cycle atomically returns the hook to Guarding, so
    /// a second click cannot slip through while capture notices the first click's changed frame.
    /// A button already held when this method is called is drained as pre-existing input and does
    /// not count as the allowed causative click.
    /// </summary>
    public bool AwaitNextCausativeClick()
    {
        lock (_lifecycleGate)
        {
            if (_disposed || !EnsureInstalledLocked())
            {
                return false;
            }

            // Invalidate a pending idle uninstall or drain fail-open. Awaiting a user click is an
            // intentional, unbounded pass-through phase; the upper Guarded owner starts its
            // decision watchdog only after CausativeClickCompleted.
            _lifecycleGeneration++;
            Volatile.Write(ref _stopAfterDrain, 0);
            _state.AwaitNextCausativeClick();
            return true;
        }
    }

    /// <summary>
    /// Stops accepting new clicks immediately. If a button-down was already consumed, its paired
    /// button-up is consumed too; the hook then returns to pass-through and uninstalls after a
    /// short idle grace period unless another changed frame re-arms it.
    /// </summary>
    public void Release()
    {
        int generation;
        bool released;
        lock (_lifecycleGate)
        {
            if (_disposed)
            {
                return;
            }

            generation = ++_lifecycleGeneration;
            Volatile.Write(ref _stopAfterDrain, 1);
            released = _state.Release();
        }

        if (released)
        {
            ScheduleIdleUninstall(generation);
        }
        else
        {
            ScheduleDrainFailOpen(generation);
        }
    }

    /// <summary>
    /// Called only after the native red overlay is already visible and blocking. The overlay can
    /// safely receive the tail of a consumed click, so the low-level hook may fail open at once.
    /// </summary>
    public void TransferToBlockingOverlay() => ForceReleaseAndStop();

    public void Dispose()
    {
        Thread? thread;
        lock (_lifecycleGate)
        {
            if (_disposed)
            {
                return;
            }

            _disposed = true;
            _lifecycleGeneration++;
            Volatile.Write(ref _stopAfterDrain, 0);
            _state.ForceRelease();
            RequestHookThreadStop();
            thread = _hookThread;
        }

        if (thread is not null && thread != Thread.CurrentThread)
        {
            _ = thread.Join(millisecondsTimeout: 500);
        }

        GC.SuppressFinalize(this);
    }

    private bool EnsureInstalledLocked()
    {
        if (_hookThread is { IsAlive: true } existingThread)
        {
            if (IsInstalled && Volatile.Read(ref _stopRequested) == 0)
            {
                return true;
            }

            // Acknowledge followed immediately by Start can reach here before the hook thread has
            // consumed its queued WM_QUIT. Wait for that fail-open transition before creating the
            // replacement; otherwise the new Arm could target a hook already committed to exit.
            if (!existingThread.Join(StartupTimeout))
            {
                return false;
            }
        }

        Volatile.Write(ref _stopRequested, 0);
        // Do not dispose this signal from the caller: a late native startup failure can still be
        // unwinding on the hook thread after the bounded wait returns. It becomes ordinary
        // collectible managed state as soon as that short-lived thread exits.
        var ready = new ManualResetEventSlim(initialState: false);
        Exception? startupError = null;
        var thread = new Thread(() =>
        {
            try
            {
                HookThreadMain(ready);
            }
            catch (Exception exception)
            {
                startupError = exception;
                ready.Set();
            }
        })
        {
            IsBackground = true,
            Name = "POE Alarm pending mouse guard",
        };

        _hookThread = thread;
        thread.Start();
        if (!ready.Wait(StartupTimeout))
        {
            _state.ForceRelease();
            RequestHookThreadStop();
            return false;
        }

        if (startupError is not null || !IsInstalled)
        {
            _state.ForceRelease();
            return false;
        }

        return true;
    }

    private void HookThreadMain(ManualResetEventSlim ready)
    {
        // Force creation of this thread's Win32 message queue before another thread can post
        // WM_QUIT during a stop/start race.
        _ = PeekMessage(out _, IntPtr.Zero, 0, 0, PmNoRemove);
        Volatile.Write(ref _hookThreadId, GetCurrentThreadId());

        var initialButtons = ReadPhysicalButtonMask();
        _state.InitializeReleased(initialButtons);

        var hook = SetWindowsHookEx(WhMouseLl, _hookProcedure, GetModuleHandle(null), 0);
        if (hook == IntPtr.Zero)
        {
            var error = Marshal.GetLastWin32Error();
            throw new Win32Exception(error, "Could not install the pending mouse input guard.");
        }

        Volatile.Write(ref _hookHandle, hook);
        ready.Set();

        try
        {
            if (Volatile.Read(ref _stopRequested) != 0)
            {
                return;
            }

            while (GetMessage(out var message, IntPtr.Zero, 0, 0) > 0)
            {
                _ = TranslateMessage(ref message);
                _ = DispatchMessage(ref message);
            }
        }
        finally
        {
            _state.ForceRelease();
            Volatile.Write(ref _hookHandle, IntPtr.Zero);
            _ = UnhookWindowsHookEx(hook);
            Volatile.Write(ref _hookThreadId, 0);
        }
    }

    private IntPtr HookProcedure(int code, IntPtr wParam, IntPtr lParam)
    {
        if (code < 0)
        {
            return CallNextHookEx(IntPtr.Zero, code, wParam, lParam);
        }

        var message = unchecked((int)wParam.ToInt64());
        MouseInputGuardDecision decision;
        switch (message)
        {
            case WmLButtonDown:
            case WmLButtonDoubleClick:
                decision = _state.ProcessButton(MouseButtonBits.Left, isDown: true);
                break;
            case WmLButtonUp:
                decision = _state.ProcessButton(MouseButtonBits.Left, isDown: false);
                break;
            case WmRButtonDown:
            case WmRButtonDoubleClick:
                decision = _state.ProcessButton(MouseButtonBits.Right, isDown: true);
                break;
            case WmRButtonUp:
                decision = _state.ProcessButton(MouseButtonBits.Right, isDown: false);
                break;
            case WmMButtonDown:
            case WmMButtonDoubleClick:
                decision = _state.ProcessButton(MouseButtonBits.Middle, isDown: true);
                break;
            case WmMButtonUp:
                decision = _state.ProcessButton(MouseButtonBits.Middle, isDown: false);
                break;
            case WmXButtonDown:
            case WmXButtonDoubleClick:
            case WmXButtonUp:
                var mouseData = unchecked((uint)Marshal.ReadInt32(lParam, 8));
                var xButton = (mouseData >> 16) == XButton2
                    ? MouseButtonBits.X2
                    : MouseButtonBits.X1;
                decision = _state.ProcessButton(xButton, message != WmXButtonUp);
                break;
            case WmMouseWheel:
            case WmMouseHorizontalWheel:
                decision = new MouseInputGuardDecision(
                    _state.ShouldSuppressWheel,
                    BecameReleased: false,
                    CausativeClickCompleted: false);
                break;
            default:
                return CallNextHookEx(IntPtr.Zero, code, wParam, lParam);
        }

        if (decision.BecameReleased && Volatile.Read(ref _stopAfterDrain) != 0)
        {
            var generation = Volatile.Read(ref _lifecycleGeneration);
            ScheduleIdleUninstall(generation);
        }

        if (decision.Suppress)
        {
            return new IntPtr(1);
        }

        // Let the causative Up traverse the rest of the native hook chain before notifying the
        // Guarded owner. A slow subscriber therefore cannot delay the exact release we promised
        // to pass through. The packed state was already changed to Guarding by the successful CAS.
        var nextHookResult = CallNextHookEx(IntPtr.Zero, code, wParam, lParam);
        if (decision.CausativeClickCompleted)
        {
            RaiseCausativeClickCompleted();
        }

        return nextHookResult;
    }

    private void RaiseCausativeClickCompleted()
    {
        if (Volatile.Read(ref _stopAfterDrain) != 0 ||
            _state.Mode != MouseInputGuardMode.Guarding)
        {
            return;
        }

        var handlers = CausativeClickCompleted;
        if (handlers is null)
        {
            return;
        }

        foreach (EventHandler handler in handlers.GetInvocationList())
        {
            try
            {
                handler(this, EventArgs.Empty);
            }
            catch
            {
                // A subscriber must never unwind through WH_MOUSE_LL. The Guarded service has
                // its own fail-open watchdog for a notification it cannot act upon.
            }
        }
    }

    private void ForceReleaseAndStop()
    {
        lock (_lifecycleGate)
        {
            if (_disposed)
            {
                return;
            }

            _lifecycleGeneration++;
            Volatile.Write(ref _stopAfterDrain, 0);
            _state.ForceRelease();
            RequestHookThreadStop();
        }
    }

    private void ScheduleDrainFailOpen(int generation) => _ = Task.Run(async () =>
    {
        await Task.Delay(DrainTimeout).ConfigureAwait(false);
        lock (_lifecycleGate)
        {
            if (_disposed || generation != _lifecycleGeneration ||
                _state.Mode != MouseInputGuardMode.Draining)
            {
                return;
            }

            _state.ForceRelease();
            RequestHookThreadStop();
        }
    });

    private void ScheduleIdleUninstall(int generation) => _ = Task.Run(async () =>
    {
        await Task.Delay(IdleUninstallDelay).ConfigureAwait(false);
        lock (_lifecycleGate)
        {
            if (_disposed || generation != _lifecycleGeneration ||
                _state.Mode != MouseInputGuardMode.Released)
            {
                return;
            }

            RequestHookThreadStop();
        }
    });

    private void RequestHookThreadStop()
    {
        Volatile.Write(ref _stopRequested, 1);
        var threadId = Volatile.Read(ref _hookThreadId);
        if (threadId != 0)
        {
            _ = PostThreadMessage(threadId, WmQuit, UIntPtr.Zero, IntPtr.Zero);
        }
    }

    private static int ReadPhysicalButtonMask()
    {
        var mask = 0;
        AddIfPressed(VkLeftButton, MouseButtonBits.Left);
        AddIfPressed(VkRightButton, MouseButtonBits.Right);
        AddIfPressed(VkMiddleButton, MouseButtonBits.Middle);
        AddIfPressed(VkXButton1, MouseButtonBits.X1);
        AddIfPressed(VkXButton2, MouseButtonBits.X2);
        return mask;

        void AddIfPressed(int virtualKey, int bit)
        {
            if ((GetAsyncKeyState(virtualKey) & 0x8000) != 0)
            {
                mask |= bit;
            }
        }
    }

    private delegate IntPtr LowLevelMouseProcedure(int code, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeMessage
    {
        public IntPtr Window;
        public uint Message;
        public UIntPtr WParam;
        public IntPtr LParam;
        public uint Time;
        public NativePoint Point;
        public uint Private;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct NativePoint
    {
        public int X;
        public int Y;
    }

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr SetWindowsHookEx(
        int hookIdentifier,
        LowLevelMouseProcedure hookProcedure,
        IntPtr module,
        uint threadId);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool UnhookWindowsHookEx(IntPtr hook);

    [DllImport("user32.dll")]
    private static extern IntPtr CallNextHookEx(
        IntPtr hook,
        int code,
        IntPtr wParam,
        IntPtr lParam);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int GetMessage(
        out NativeMessage message,
        IntPtr window,
        uint minimumMessage,
        uint maximumMessage);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool TranslateMessage(ref NativeMessage message);

    [DllImport("user32.dll")]
    private static extern IntPtr DispatchMessage(ref NativeMessage message);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PeekMessage(
        out NativeMessage message,
        IntPtr window,
        uint minimumMessage,
        uint maximumMessage,
        uint removeMessage);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PostThreadMessage(
        uint threadId,
        uint message,
        UIntPtr wParam,
        IntPtr lParam);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr GetModuleHandle(string? moduleName);

    [DllImport("kernel32.dll")]
    private static extern uint GetCurrentThreadId();

    [DllImport("user32.dll")]
    private static extern short GetAsyncKeyState(int virtualKey);
}

internal enum MouseInputGuardMode
{
    Released,
    WaitingForExistingRelease,
    Guarding,
    Draining,
    AwaitingCausativeClick,
    WaitingForExistingReleaseThenAwait,
    DrainingToAwait,
}

internal static class MouseButtonBits
{
    public const int Left = 1 << 0;
    public const int Right = 1 << 1;
    public const int Middle = 1 << 2;
    public const int X1 = 1 << 3;
    public const int X2 = 1 << 4;
    public const int All = Left | Right | Middle | X1 | X2;
}

internal readonly record struct MouseInputGuardDecision(
    bool Suppress,
    bool BecameReleased,
    bool CausativeClickCompleted = false);

/// <summary>
/// Lock-free packed state used by the low-level callback. Keeping the transition logic free of
/// WPF, allocation, logging, and locks also makes button-pairing behavior deterministic to test.
/// </summary>
internal sealed class MouseInputGuardStateMachine
{
    private const int PhysicalShift = 0;
    private const int SuppressedShift = 8;
    private const int ModeShift = 16;
    private const int CausativeCycleShift = 24;
    private const int ButtonMask = MouseButtonBits.All;
    private const int ModeMask = 0x7;

    private int _packedState;

    public MouseInputGuardMode Mode => DecodeMode(Volatile.Read(ref _packedState));

    public int PhysicalButtons => DecodePhysical(Volatile.Read(ref _packedState));

    public int SuppressedButtons => DecodeSuppressed(Volatile.Read(ref _packedState));

    public bool ShouldSuppressWheel => SuppressesNewInput(Mode);

    public void InitializeReleased(int physicalButtons) =>
        Volatile.Write(ref _packedState, Pack(
            physicalButtons & ButtonMask,
            suppressed: 0,
            MouseInputGuardMode.Released,
            causativeCycle: false));

    public void Arm()
    {
        while (true)
        {
            var current = Volatile.Read(ref _packedState);
            var mode = DecodeMode(current);
            if (mode is MouseInputGuardMode.WaitingForExistingRelease or
                MouseInputGuardMode.Guarding)
            {
                return;
            }

            var physical = DecodePhysical(current);
            var suppressed = DecodeSuppressed(current);
            var nextMode = (physical & ~suppressed) != 0
                ? MouseInputGuardMode.WaitingForExistingRelease
                : MouseInputGuardMode.Guarding;
            var next = Pack(
                physical,
                suppressed,
                nextMode,
                causativeCycle: false);
            if (Interlocked.CompareExchange(ref _packedState, next, current) == current)
            {
                return;
            }
        }
    }

    public void AwaitNextCausativeClick()
    {
        while (true)
        {
            var current = Volatile.Read(ref _packedState);
            var mode = DecodeMode(current);
            var causativeCycle = DecodeCausativeCycle(current);

            // Repeated upper-layer transitions are deliberately idempotent once the next cycle
            // is already pending. If a completed decision races the currently allowed causative
            // chord, reclassify that chord as pre-existing: its remaining Up still passes, then a
            // fresh complete causative cycle is awaited for the next decision.
            if (mode is MouseInputGuardMode.AwaitingCausativeClick or
                MouseInputGuardMode.WaitingForExistingReleaseThenAwait or
                MouseInputGuardMode.DrainingToAwait)
            {
                return;
            }

            var physical = DecodePhysical(current);
            var suppressed = DecodeSuppressed(current);
            var nextMode = suppressed != 0
                ? MouseInputGuardMode.DrainingToAwait
                : physical != 0 ||
                  (mode == MouseInputGuardMode.WaitingForExistingRelease && causativeCycle)
                    ? MouseInputGuardMode.WaitingForExistingReleaseThenAwait
                    : MouseInputGuardMode.AwaitingCausativeClick;
            var next = Pack(
                physical,
                suppressed,
                nextMode,
                causativeCycle: false);
            if (Interlocked.CompareExchange(ref _packedState, next, current) == current)
            {
                return;
            }
        }
    }

    /// <returns>True when no previously suppressed button-up remains to be drained.</returns>
    public bool Release()
    {
        while (true)
        {
            var current = Volatile.Read(ref _packedState);
            var physical = DecodePhysical(current);
            var suppressed = DecodeSuppressed(current);
            var nextMode = suppressed == 0
                ? MouseInputGuardMode.Released
                : MouseInputGuardMode.Draining;
            var next = Pack(
                physical,
                suppressed,
                nextMode,
                causativeCycle: false);
            if (Interlocked.CompareExchange(ref _packedState, next, current) == current)
            {
                return nextMode == MouseInputGuardMode.Released;
            }
        }
    }

    public void ForceRelease()
    {
        while (true)
        {
            var current = Volatile.Read(ref _packedState);
            var next = Pack(
                DecodePhysical(current),
                suppressed: 0,
                MouseInputGuardMode.Released,
                causativeCycle: false);
            if (Interlocked.CompareExchange(ref _packedState, next, current) == current)
            {
                return;
            }
        }
    }

    public MouseInputGuardDecision ProcessButton(int button, bool isDown)
    {
        if ((button & ButtonMask) == 0 || (button & (button - 1)) != 0)
        {
            return default;
        }

        while (true)
        {
            var current = Volatile.Read(ref _packedState);
            var physical = DecodePhysical(current);
            var suppressed = DecodeSuppressed(current);
            var mode = DecodeMode(current);
            var causativeCycle = DecodeCausativeCycle(current);
            var suppress = false;
            var becameReleased = false;
            var causativeClickCompleted = false;

            if (isDown)
            {
                physical |= button;
                if (mode == MouseInputGuardMode.AwaitingCausativeClick)
                {
                    // This is the one cycle explicitly allowed through to POE. Every button in a
                    // chord remains pass-through until the last physical Up; that Up atomically
                    // puts the gate back into Guarding.
                    mode = MouseInputGuardMode.WaitingForExistingRelease;
                    causativeCycle = true;
                }
                else if (mode is MouseInputGuardMode.Guarding or
                         MouseInputGuardMode.DrainingToAwait ||
                         (suppressed & button) != 0)
                {
                    suppressed |= button;
                    suppress = true;
                }
            }
            else
            {
                physical &= ~button;
                if ((suppressed & button) != 0)
                {
                    suppressed &= ~button;
                    suppress = true;
                }

                if (mode == MouseInputGuardMode.WaitingForExistingRelease &&
                    (physical & ~suppressed) == 0)
                {
                    mode = MouseInputGuardMode.Guarding;
                    causativeClickCompleted = causativeCycle;
                    causativeCycle = false;
                }
                else if (mode == MouseInputGuardMode.WaitingForExistingReleaseThenAwait &&
                         (physical & ~suppressed) == 0)
                {
                    mode = MouseInputGuardMode.AwaitingCausativeClick;
                    causativeCycle = false;
                }
                else if (mode == MouseInputGuardMode.Draining && suppressed == 0)
                {
                    mode = MouseInputGuardMode.Released;
                    becameReleased = true;
                    causativeCycle = false;
                }
                else if (mode == MouseInputGuardMode.DrainingToAwait && suppressed == 0)
                {
                    mode = physical == 0
                        ? MouseInputGuardMode.AwaitingCausativeClick
                        : MouseInputGuardMode.WaitingForExistingReleaseThenAwait;
                    causativeCycle = false;
                }
            }

            var next = Pack(physical, suppressed, mode, causativeCycle);
            if (Interlocked.CompareExchange(ref _packedState, next, current) == current)
            {
                return new MouseInputGuardDecision(
                    suppress,
                    becameReleased,
                    causativeClickCompleted);
            }
        }
    }

    private static bool SuppressesNewInput(MouseInputGuardMode mode) =>
        mode is MouseInputGuardMode.Guarding or MouseInputGuardMode.DrainingToAwait;

    private static int Pack(
        int physical,
        int suppressed,
        MouseInputGuardMode mode,
        bool causativeCycle) =>
        ((physical & ButtonMask) << PhysicalShift) |
        ((suppressed & ButtonMask) << SuppressedShift) |
        (((int)mode & ModeMask) << ModeShift) |
        ((causativeCycle ? 1 : 0) << CausativeCycleShift);

    private static int DecodePhysical(int state) => (state >> PhysicalShift) & ButtonMask;

    private static int DecodeSuppressed(int state) => (state >> SuppressedShift) & ButtonMask;

    private static MouseInputGuardMode DecodeMode(int state) =>
        (MouseInputGuardMode)((state >> ModeShift) & ModeMask);

    private static bool DecodeCausativeCycle(int state) =>
        ((state >> CausativeCycleShift) & 1) != 0;
}

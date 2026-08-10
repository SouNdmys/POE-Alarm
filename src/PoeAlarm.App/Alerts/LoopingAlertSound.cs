using System.IO;
using System.Media;
using System.Runtime.InteropServices;
using System.Text;

namespace PoeAlarm.App.Alerts;

internal sealed class LoopingAlertSound : IDisposable
{
    private const uint SndAsync = 0x0001;
    private const uint SndNoDefault = 0x0002;
    private const uint SndMemory = 0x0004;
    private const uint SndLoop = 0x0008;
    private const uint SndSystem = 0x00200000;

    private readonly object soundGate = new();
    private readonly byte[] waveData;
    private GCHandle pinnedWave;
    private Timer? fallbackTimer;
    private bool isPlaying;
    private bool isDisposed;

    public LoopingAlertSound()
    {
        waveData = CreateAlertWave();
        pinnedWave = GCHandle.Alloc(waveData, GCHandleType.Pinned);
    }

    public void Start()
    {
        lock (soundGate)
        {
            if (isDisposed || isPlaying)
            {
                return;
            }

            isPlaying = true;

            var started = false;
            try
            {
                started = PlaySound(
                    pinnedWave.AddrOfPinnedObject(),
                    IntPtr.Zero,
                    SndAsync | SndNoDefault | SndMemory | SndLoop | SndSystem);
            }
            catch (DllNotFoundException)
            {
                // This app targets Windows, but retain a safe fallback for stripped-down systems.
            }
            catch (EntryPointNotFoundException)
            {
                // See comment above.
            }

            if (!started)
            {
                PlayFallbackSound();
                fallbackTimer = new Timer(
                    static _ => PlayFallbackSound(),
                    null,
                    TimeSpan.FromMilliseconds(900),
                    TimeSpan.FromMilliseconds(900));
            }
        }
    }

    public void Stop()
    {
        lock (soundGate)
        {
            if (!isPlaying)
            {
                return;
            }

            isPlaying = false;
            fallbackTimer?.Dispose();
            fallbackTimer = null;

            try
            {
                _ = PlaySound(IntPtr.Zero, IntPtr.Zero, 0);
            }
            catch (DllNotFoundException)
            {
                // Nothing remains to stop when winmm is unavailable.
            }
            catch (EntryPointNotFoundException)
            {
                // Nothing remains to stop when winmm is unavailable.
            }
        }
    }

    public void Dispose()
    {
        lock (soundGate)
        {
            if (isDisposed)
            {
                return;
            }

            // Inline Stop to avoid re-entering the same lock if its implementation changes.
            isPlaying = false;
            fallbackTimer?.Dispose();
            fallbackTimer = null;

            try
            {
                _ = PlaySound(IntPtr.Zero, IntPtr.Zero, 0);
            }
            catch (DllNotFoundException)
            {
                // Nothing remains to stop when winmm is unavailable.
            }
            catch (EntryPointNotFoundException)
            {
                // Nothing remains to stop when winmm is unavailable.
            }

            isDisposed = true;
            if (pinnedWave.IsAllocated)
            {
                pinnedWave.Free();
            }
        }
    }

    private static void PlayFallbackSound()
    {
        try
        {
            SystemSounds.Exclamation.Play();
        }
        catch (InvalidOperationException)
        {
            // A missing/disabled Windows sound scheme should not crash monitoring.
        }
    }

    private static byte[] CreateAlertWave()
    {
        const int sampleRate = 22_050;
        const short channelCount = 1;
        const short bitsPerSample = 16;
        const double durationSeconds = 1.35;

        var sampleCount = (int)(sampleRate * durationSeconds);
        var pcmByteCount = sampleCount * sizeof(short);

        using var stream = new MemoryStream(44 + pcmByteCount);
        using var writer = new BinaryWriter(stream, Encoding.ASCII, leaveOpen: true);

        writer.Write(Encoding.ASCII.GetBytes("RIFF"));
        writer.Write(36 + pcmByteCount);
        writer.Write(Encoding.ASCII.GetBytes("WAVE"));
        writer.Write(Encoding.ASCII.GetBytes("fmt "));
        writer.Write(16);
        writer.Write((short)1);
        writer.Write(channelCount);
        writer.Write(sampleRate);
        writer.Write(sampleRate * channelCount * bitsPerSample / 8);
        writer.Write((short)(channelCount * bitsPerSample / 8));
        writer.Write(bitsPerSample);
        writer.Write(Encoding.ASCII.GetBytes("data"));
        writer.Write(pcmByteCount);

        for (var index = 0; index < sampleCount; index++)
        {
            var elapsed = (double)index / sampleRate;
            var sample = CreateToneSample(elapsed);
            writer.Write(sample);
        }

        writer.Flush();
        return stream.ToArray();
    }

    private static short CreateToneSample(double elapsedSeconds)
    {
        // Three short, alternating tones followed by silence make each loop easy to notice while
        // leaving enough quiet time for speech/game audio to remain intelligible.
        var (segmentStart, segmentDuration, frequency) = elapsedSeconds switch
        {
            >= 0.00 and < 0.19 => (0.00, 0.19, 880.0),
            >= 0.27 and < 0.46 => (0.27, 0.19, 660.0),
            >= 0.54 and < 0.76 => (0.54, 0.22, 990.0),
            _ => (0.0, 0.0, 0.0),
        };

        if (frequency == 0)
        {
            return 0;
        }

        var position = elapsedSeconds - segmentStart;
        const double rampDuration = 0.012;
        var attack = Math.Min(1.0, position / rampDuration);
        var release = Math.Min(1.0, (segmentDuration - position) / rampDuration);
        var envelope = Math.Max(0.0, Math.Min(attack, release));
        var value = Math.Sin(2 * Math.PI * frequency * position) * envelope * 0.46;
        return (short)(value * short.MaxValue);
    }

    [DllImport("winmm.dll", EntryPoint = "PlaySoundW", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PlaySound(IntPtr sound, IntPtr module, uint flags);
}

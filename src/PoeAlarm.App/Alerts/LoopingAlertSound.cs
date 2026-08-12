using System.Buffers.Binary;
using System.IO;
using System.Media;
using System.Runtime.InteropServices;
using System.Text;

namespace PoeAlarm.App.Alerts;

internal sealed class LoopingAlertSound : IDisposable
{
    private const int MaximumCustomWaveBytes = 32 * 1024 * 1024;
    private const uint SndAsync = 0x0001;
    private const uint SndNoDefault = 0x0002;
    private const uint SndMemory = 0x0004;
    private const uint SndLoop = 0x0008;

    private readonly object soundGate = new();
    private readonly byte[] builtInWaveData;
    private readonly byte[] playbackWaveData;
    private readonly bool hasSeparateBuiltInFallback;
    private GCHandle pinnedPlaybackWave;
    private GCHandle pinnedBuiltInFallback;
    private Timer? fallbackTimer;
    private bool isPlaying;
    private bool isDisposed;

    public LoopingAlertSound()
        : this(null)
    {
    }

    /// <summary>
    /// Creates a looping alert. A readable, standard PCM WAV is preferred when supplied;
    /// otherwise the built-in original rare-drop style sound is used.
    /// </summary>
    public LoopingAlertSound(string? customWavePath)
    {
        builtInWaveData = CreateAlertWave();
        if (TryLoadCustomWave(customWavePath, out var customWave, out var resolvedPath))
        {
            playbackWaveData = customWave;
            CustomWavePath = resolvedPath;
            hasSeparateBuiltInFallback = true;
        }
        else
        {
            playbackWaveData = builtInWaveData;
        }

        pinnedPlaybackWave = GCHandle.Alloc(playbackWaveData, GCHandleType.Pinned);
        if (hasSeparateBuiltInFallback)
        {
            pinnedBuiltInFallback = GCHandle.Alloc(builtInWaveData, GCHandleType.Pinned);
        }
    }

    /// <summary>The validated custom WAV currently selected, or <see langword="null"/>.</summary>
    public string? CustomWavePath { get; }

    public bool IsUsingCustomWave => CustomWavePath is not null;

    /// <summary>
    /// Checks whether a file can be used by the zero-dependency WinMM playback path. The accepted
    /// interchange format is RIFF/WAVE, uncompressed PCM, one or two channels, 8-32 bits.
    /// </summary>
    public static bool TryValidateCustomWaveFile(string? path, out string error)
    {
        if (!TryLoadCustomWave(path, out _, out _, out error))
        {
            return false;
        }

        error = string.Empty;
        return true;
    }

    /// <summary>
    /// Starts playback and reports whether WinMM accepted either the selected WAV or the
    /// built-in WAV. The sound intentionally uses the application's multimedia session instead
    /// of <c>SND_SYSTEM</c>: Windows and several audio drivers allow the System Sounds session to
    /// be muted independently, which previously made both custom and built-in alerts appear
    /// successful while remaining inaudible.
    /// </summary>
    public bool Start()
    {
        lock (soundGate)
        {
            if (isDisposed || isPlaying)
            {
                return isPlaying;
            }

            isPlaying = true;

            var started = TryStartWave(pinnedPlaybackWave);
            if (!started && hasSeparateBuiltInFallback)
            {
                // A valid WAV can still be rejected by a particular audio driver. Never let a
                // user's custom notification silence the safety alert: retry the bundled sound.
                started = TryStartWave(pinnedBuiltInFallback);
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

            return started;
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
            if (pinnedPlaybackWave.IsAllocated)
            {
                pinnedPlaybackWave.Free();
            }

            if (pinnedBuiltInFallback.IsAllocated)
            {
                pinnedBuiltInFallback.Free();
            }
        }
    }

    private static bool TryStartWave(GCHandle pinnedWave)
    {
        try
        {
            return pinnedWave.IsAllocated && PlaySound(
                pinnedWave.AddrOfPinnedObject(),
                IntPtr.Zero,
                SndAsync | SndNoDefault | SndMemory | SndLoop);
        }
        catch (DllNotFoundException)
        {
            // This app targets Windows, but retain a safe fallback for stripped-down systems.
            return false;
        }
        catch (EntryPointNotFoundException)
        {
            return false;
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
        const int sampleRate = 44_100;
        const short channelCount = 1;
        const short bitsPerSample = 16;
        const double durationSeconds = 2.15;

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
            var sample = CreateRareDropSample(elapsed);
            writer.Write(sample);
        }

        writer.Flush();
        return stream.ToArray();
    }

    private static short CreateRareDropSample(double elapsedSeconds)
    {
        // This is an original synthesized cue, not an extraction or reconstruction of any POE
        // audio. A soft impact followed by a four-note crystalline rise conveys "valuable drop";
        // the long quiet tail keeps the latched loop noticeable without becoming exhausting.
        var value = CreateImpact(elapsedSeconds) +
                    CreateBell(elapsedSeconds, 0.030, 659.25, 0.55, 0.34) +
                    CreateBell(elapsedSeconds, 0.115, 987.77, 0.64, 0.30) +
                    CreateBell(elapsedSeconds, 0.225, 1_318.51, 0.72, 0.27) +
                    CreateBell(elapsedSeconds, 0.365, 1_975.53, 0.78, 0.20) +
                    CreateBell(elapsedSeconds, 0.515, 2_637.02, 0.62, 0.12);

        // A gentle saturator controls coincident attacks without the hard edge of clipping.
        value = Math.Tanh(value * 1.12) * 0.72;
        return (short)Math.Round(value * short.MaxValue);
    }

    private static double CreateImpact(double elapsedSeconds)
    {
        if (elapsedSeconds is < 0 or > 0.28)
        {
            return 0;
        }

        const double attackSeconds = 0.006;
        var attack = Math.Min(1.0, elapsedSeconds / attackSeconds);
        var decay = Math.Exp(-elapsedSeconds * 15.0);
        var phase = 2.0 * Math.PI *
                    ((126.0 * elapsedSeconds) - (82.0 * elapsedSeconds * elapsedSeconds));
        return Math.Sin(phase) * attack * decay * 0.42;
    }

    private static double CreateBell(
        double elapsedSeconds,
        double startSeconds,
        double frequency,
        double decaySeconds,
        double gain)
    {
        var position = elapsedSeconds - startSeconds;
        if (position is < 0 or > 1.15)
        {
            return 0;
        }

        var attack = Math.Min(1.0, position / 0.0045);
        var decay = Math.Exp(-position / decaySeconds);
        var fundamental = Math.Sin(2.0 * Math.PI * frequency * position);
        var overtone = Math.Sin(2.0 * Math.PI * frequency * 2.006 * position) * 0.31;
        var shimmer = Math.Sin(2.0 * Math.PI * frequency * 3.973 * position) * 0.10;
        return (fundamental + overtone + shimmer) * attack * decay * gain;
    }

    private static bool TryLoadCustomWave(
        string? path,
        out byte[] waveData,
        out string? resolvedPath) =>
        TryLoadCustomWave(path, out waveData, out resolvedPath, out _);

    private static bool TryLoadCustomWave(
        string? path,
        out byte[] waveData,
        out string? resolvedPath,
        out string error)
    {
        waveData = [];
        resolvedPath = null;
        if (string.IsNullOrWhiteSpace(path))
        {
            error = "未选择 WAV 文件。";
            return false;
        }

        try
        {
            resolvedPath = Path.GetFullPath(path.Trim());
            var fileInfo = new FileInfo(resolvedPath);
            if (!fileInfo.Exists)
            {
                error = "WAV 文件不存在。";
                resolvedPath = null;
                return false;
            }

            if (fileInfo.Length is < 44 or > MaximumCustomWaveBytes)
            {
                error = $"WAV 文件大小必须介于 44 字节和 {MaximumCustomWaveBytes / 1024 / 1024} MB 之间。";
                resolvedPath = null;
                return false;
            }

            waveData = File.ReadAllBytes(resolvedPath);
            if (!TryValidatePcmWave(waveData, out error))
            {
                waveData = [];
                resolvedPath = null;
                return false;
            }

            return true;
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException or
                                          ArgumentException or NotSupportedException)
        {
            error = $"无法读取 WAV 文件：{exception.Message}";
            waveData = [];
            resolvedPath = null;
            return false;
        }
    }

    private static bool TryValidatePcmWave(ReadOnlySpan<byte> wave, out string error)
    {
        if (wave.Length < 44 || !wave[..4].SequenceEqual("RIFF"u8) ||
            !wave.Slice(8, 4).SequenceEqual("WAVE"u8))
        {
            error = "文件不是有效的 RIFF/WAVE 音频。";
            return false;
        }

        var riffSize = BinaryPrimitives.ReadUInt32LittleEndian(wave.Slice(4, 4));
        if ((long)riffSize + 8 > wave.Length)
        {
            error = "WAV 文件长度与 RIFF 标头不一致。";
            return false;
        }

        var hasSupportedFormat = false;
        var hasAudioData = false;
        var offset = 12;
        while (offset + 8 <= wave.Length)
        {
            var chunkId = wave.Slice(offset, 4);
            var chunkSize = BinaryPrimitives.ReadUInt32LittleEndian(wave.Slice(offset + 4, 4));
            var contentStart = offset + 8L;
            var contentEnd = contentStart + chunkSize;
            if (contentEnd > wave.Length)
            {
                error = "WAV 数据块超出文件边界。";
                return false;
            }

            if (chunkId.SequenceEqual("fmt "u8))
            {
                if (chunkSize < 16)
                {
                    error = "WAV 格式数据块不完整。";
                    return false;
                }

                var format = BinaryPrimitives.ReadUInt16LittleEndian(wave.Slice((int)contentStart, 2));
                var channels = BinaryPrimitives.ReadUInt16LittleEndian(wave.Slice((int)contentStart + 2, 2));
                var sampleRate = BinaryPrimitives.ReadUInt32LittleEndian(wave.Slice((int)contentStart + 4, 4));
                var blockAlign = BinaryPrimitives.ReadUInt16LittleEndian(wave.Slice((int)contentStart + 12, 2));
                var bitsPerSample = BinaryPrimitives.ReadUInt16LittleEndian(wave.Slice((int)contentStart + 14, 2));
                var expectedBlockAlign = channels * bitsPerSample / 8;
                hasSupportedFormat = format == 1 &&
                                     channels is 1 or 2 &&
                                     sampleRate is >= 8_000 and <= 192_000 &&
                                     bitsPerSample is 8 or 16 or 24 or 32 &&
                                     blockAlign == expectedBlockAlign;
            }
            else if (chunkId.SequenceEqual("data"u8))
            {
                hasAudioData = chunkSize > 0;
            }

            var nextOffset = contentEnd + (chunkSize & 1);
            if (nextOffset > int.MaxValue)
            {
                error = "WAV 文件过大。";
                return false;
            }

            offset = (int)nextOffset;
        }

        if (!hasSupportedFormat)
        {
            error = "仅支持 8–32 位、单声道或双声道的标准 PCM WAV。";
            return false;
        }

        if (!hasAudioData)
        {
            error = "WAV 文件不包含音频数据。";
            return false;
        }

        error = string.Empty;
        return true;
    }

    [DllImport("winmm.dll", EntryPoint = "PlaySoundW", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PlaySound(IntPtr sound, IntPtr module, uint flags);
}

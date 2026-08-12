using System.Collections.Concurrent;
using System.Diagnostics;
using System.IO;
using System.Text;
using Microsoft.ML.OnnxRuntime;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Recognition;

internal sealed class PaddleCtcSession : IDisposable
{
    private const int ModelHeight = 48;
    private const int MinimumModelWidth = 320;
    private const int MaximumModelWidth = 3200;
    private readonly IReadOnlyList<string> _characters;
    private readonly IReadOnlyDictionary<string, int> _characterClasses;
    private readonly InferenceSession _session;
    private readonly string _inputName;
    private readonly string[] _outputNames;
    private readonly ConcurrentBag<InferenceWorkspace> _workspaces = [];
    private readonly TextPolarity _polarity;
    private readonly int _inkMargin;
    private readonly int _rightPadding;
    private readonly int _maximumSegmentWidth;
    private readonly bool _allowLatinTargetSupport;

    public PaddleCtcSession(
        string modelPath,
        string dictionaryPath,
        int threads,
        TextPolarity polarity,
        int inkMargin,
        int rightPadding = 0,
        int maximumSegmentWidth = 0,
        bool allowLatinTargetSupport = false)
        : this(
            LoadDictionary(dictionaryPath),
            options => new InferenceSession(RequireModelPath(modelPath), options),
            threads,
            polarity,
            inkMargin,
            rightPadding,
            maximumSegmentWidth,
            allowLatinTargetSupport)
    {
    }

    public PaddleCtcSession(
        byte[] modelBytes,
        IReadOnlyList<string> dictionary,
        int threads,
        TextPolarity polarity,
        int inkMargin,
        int rightPadding = 0,
        int maximumSegmentWidth = 0,
        bool allowLatinTargetSupport = false)
        : this(
            ValidateDictionary(dictionary),
            options => new InferenceSession(ValidateModelBytes(modelBytes), options),
            threads,
            polarity,
            inkMargin,
            rightPadding,
            maximumSegmentWidth,
            allowLatinTargetSupport)
    {
    }

    private PaddleCtcSession(
        IReadOnlyList<string> dictionary,
        Func<SessionOptions, InferenceSession> sessionFactory,
        int threads,
        TextPolarity polarity,
        int inkMargin,
        int rightPadding,
        int maximumSegmentWidth,
        bool allowLatinTargetSupport)
    {
        if (threads is < 1 or > 16)
        {
            throw new ArgumentOutOfRangeException(nameof(threads));
        }

        if (inkMargin is < 0 or > 8)
        {
            throw new ArgumentOutOfRangeException(nameof(inkMargin));
        }

        if (rightPadding is < 0 or > 128)
        {
            throw new ArgumentOutOfRangeException(nameof(rightPadding));
        }

        if (maximumSegmentWidth != 0 && maximumSegmentWidth is < 320 or > 1600)
        {
            throw new ArgumentOutOfRangeException(nameof(maximumSegmentWidth));
        }

        _characters = dictionary;
        _characterClasses = _characters
            .Select((character, index) => new { Character = character, Class = index + 1 })
            .GroupBy(item => item.Character, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.First().Class, StringComparer.Ordinal);
        _polarity = polarity;
        _inkMargin = inkMargin;
        _rightPadding = rightPadding;
        _maximumSegmentWidth = maximumSegmentWidth;
        _allowLatinTargetSupport = allowLatinTargetSupport;

        using var options = new SessionOptions
        {
            ExecutionMode = ExecutionMode.ORT_SEQUENTIAL,
            GraphOptimizationLevel = GraphOptimizationLevel.ORT_ENABLE_ALL,
            IntraOpNumThreads = threads,
            InterOpNumThreads = 1,
            EnableCpuMemArena = true,
            EnableMemoryPattern = true,
            LogSeverityLevel = OrtLoggingLevel.ORT_LOGGING_LEVEL_ERROR,
        };
        options.AddSessionConfigEntry("session.intra_op.allow_spinning", "0");

        var sessionWatch = Stopwatch.StartNew();
        var session = sessionFactory(options);
        sessionWatch.Stop();
        SessionCreationElapsed = sessionWatch.Elapsed;

        try
        {
            if (session.InputMetadata.Count != 1 || session.OutputMetadata.Count < 1)
            {
                throw new InvalidDataException(
                    $"Expected one model input and at least one output; found " +
                    $"{session.InputMetadata.Count} input(s) and {session.OutputMetadata.Count} output(s).");
            }

            _inputName = session.InputMetadata.Keys.Single();
            _outputNames = [session.OutputMetadata.Keys.First()];
            InputDimensions = [.. session.InputMetadata[_inputName].Dimensions];
            OutputDimensions = [.. session.OutputMetadata[_outputNames[0]].Dimensions];
            _session = session;
        }
        catch
        {
            session.Dispose();
            throw;
        }
    }

    public TimeSpan SessionCreationElapsed { get; }

    public IReadOnlyList<int> InputDimensions { get; }

    public IReadOnlyList<int> OutputDimensions { get; }

    public string InputName => _inputName;

    public string OutputName => _outputNames[0];

    public int DictionarySize => _characters.Count;

    private static string RequireModelPath(string modelPath)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(modelPath);
        return modelPath;
    }

    private static IReadOnlyList<string> LoadDictionary(string dictionaryPath)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(dictionaryPath);
        return File.ReadAllLines(dictionaryPath, Encoding.UTF8);
    }

    private static byte[] ValidateModelBytes(byte[] modelBytes)
    {
        ArgumentNullException.ThrowIfNull(modelBytes);
        if (modelBytes.Length == 0)
        {
            throw new ArgumentException("The ONNX model is empty.", nameof(modelBytes));
        }

        return modelBytes;
    }

    private static IReadOnlyList<string> ValidateDictionary(IReadOnlyList<string> dictionary)
    {
        ArgumentNullException.ThrowIfNull(dictionary);
        if (dictionary.Count == 0)
        {
            throw new ArgumentException("The OCR dictionary is empty.", nameof(dictionary));
        }

        return dictionary;
    }

    public CtcRecognition Recognize(
        PreparedFrame frame,
        string? targetTemplate = null,
        int? maximumSegmentWidthOverride = null)
    {
        ArgumentNullException.ThrowIfNull(frame);
        var maximumSegmentWidth = maximumSegmentWidthOverride ?? _maximumSegmentWidth;
        if (maximumSegmentWidth != 0 && maximumSegmentWidth is < 320 or > 1600)
        {
            throw new ArgumentOutOfRangeException(nameof(maximumSegmentWidthOverride));
        }

        var crop = FindInkCrop(frame, _inkMargin);
        var naturalWidth = CalculateResizedWidth(crop);
        if (maximumSegmentWidth > 0 && naturalWidth > maximumSegmentWidth)
        {
            var segments = SplitCrop(frame, crop, naturalWidth, maximumSegmentWidth);
            var results = segments.Select(segment => RecognizeCrop(frame, segment)).ToArray();
            var combinedText = string.Join(' ', results
                .Select(result => result.Text.Trim())
                .Where(text => text.Length > 0));
            var canonicalMatch = !string.IsNullOrWhiteSpace(targetTemplate) &&
                                 new FullLineAffixMatcher(targetTemplate).IsMatch(combinedText);
            return new CtcRecognition(
                combinedText,
                results.Length == 0 ? 0 : results.Average(result => result.MeanConfidence),
                results.Sum(result => result.TensorWidth),
                results.Sum(result => result.TimeSteps),
                results.Length == 0 ? 0 : results[0].ClassCount,
                TimeSpan.FromTicks(results.Sum(result => result.TensorPreparationElapsed.Ticks)),
                TimeSpan.FromTicks(results.Sum(result => result.InferenceElapsed.Ticks)),
                TimeSpan.FromTicks(results.Sum(result => result.DecodeElapsed.Ticks)),
                string.IsNullOrWhiteSpace(targetTemplate)
                    ? null
                    : canonicalMatch
                        ? new CtcTargetSupport(true, true, [], "segmented canonical text matches")
                        : CtcTargetSupport.Unsupported("segmented text does not strictly match"));
        }

        return RecognizeCrop(frame, crop, targetTemplate);
    }

    private CtcRecognition RecognizeCrop(
        PreparedFrame frame,
        InkCrop crop,
        string? targetTemplate = null)
    {
        if (!_workspaces.TryTake(out var workspace))
        {
            workspace = new InferenceWorkspace();
        }

        try
        {
            var preparationWatch = Stopwatch.StartNew();
            var resizedWidth = CalculateResizedWidth(crop);
            var tensorWidth = Math.Clamp(
                RoundUp(Math.Max(MinimumModelWidth, resizedWidth + _rightPadding), 8),
                8,
                MaximumModelWidth);
            var inputLength = checked(3 * ModelHeight * tensorWidth);
            var inputMemory = workspace.PrepareInput(inputLength);
            FillTensor(frame, crop, resizedWidth, tensorWidth, inputMemory.Span, _polarity);
            preparationWatch.Stop();

            long[] inputShape = [1, 3, ModelHeight, tensorWidth];
            using var inputValue = OrtValue.CreateTensorValueFromMemory<float>(
                OrtMemoryInfo.DefaultInstance,
                inputMemory,
                inputShape);
            var inputs = new Dictionary<string, OrtValue>(1, StringComparer.Ordinal)
            {
                [_inputName] = inputValue,
            };

            var inferenceWatch = Stopwatch.StartNew();
            using var outputValues = _session.Run(workspace.RunOptions, inputs, _outputNames);
            inferenceWatch.Stop();

            var decodeWatch = Stopwatch.StartNew();
            var outputValue = outputValues[0];
            var outputShape = outputValue.GetTensorTypeAndShape().Shape;
            if (outputShape.Length != 3 || outputShape[0] != 1)
            {
                throw new InvalidDataException($"Expected a [1,time,classes] CTC tensor; received [{string.Join(',', outputShape)}].");
            }

            var timeSteps = checked((int)outputShape[1]);
            var classCount = checked((int)outputShape[2]);
            var output = outputValue.GetTensorDataAsSpan<float>();
            if (output.Length != checked(timeSteps * classCount))
            {
                throw new InvalidDataException(
                    $"CTC output contains {output.Length} values; expected {timeSteps * classCount}.");
            }

            var decoded = Decode(output, timeSteps, classCount);
            var targetSupport = string.IsNullOrWhiteSpace(targetTemplate)
                ? null
                : AnalyzeTargetSupport(targetTemplate, decoded, output, timeSteps, classCount);
            decodeWatch.Stop();

            return new CtcRecognition(
                decoded.Text,
                decoded.MeanConfidence,
                tensorWidth,
                timeSteps,
                classCount,
                preparationWatch.Elapsed,
                inferenceWatch.Elapsed,
                decodeWatch.Elapsed,
                targetSupport);
        }
        finally
        {
            _workspaces.Add(workspace);
        }
    }

    private static int CalculateResizedWidth(InkCrop crop) => Math.Min(
        MaximumModelWidth,
        Math.Max(1, (int)Math.Ceiling(crop.Width * (ModelHeight / (double)crop.Height))));

    private static IReadOnlyList<InkCrop> SplitCrop(
        PreparedFrame frame,
        InkCrop crop,
        int naturalWidth,
        int maximumSegmentWidth)
    {
        var segmentCount = Math.Max(2, (int)Math.Ceiling(naturalWidth / (double)maximumSegmentWidth));
        var boundaries = new List<int>(segmentCount + 1) { crop.X };
        var cropEnd = crop.X + crop.Width;
        for (var index = 1; index < segmentCount; index++)
        {
            var ideal = crop.X + (int)Math.Round(crop.Width * (index / (double)segmentCount));
            var minimum = boundaries[^1] + Math.Max(4, crop.Height / 2);
            var remainingSegments = segmentCount - index;
            var maximum = cropEnd - (remainingSegments * Math.Max(4, crop.Height / 2));
            boundaries.Add(FindQuietColumn(frame, crop, ideal, minimum, maximum));
        }

        boundaries.Add(cropEnd);
        var segments = new List<InkCrop>(segmentCount);
        for (var index = 0; index < boundaries.Count - 1; index++)
        {
            var start = boundaries[index];
            var end = boundaries[index + 1];
            segments.Add(new InkCrop(start, crop.Y, Math.Max(1, end - start), crop.Height));
        }

        return segments;
    }

    private static int FindQuietColumn(
        PreparedFrame frame,
        InkCrop crop,
        int ideal,
        int minimum,
        int maximum)
    {
        if (minimum >= maximum)
        {
            return Math.Clamp(ideal, crop.X + 1, crop.X + crop.Width - 1);
        }

        var radius = Math.Max(6, crop.Height);
        var start = Math.Max(minimum, ideal - radius);
        var end = Math.Min(maximum, ideal + radius);
        var bestColumn = Math.Clamp(ideal, start, end);
        var bestInk = int.MaxValue;
        var bestDistance = int.MaxValue;
        for (var x = start; x <= end; x++)
        {
            var ink = 0;
            for (var y = crop.Y; y < crop.Y + crop.Height; y++)
            {
                if (ReadIntensity(frame, x, y) != 0)
                {
                    ink++;
                }
            }

            var distance = Math.Abs(x - ideal);
            if (ink < bestInk || ink == bestInk && distance < bestDistance)
            {
                bestColumn = x;
                bestInk = ink;
                bestDistance = distance;
            }
        }

        return bestColumn;
    }

    public void Dispose()
    {
        while (_workspaces.TryTake(out var workspace))
        {
            workspace.Dispose();
        }

        _session.Dispose();
    }

    private static InkCrop FindInkCrop(PreparedFrame frame, int margin)
    {
        var minimumX = frame.Width;
        var minimumY = frame.Height;
        var maximumX = -1;
        var maximumY = -1;
        var logicalTop = Math.Clamp(frame.LogicalContentTop, 0, frame.Height - 1);
        var logicalBottom = frame.LogicalContentBottom < 0
            ? frame.Height - 1
            : Math.Clamp(frame.LogicalContentBottom, logicalTop, frame.Height - 1);

        for (var y = logicalTop; y <= logicalBottom; y++)
        {
            var row = y * frame.Stride;
            for (var x = 0; x < frame.Width; x++)
            {
                if (frame.BgraPixels[row + (x * 4)] == 0)
                {
                    continue;
                }

                minimumX = Math.Min(minimumX, x);
                minimumY = Math.Min(minimumY, y);
                maximumX = Math.Max(maximumX, x);
                maximumY = Math.Max(maximumY, y);
            }
        }

        if (maximumX < minimumX || maximumY < minimumY)
        {
            return new InkCrop(0, 0, frame.Width, frame.Height);
        }

        minimumX = Math.Max(0, minimumX - margin);
        minimumY = Math.Max(0, minimumY - margin);
        maximumX = Math.Min(frame.Width - 1, maximumX + margin);
        maximumY = Math.Min(frame.Height - 1, maximumY + margin);
        return new InkCrop(minimumX, minimumY, maximumX - minimumX + 1, maximumY - minimumY + 1);
    }

    private static void FillTensor(
        PreparedFrame frame,
        InkCrop crop,
        int resizedWidth,
        int tensorWidth,
        Span<float> output,
        TextPolarity polarity)
    {
        var planeLength = ModelHeight * tensorWidth;
        for (var targetY = 0; targetY < ModelHeight; targetY++)
        {
            var sourceY = ((targetY + 0.5) * crop.Height / ModelHeight) - 0.5;
            var y0 = Math.Clamp((int)Math.Floor(sourceY), 0, crop.Height - 1);
            var y1 = Math.Min(crop.Height - 1, y0 + 1);
            var yFraction = Math.Clamp(sourceY - y0, 0, 1);

            for (var targetX = 0; targetX < resizedWidth; targetX++)
            {
                var sourceX = ((targetX + 0.5) * crop.Width / resizedWidth) - 0.5;
                var x0 = Math.Clamp((int)Math.Floor(sourceX), 0, crop.Width - 1);
                var x1 = Math.Min(crop.Width - 1, x0 + 1);
                var xFraction = Math.Clamp(sourceX - x0, 0, 1);

                var topLeft = ReadIntensity(frame, crop.X + x0, crop.Y + y0);
                var topRight = ReadIntensity(frame, crop.X + x1, crop.Y + y0);
                var bottomLeft = ReadIntensity(frame, crop.X + x0, crop.Y + y1);
                var bottomRight = ReadIntensity(frame, crop.X + x1, crop.Y + y1);
                var top = topLeft + ((topRight - topLeft) * xFraction);
                var bottom = bottomLeft + ((bottomRight - bottomLeft) * xFraction);
                var intensity = top + ((bottom - top) * yFraction);
                if (polarity == TextPolarity.DarkOnLight)
                {
                    intensity = 255 - intensity;
                }

                var normalized = (float)((intensity / 127.5) - 1);
                var pixel = (targetY * tensorWidth) + targetX;
                output[pixel] = normalized;
                output[planeLength + pixel] = normalized;
                output[(planeLength * 2) + pixel] = normalized;
            }
        }
    }

    private static byte ReadIntensity(PreparedFrame frame, int x, int y) =>
        frame.BgraPixels[(y * frame.Stride) + (x * 4)];

    private DecodedText Decode(ReadOnlySpan<float> output, int timeSteps, int classCount)
    {
        var text = new StringBuilder();
        var emissions = new List<CtcEmission>();
        var confidenceTotal = 0d;
        var emittedCount = 0;
        var previousClass = -1;

        for (var time = 0; time < timeSteps; time++)
        {
            var step = output.Slice(time * classCount, classCount);
            var bestClass = 0;
            var bestScore = step[0];
            for (var classIndex = 1; classIndex < step.Length; classIndex++)
            {
                if (step[classIndex] <= bestScore)
                {
                    continue;
                }

                bestClass = classIndex;
                bestScore = step[classIndex];
            }

            if (bestClass != 0 && bestClass != previousClass)
            {
                var dictionaryIndex = bestClass - 1;
                string emitted;
                if (dictionaryIndex < _characters.Count)
                {
                    emitted = _characters[dictionaryIndex];
                }
                else if (dictionaryIndex == _characters.Count)
                {
                    // Some exported PaddleOCR models append use_space_char after loading a
                    // dictionary which already contains its own final space entry.
                    emitted = " ";
                }
                else
                {
                    emitted = "\uFFFD";
                }

                text.Append(emitted);
                emissions.Add(new CtcEmission(emitted, time, bestScore));
                confidenceTotal += bestScore;
                emittedCount++;
            }

            previousClass = bestClass;
        }

        return new DecodedText(
            text.ToString(),
            emittedCount == 0 ? 0 : confidenceTotal / emittedCount,
            emissions);
    }

    private CtcTargetSupport AnalyzeTargetSupport(
        string targetTemplate,
        DecodedText decoded,
        ReadOnlySpan<float> output,
        int timeSteps,
        int classCount)
    {
        var expected = AffixCanonicalizer.Normalize(targetTemplate).Tokens;
        var actual = AffixCanonicalizer.Normalize(decoded.Text).Tokens;
        if (expected.Count != actual.Count)
        {
            return CtcTargetSupport.Unsupported(
                $"canonical token count differs ({expected.Count} vs {actual.Count})");
        }

        var mismatches = new List<(int CharacterOrdinal, string Expected, string Actual)>();
        var characterOrdinal = 0;
        for (var index = 0; index < expected.Count; index++)
        {
            var expectedToken = expected[index];
            var actualToken = actual[index];
            if (expectedToken.Kind != actualToken.Kind)
            {
                return CtcTargetSupport.Unsupported(
                    $"token {index} kind differs ({expectedToken.Kind} vs {actualToken.Kind})");
            }

            if (expectedToken.Kind != AffixTokenKind.Word)
            {
                continue;
            }

            if (!string.Equals(expectedToken.Text, actualToken.Text, StringComparison.Ordinal))
            {
                if (!CanVerifyLexicalSubstitution(
                        expectedToken.Text,
                        actualToken.Text,
                        _allowLatinTargetSupport))
                {
                    return CtcTargetSupport.Unsupported(
                        $"token {index} is not a position-preserving lexical substitution");
                }

                for (var characterIndex = 0; characterIndex < expectedToken.Text.Length; characterIndex++)
                {
                    if (expectedToken.Text[characterIndex] == actualToken.Text[characterIndex])
                    {
                        continue;
                    }

                    mismatches.Add((
                        characterOrdinal + characterIndex,
                        expectedToken.Text[characterIndex].ToString(),
                        actualToken.Text[characterIndex].ToString()));
                }
            }

            characterOrdinal += expectedToken.Text.Length;
        }

        if (mismatches.Count == 0)
        {
            return new CtcTargetSupport(true, true, [], "canonical text already matches");
        }

        var lexicalEmissions = decoded.Emissions
            .SelectMany(emission => NormalizeLexicalEmission(emission))
            .ToArray();
        if (lexicalEmissions.Length != characterOrdinal)
        {
            return CtcTargetSupport.Unsupported(
                $"lexical emission count differs ({characterOrdinal} vs {lexicalEmissions.Length})");
        }

        var support = new List<CtcMismatchSupport>(mismatches.Count);
        foreach (var mismatch in mismatches)
        {
            if (!_characterClasses.TryGetValue(mismatch.Expected, out var expectedClass) || expectedClass >= classCount)
            {
                return CtcTargetSupport.Unsupported(
                    $"target character '{mismatch.Expected}' is absent from the model dictionary");
            }

            var center = lexicalEmissions[mismatch.CharacterOrdinal].TimeStep;
            var best = FindBestSupport(output, timeSteps, classCount, expectedClass, center);
            support.Add(new CtcMismatchSupport(
                mismatch.Expected,
                mismatch.Actual,
                center,
                best.TimeStep,
                best.Rank,
                best.Probability,
                best.TopProbability,
                best.TopProbability <= 0 ? 0 : best.Probability / best.TopProbability));
        }

        var stronglySupported = support.Count <= 2 && support.All(item =>
            item.Rank <= 3 && item.Probability >= 0.01 && item.ProbabilityRatio >= 0.012);
        return new CtcTargetSupport(
            true,
            stronglySupported,
            support,
            stronglySupported
                ? "all substitutions have top-3 target evidence"
                : "one or more substitutions lack strong target evidence");
    }

    private static BestClassSupport FindBestSupport(
        ReadOnlySpan<float> output,
        int timeSteps,
        int classCount,
        int expectedClass,
        int center)
    {
        var best = new BestClassSupport(center, int.MaxValue, 0, 0);
        var start = Math.Max(0, center - 2);
        var end = Math.Min(timeSteps - 1, center + 2);
        for (var time = start; time <= end; time++)
        {
            var step = output.Slice(time * classCount, classCount);
            var expectedProbability = step[expectedClass];
            var topProbability = 0f;
            var rank = 1;
            for (var classIndex = 1; classIndex < step.Length; classIndex++)
            {
                topProbability = Math.Max(topProbability, step[classIndex]);
                if (step[classIndex] > expectedProbability)
                {
                    rank++;
                }
            }

            if (expectedProbability > best.Probability)
            {
                best = new BestClassSupport(time, rank, expectedProbability, topProbability);
            }
        }

        return best;
    }

    private static IEnumerable<CtcEmission> NormalizeLexicalEmission(CtcEmission emission)
    {
        var normalized = emission.Text.Normalize(NormalizationForm.FormKC).ToLowerInvariant();
        foreach (var rune in normalized.EnumerateRunes())
        {
            if (Rune.IsLetter(rune))
            {
                yield return emission with { Text = rune.ToString() };
            }
        }
    }

    private static bool IsSingleHan(string text) =>
        text.Length == 1 && text[0] is >= '\u3400' and <= '\u4DBF' or >= '\u4E00' and <= '\u9FFF' or >= '\uF900' and <= '\uFAFF';

    private static bool CanVerifyLexicalSubstitution(
        string expected,
        string actual,
        bool allowLatinTargetSupport) =>
        IsSingleHan(expected) && IsSingleHan(actual) ||
        allowLatinTargetSupport && expected.Length >= 4 &&
        expected.Length == actual.Length &&
        expected.Zip(actual).Count(pair => pair.First != pair.Second) <= 2;

    private static int RoundUp(int value, int multiple) =>
        checked(((value + multiple - 1) / multiple) * multiple);

    private readonly record struct InkCrop(int X, int Y, int Width, int Height);

    private readonly record struct DecodedText(
        string Text,
        double MeanConfidence,
        IReadOnlyList<CtcEmission> Emissions);

    private readonly record struct CtcEmission(string Text, int TimeStep, float Probability);

    private readonly record struct BestClassSupport(
        int TimeStep,
        int Rank,
        float Probability,
        float TopProbability);

    private sealed class InferenceWorkspace : IDisposable
    {
        private float[] _inputBuffer = [];

        public RunOptions RunOptions { get; } = new();

        public Memory<float> PrepareInput(int inputLength)
        {
            if (_inputBuffer.Length < inputLength)
            {
                // Each concurrent worker retains only its largest observed tensor. With the
                // production concurrency cap this stays bounded to a few MiB.
                _inputBuffer = new float[inputLength];
            }

            var memory = _inputBuffer.AsMemory(0, inputLength);
            memory.Span.Clear();
            return memory;
        }

        public void Dispose() => RunOptions.Dispose();
    }
}

internal enum TextPolarity
{
    BrightOnDark,
    DarkOnLight,
}

internal sealed record CtcRecognition(
    string Text,
    double MeanConfidence,
    int TensorWidth,
    int TimeSteps,
    int ClassCount,
    TimeSpan TensorPreparationElapsed,
    TimeSpan InferenceElapsed,
    TimeSpan DecodeElapsed,
    CtcTargetSupport? TargetSupport)
{
    public TimeSpan TotalElapsed => TensorPreparationElapsed + InferenceElapsed + DecodeElapsed;
}

internal sealed record CtcTargetSupport(
    bool ShapeCompatible,
    bool StronglySupported,
    IReadOnlyList<CtcMismatchSupport> Mismatches,
    string Reason)
{
    public static CtcTargetSupport Unsupported(string reason) => new(false, false, [], reason);
}

internal sealed record CtcMismatchSupport(
    string Expected,
    string Actual,
    int GreedyTimeStep,
    int EvidenceTimeStep,
    int Rank,
    double Probability,
    double TopProbability,
    double ProbabilityRatio);

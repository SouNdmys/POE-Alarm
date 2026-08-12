namespace PoeAlarm.RecognizerProbe;

internal readonly record struct BenchmarkDistribution(
    double Minimum,
    double Mean,
    double P50,
    double P95,
    double P99,
    double Maximum)
{
    public static BenchmarkDistribution Create(IReadOnlyCollection<double> samples)
    {
        ArgumentNullException.ThrowIfNull(samples);
        if (samples.Count == 0)
        {
            return default;
        }

        var ordered = samples.Order().ToArray();
        return new BenchmarkDistribution(
            ordered[0],
            ordered.Average(),
            Percentile(ordered, 0.50),
            Percentile(ordered, 0.95),
            Percentile(ordered, 0.99),
            ordered[^1]);
    }

    public override string ToString() =>
        $"min={Minimum:F2} mean={Mean:F2} p50={P50:F2} p95={P95:F2} p99={P99:F2} max={Maximum:F2} ms";

    private static double Percentile(IReadOnlyList<double> ordered, double percentile)
    {
        if (ordered.Count == 1)
        {
            return ordered[0];
        }

        var position = percentile * (ordered.Count - 1);
        var lower = (int)Math.Floor(position);
        var upper = (int)Math.Ceiling(position);
        if (lower == upper)
        {
            return ordered[lower];
        }

        var fraction = position - lower;
        return ordered[lower] + ((ordered[upper] - ordered[lower]) * fraction);
    }
}

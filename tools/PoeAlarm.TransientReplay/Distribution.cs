namespace PoeAlarm.TransientReplay;

internal readonly record struct Distribution(
    double Minimum,
    double Mean,
    double P50,
    double P95,
    double Maximum)
{
    public static Distribution Create(IEnumerable<double> samples)
    {
        var ordered = samples.Order().ToArray();
        if (ordered.Length == 0)
        {
            return default;
        }

        return new Distribution(
            ordered[0],
            ordered.Average(),
            Percentile(ordered, 0.50),
            Percentile(ordered, 0.95),
            ordered[^1]);
    }

    public override string ToString() =>
        $"min={Minimum:F1} mean={Mean:F1} p50={P50:F1} p95={P95:F1} max={Maximum:F1} ms";

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

        return ordered[lower] + ((ordered[upper] - ordered[lower]) * (position - lower));
    }
}

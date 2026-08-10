using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Monitoring;

public sealed class AffixDetectedEventArgs(
    LogicalAffixMatch match,
    MonitorSnapshot snapshot) : EventArgs
{
    public LogicalAffixMatch Match { get; } = match;

    public MonitorSnapshot Snapshot { get; } = snapshot;
}

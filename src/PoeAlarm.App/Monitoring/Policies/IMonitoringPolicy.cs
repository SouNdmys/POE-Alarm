namespace PoeAlarm.App.Monitoring.Policies;

/// <summary>
/// Controls when monitoring may release input after an item-tooltip state changes. The rule set
/// decides what is desirable; the policy independently decides how much evidence is required
/// before the next crafting input may proceed.
/// </summary>
public interface IMonitoringPolicy
{
    MonitoringPolicyKind Kind { get; }
}

public enum MonitoringPolicyKind
{
    Fast,
    Guarded,
}

/// <summary>Keeps the verified 0.6.1 monitor cadence and input behavior.</summary>
public sealed class FastMonitoringPolicy : IMonitoringPolicy
{
    private FastMonitoringPolicy()
    {
    }

    public static FastMonitoringPolicy Instance { get; } = new();

    public MonitoringPolicyKind Kind => MonitoringPolicyKind.Fast;
}

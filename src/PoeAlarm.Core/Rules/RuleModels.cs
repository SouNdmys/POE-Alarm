using System.Text.Json.Serialization;

namespace PoeAlarm.Core.Rules;

[JsonConverter(typeof(JsonStringEnumConverter))]
public enum NumericConstraintMode
{
    Ignore,
    RangeInclusive,
    AtLeast,
    AtMost,
    Exactly,
}

/// <summary>A constraint bound to one numeric slot in an affix template.</summary>
public sealed record NumericConstraint
{
    public NumericConstraintMode Mode { get; init; } = NumericConstraintMode.Ignore;

    public decimal? Minimum { get; init; }

    public decimal? Maximum { get; init; }

    public decimal? Expected { get; init; }

    public static NumericConstraint Ignored() => new();

    public static NumericConstraint Range(decimal minimum, decimal maximum) =>
        Between(minimum, maximum);

    public static NumericConstraint Between(decimal minimum, decimal maximum) => new()
    {
        Mode = NumericConstraintMode.RangeInclusive,
        Minimum = minimum,
        Maximum = maximum,
    };

    public static NumericConstraint AtLeast(decimal minimum) => new()
    {
        Mode = NumericConstraintMode.AtLeast,
        Minimum = minimum,
    };

    public static NumericConstraint AtMost(decimal maximum) => new()
    {
        Mode = NumericConstraintMode.AtMost,
        Maximum = maximum,
    };

    public static NumericConstraint Exactly(decimal expected) => new()
    {
        Mode = NumericConstraintMode.Exactly,
        Expected = expected,
    };
}

/// <summary>A complete affix sentence plus optional constraints for its numeric slots.</summary>
public sealed record AffixCondition
{
    public AffixCondition()
    {
    }

    public AffixCondition(
        string name,
        string template,
        IReadOnlyList<NumericConstraint>? numericConstraints = null)
    {
        Name = name;
        Template = template;
        NumericConstraints = numericConstraints ?? [];
    }

    public string Name { get; init; } = string.Empty;

    public string Template { get; init; } = string.Empty;

    public IReadOnlyList<NumericConstraint> NumericConstraints { get; init; } = [];
}

[JsonConverter(typeof(JsonStringEnumConverter))]
public enum ResultGroupMode
{
    Any,
    All,
    AtLeast,
}

/// <summary>One result worth stopping for. Conditions inside the result use the selected mode.</summary>
public sealed record AcceptableResultGroup
{
    public AcceptableResultGroup()
    {
    }

    public AcceptableResultGroup(
        string name,
        ResultGroupMode mode,
        IReadOnlyList<AffixCondition> conditions,
        int threshold = 1)
    {
        Name = name;
        Mode = mode;
        Conditions = conditions;
        RequiredCount = threshold;
    }

    public string Name { get; init; } = string.Empty;

    public ResultGroupMode Mode { get; init; } = ResultGroupMode.Any;

    public int RequiredCount { get; init; } = 1;

    public IReadOnlyList<AffixCondition> Conditions { get; init; } = [];
}

/// <summary>Any acceptable result may trigger the alarm.</summary>
public sealed record RuleSetDefinition
{
    public const int CurrentSchemaVersion = 1;
    public const int MaximumGroups = 8;
    public const int MaximumConditions = 32;

    public int SchemaVersion { get; init; } = CurrentSchemaVersion;

    public RuleSetDefinition()
    {
    }

    public RuleSetDefinition(string name, IReadOnlyList<AcceptableResultGroup> groups)
    {
        Name = name;
        Groups = groups;
    }

    public string Name { get; init; } = string.Empty;

    public IReadOnlyList<AcceptableResultGroup> Groups { get; init; } = [];
}

using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Globalization;
using System.Runtime.CompilerServices;
using PoeAlarm.Core.Matching;
using PoeAlarm.Core.Rules;

namespace PoeAlarm.App.RulesUi;

internal sealed record EditorOption<T>(T Value, string Display)
{
    // The app's custom ComboBox template presents SelectionBoxItem directly, so this keeps the
    // closed selection localized as well as the drop-down items.
    public override string ToString() => Display;
}

internal abstract class EditorViewModelBase : INotifyPropertyChanged
{
    public event PropertyChangedEventHandler? PropertyChanged;

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string? propertyName = null)
    {
        if (EqualityComparer<T>.Default.Equals(field, value))
        {
            return false;
        }

        field = value;
        OnPropertyChanged(propertyName);
        return true;
    }

    protected void OnPropertyChanged([CallerMemberName] string? propertyName = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
}

internal sealed class RuleEditorViewModel : EditorViewModelBase
{
    private readonly int maximumPhysicalLineSpan;
    private string ruleSetName;

    public RuleEditorViewModel(
        RuleSetDefinition? source,
        bool isEnglish,
        int maximumPhysicalLineSpan)
    {
        if (maximumPhysicalLineSpan is < 1 or > FullLineAffixMatcher.MaximumSupportedPhysicalLineSpan)
        {
            throw new ArgumentOutOfRangeException(nameof(maximumPhysicalLineSpan));
        }

        this.maximumPhysicalLineSpan = maximumPhysicalLineSpan;
        Text = RuleEditorStrings.For(isEnglish);
        ruleSetName = source?.Name ?? string.Empty;
        GroupModeOptions =
        [
            new EditorOption<ResultGroupMode>(ResultGroupMode.Any, Text.AnyMode),
            new EditorOption<ResultGroupMode>(ResultGroupMode.All, Text.AllMode),
            new EditorOption<ResultGroupMode>(ResultGroupMode.AtLeast, Text.AtLeastMode),
        ];
        NumericModeOptions =
        [
            new EditorOption<NumericConstraintMode>(NumericConstraintMode.Ignore, Text.IgnoreMode),
            new EditorOption<NumericConstraintMode>(NumericConstraintMode.RangeInclusive, Text.RangeMode),
            new EditorOption<NumericConstraintMode>(NumericConstraintMode.AtLeast, Text.AtLeastNumericMode),
            new EditorOption<NumericConstraintMode>(NumericConstraintMode.AtMost, Text.AtMostNumericMode),
            new EditorOption<NumericConstraintMode>(NumericConstraintMode.Exactly, Text.ExactlyMode),
        ];

        var sourceGroups = source?.Groups ?? [];
        if (sourceGroups.Count == 0)
        {
            Groups.Add(CreateGroup());
        }
        else
        {
            foreach (var group in sourceGroups)
            {
                Groups.Add(new ResultGroupEditorViewModel(this, group));
            }
        }

        RefreshCounts();
    }

    public RuleEditorStrings Text { get; }

    public IReadOnlyList<EditorOption<ResultGroupMode>> GroupModeOptions { get; }

    public IReadOnlyList<EditorOption<NumericConstraintMode>> NumericModeOptions { get; }

    public ObservableCollection<ResultGroupEditorViewModel> Groups { get; } = [];

    public string RuleSetName
    {
        get => ruleSetName;
        set => SetField(ref ruleSetName, value);
    }

    public int TotalConditionCount => Groups.Sum(static group => group.Conditions.Count);

    public bool CanAddGroup => Groups.Count < RuleSetDefinition.MaximumGroups &&
                               TotalConditionCount < RuleSetDefinition.MaximumConditions;

    public bool CanAddCondition => TotalConditionCount < RuleSetDefinition.MaximumConditions;

    public string CounterText => string.Format(
        CultureInfo.CurrentCulture,
        Text.CounterFormat,
        Groups.Count,
        RuleSetDefinition.MaximumGroups,
        TotalConditionCount,
        RuleSetDefinition.MaximumConditions);

    public bool TryAddGroup(out string? error)
    {
        if (Groups.Count >= RuleSetDefinition.MaximumGroups)
        {
            error = Text.GroupLimitReached;
            return false;
        }

        if (TotalConditionCount >= RuleSetDefinition.MaximumConditions)
        {
            error = Text.ConditionLimitReached;
            return false;
        }

        Groups.Add(CreateGroup());
        RefreshCounts();
        error = null;
        return true;
    }

    public bool TryRemoveGroup(ResultGroupEditorViewModel group, out string? error)
    {
        if (Groups.Count <= 1)
        {
            error = Text.CannotDeleteOnlyGroup;
            return false;
        }

        _ = Groups.Remove(group);
        RefreshCounts();
        error = null;
        return true;
    }

    public bool TryAddCondition(ResultGroupEditorViewModel group, out string? error)
    {
        if (TotalConditionCount >= RuleSetDefinition.MaximumConditions)
        {
            error = Text.ConditionLimitReached;
            return false;
        }

        group.AddCondition();
        RefreshCounts();
        error = null;
        return true;
    }

    public bool TryRemoveCondition(
        ResultGroupEditorViewModel group,
        AffixConditionEditorViewModel condition,
        out string? error)
    {
        if (group.Conditions.Count <= 1)
        {
            error = Text.CannotDeleteOnlyCondition;
            return false;
        }

        _ = group.Conditions.Remove(condition);
        group.RefreshAfterConditionsChanged();
        RefreshCounts();
        error = null;
        return true;
    }

    public bool TryBuildDefinition(out RuleSetDefinition? definition, out IReadOnlyList<string> errors)
    {
        var validationErrors = new List<string>();
        var groups = new List<AcceptableResultGroup>(Groups.Count);
        for (var groupIndex = 0; groupIndex < Groups.Count; groupIndex++)
        {
            groups.Add(Groups[groupIndex].BuildDefinition(groupIndex, validationErrors));
        }

        if (validationErrors.Count > 0)
        {
            definition = null;
            errors = validationErrors;
            return false;
        }

        var candidate = new RuleSetDefinition(
            (RuleSetName ?? string.Empty).Trim(),
            groups);
        try
        {
            _ = RuleCompiler.Compile(candidate, maximumPhysicalLineSpan);
            definition = candidate;
            errors = [];
            return true;
        }
        catch (RuleValidationException exception)
        {
            definition = null;
            errors = exception.Errors;
            return false;
        }
    }

    public void RefreshCounts()
    {
        OnPropertyChanged(nameof(TotalConditionCount));
        OnPropertyChanged(nameof(CanAddGroup));
        OnPropertyChanged(nameof(CanAddCondition));
        OnPropertyChanged(nameof(CounterText));
    }

    private ResultGroupEditorViewModel CreateGroup()
    {
        var nextIndex = 1;
        while (Groups.Any(group => string.Equals(
                   group.Name,
                   string.Format(
                       CultureInfo.CurrentCulture,
                       Text.ResultDefaultNameFormat,
                       nextIndex),
                   StringComparison.Ordinal)))
        {
            nextIndex++;
        }

        var definition = new AcceptableResultGroup(
            string.Format(CultureInfo.CurrentCulture, Text.ResultDefaultNameFormat, nextIndex),
            ResultGroupMode.AtLeast,
            [new AffixCondition(
                string.Format(CultureInfo.CurrentCulture, Text.ConditionDefaultNameFormat, 1),
                string.Empty)],
            threshold: 1);
        return new ResultGroupEditorViewModel(this, definition);
    }
}

internal sealed class ResultGroupEditorViewModel : EditorViewModelBase
{
    private readonly RuleEditorViewModel owner;
    private string name;
    private ResultGroupMode mode;
    private int requiredCount;
    private bool isExpanded = true;

    public ResultGroupEditorViewModel(
        RuleEditorViewModel owner,
        AcceptableResultGroup source)
    {
        this.owner = owner;
        name = source.Name ?? string.Empty;
        mode = source.Mode;
        requiredCount = Math.Max(1, source.RequiredCount);
        var sourceConditions = source.Conditions ?? [];
        if (sourceConditions.Count == 0)
        {
            AddCondition();
        }
        else
        {
            foreach (var condition in sourceConditions)
            {
                Conditions.Add(new AffixConditionEditorViewModel(owner, condition));
            }
        }

        RefreshAfterConditionsChanged();
    }

    public ObservableCollection<AffixConditionEditorViewModel> Conditions { get; } = [];

    public IReadOnlyList<EditorOption<ResultGroupMode>> ModeOptions => owner.GroupModeOptions;

    public string Name
    {
        get => name;
        set
        {
            if (SetField(ref name, value))
            {
                OnPropertyChanged(nameof(HeaderTitle));
            }
        }
    }

    public ResultGroupMode Mode
    {
        get => mode;
        set
        {
            if (!SetField(ref mode, value))
            {
                return;
            }

            OnPropertyChanged(nameof(IsAtLeast));
            OnPropertyChanged(nameof(HeaderSummary));
        }
    }

    public int RequiredCount
    {
        get => requiredCount;
        set
        {
            var bounded = Math.Clamp(value, 1, Math.Max(1, Conditions.Count));
            if (SetField(ref requiredCount, bounded))
            {
                OnPropertyChanged(nameof(HeaderSummary));
            }
        }
    }

    public bool IsAtLeast => Mode == ResultGroupMode.AtLeast;

    public bool IsExpanded
    {
        get => isExpanded;
        set => SetField(ref isExpanded, value);
    }

    public IReadOnlyList<int> RequiredCountOptions =>
        Enumerable.Range(1, Math.Max(1, Conditions.Count)).ToArray();

    public string HeaderTitle => string.IsNullOrWhiteSpace(Name)
        ? owner.Text.AcceptableResultsHeading
        : Name.Trim();

    public string HeaderSummary => Mode switch
    {
        ResultGroupMode.Any => string.Format(
            CultureInfo.CurrentCulture,
            owner.Text.GroupSummaryAnyFormat,
            Conditions.Count),
        ResultGroupMode.All => string.Format(
            CultureInfo.CurrentCulture,
            owner.Text.GroupSummaryAllFormat,
            Conditions.Count),
        _ => string.Format(
            CultureInfo.CurrentCulture,
            owner.Text.GroupSummaryAtLeastFormat,
            RequiredCount,
            Conditions.Count),
    };

    public void AddCondition()
    {
        var nextIndex = 1;
        while (Conditions.Any(condition => string.Equals(
                   condition.Name,
                   string.Format(
                       CultureInfo.CurrentCulture,
                       owner.Text.ConditionDefaultNameFormat,
                       nextIndex),
                   StringComparison.Ordinal)))
        {
            nextIndex++;
        }

        Conditions.Add(new AffixConditionEditorViewModel(
            owner,
            new AffixCondition(
                string.Format(
                    CultureInfo.CurrentCulture,
                    owner.Text.ConditionDefaultNameFormat,
                    nextIndex),
                string.Empty)));
        RefreshAfterConditionsChanged();
    }

    public void RefreshAfterConditionsChanged()
    {
        RequiredCount = Math.Min(RequiredCount, Math.Max(1, Conditions.Count));
        OnPropertyChanged(nameof(RequiredCountOptions));
        OnPropertyChanged(nameof(HeaderSummary));
    }

    public AcceptableResultGroup BuildDefinition(int groupIndex, ICollection<string> errors)
    {
        var conditions = new List<AffixCondition>(Conditions.Count);
        for (var conditionIndex = 0; conditionIndex < Conditions.Count; conditionIndex++)
        {
            conditions.Add(Conditions[conditionIndex].BuildDefinition(errors));
        }

        var stableName = string.IsNullOrWhiteSpace(Name)
            ? string.Format(
                CultureInfo.CurrentCulture,
                owner.Text.ResultFallbackNameFormat,
                groupIndex + 1)
            : Name.Trim();
        return new AcceptableResultGroup(
            stableName,
            Mode,
            conditions,
            Mode == ResultGroupMode.AtLeast ? RequiredCount : 1);
    }
}

internal sealed class AffixConditionEditorViewModel : EditorViewModelBase
{
    private readonly RuleEditorViewModel owner;
    private IReadOnlyList<NumericConstraint> rememberedConstraints;
    private string name;
    private string template;
    private string templateStatus = string.Empty;
    private bool templateIsValid;

    public AffixConditionEditorViewModel(RuleEditorViewModel owner, AffixCondition source)
    {
        this.owner = owner;
        name = source.Name ?? string.Empty;
        template = source.Template ?? string.Empty;
        rememberedConstraints = source.NumericConstraints ?? [];
        RebuildNumericSlots(rememberedConstraints);
    }

    public ObservableCollection<NumericSlotEditorViewModel> NumericSlots { get; } = [];

    public string Name
    {
        get => name;
        set => SetField(ref name, value);
    }

    public string Template
    {
        get => template;
        set
        {
            if (SetField(ref template, value))
            {
                if (NumericSlots.Count > 0)
                {
                    rememberedConstraints = NumericSlots
                        .Select(static slot => slot.BuildUnchecked())
                        .ToArray();
                }

                RebuildNumericSlots(rememberedConstraints);
            }
        }
    }

    public string TemplateStatus
    {
        get => templateStatus;
        private set => SetField(ref templateStatus, value);
    }

    public bool TemplateIsValid
    {
        get => templateIsValid;
        private set => SetField(ref templateIsValid, value);
    }

    public bool HasNumericSlots => NumericSlots.Count > 0;

    public AffixCondition BuildDefinition(ICollection<string> errors)
    {
        var conditionLabel = string.IsNullOrWhiteSpace(Name)
            ? owner.Text.TemplateLabel
            : Name.Trim();
        if (!TemplateIsValid)
        {
            errors.Add($"{conditionLabel}: {TemplateStatus}");
        }

        var constraints = new List<NumericConstraint>(NumericSlots.Count);
        for (var index = 0; index < NumericSlots.Count; index++)
        {
            if (NumericSlots[index].TryBuild(conditionLabel, index, out var constraint, out var error))
            {
                constraints.Add(constraint!);
            }
            else
            {
                errors.Add(error!);
                constraints.Add(NumericConstraint.Ignored());
            }
        }

        return new AffixCondition(
            conditionLabel,
            (Template ?? string.Empty).Trim(),
            constraints);
    }

    private void RebuildNumericSlots(IReadOnlyList<NumericConstraint> existing)
    {
        NumericSlots.Clear();
        if (string.IsNullOrWhiteSpace(Template))
        {
            TemplateIsValid = false;
            TemplateStatus = owner.Text.EnterTemplate;
            NotifySlotProperties();
            return;
        }

        try
        {
            var matcher = new FullLineAffixMatcher(Template);
            var numericTokens = matcher.Template.Tokens
                .Where(static token => token.Kind != AffixTokenKind.Word)
                .ToArray();
            for (var index = 0; index < numericTokens.Length; index++)
            {
                NumericSlots.Add(new NumericSlotEditorViewModel(
                    owner,
                    index,
                    numericTokens[index].Kind,
                    index < existing.Count ? existing[index] : NumericConstraint.Ignored()));
            }

            TemplateIsValid = true;
            rememberedConstraints = NumericSlots
                .Select(static slot => slot.BuildUnchecked())
                .ToArray();
            TemplateStatus = numericTokens.Length == 0
                ? owner.Text.NoNumericSlots
                : string.Format(
                    CultureInfo.CurrentCulture,
                    owner.Text.NumericSlotsFoundFormat,
                    numericTokens.Length);
        }
        catch (ArgumentException exception)
        {
            TemplateIsValid = false;
            TemplateStatus = $"{owner.Text.InvalidTemplate}: {exception.Message}";
        }

        NotifySlotProperties();
    }

    private void NotifySlotProperties()
    {
        OnPropertyChanged(nameof(NumericSlots));
        OnPropertyChanged(nameof(HasNumericSlots));
    }
}

internal sealed class NumericSlotEditorViewModel : EditorViewModelBase
{
    private readonly RuleEditorViewModel owner;
    private NumericConstraintMode mode;
    private string minimum;
    private string maximum;
    private string expected;

    public NumericSlotEditorViewModel(
        RuleEditorViewModel owner,
        int slotIndex,
        AffixTokenKind kind,
        NumericConstraint source)
    {
        this.owner = owner;
        SlotIndex = slotIndex;
        Kind = kind;
        mode = source.Mode;
        minimum = Format(source.Minimum);
        maximum = Format(source.Maximum);
        expected = Format(source.Expected);
    }

    public AffixTokenKind Kind { get; }

    public int SlotIndex { get; }

    public IReadOnlyList<EditorOption<NumericConstraintMode>> ModeOptions =>
        owner.NumericModeOptions;

    public NumericConstraintMode Mode
    {
        get => mode;
        set
        {
            if (!SetField(ref mode, value))
            {
                return;
            }

            OnPropertyChanged(nameof(ShowRangeInputs));
            OnPropertyChanged(nameof(ShowMinimumInput));
            OnPropertyChanged(nameof(ShowMaximumInput));
            OnPropertyChanged(nameof(ShowExpectedInput));
        }
    }

    public string Minimum
    {
        get => minimum;
        set => SetField(ref minimum, value);
    }

    public string Maximum
    {
        get => maximum;
        set => SetField(ref maximum, value);
    }

    public string Expected
    {
        get => expected;
        set => SetField(ref expected, value);
    }

    public bool ShowRangeInputs => Mode == NumericConstraintMode.RangeInclusive;

    public bool ShowMinimumInput => Mode == NumericConstraintMode.AtLeast;

    public bool ShowMaximumInput => Mode == NumericConstraintMode.AtMost;

    public bool ShowExpectedInput => Mode == NumericConstraintMode.Exactly;

    public string Label
    {
        get
        {
            var kind = Kind switch
            {
                AffixTokenKind.Percent => owner.Text.SlotPercent,
                AffixTokenKind.NegativeNumber => owner.Text.SlotNegativeNumber,
                AffixTokenKind.NegativePercent => owner.Text.SlotNegativePercent,
                _ => owner.Text.SlotPlainNumber,
            };
            return string.Format(
                CultureInfo.CurrentCulture,
                owner.Text.SlotLabelFormat,
                SlotIndex + 1,
                kind);
        }
    }

    public NumericConstraint BuildUnchecked() => Mode switch
    {
        NumericConstraintMode.RangeInclusive => new NumericConstraint
        {
            Mode = Mode,
            Minimum = ParseOrNull(Minimum),
            Maximum = ParseOrNull(Maximum),
        },
        NumericConstraintMode.AtLeast => new NumericConstraint
        {
            Mode = Mode,
            Minimum = ParseOrNull(Minimum),
        },
        NumericConstraintMode.AtMost => new NumericConstraint
        {
            Mode = Mode,
            Maximum = ParseOrNull(Maximum),
        },
        NumericConstraintMode.Exactly => new NumericConstraint
        {
            Mode = Mode,
            Expected = ParseOrNull(Expected),
        },
        _ => NumericConstraint.Ignored(),
    };

    public bool TryBuild(
        string conditionLabel,
        int slotIndex,
        out NumericConstraint? constraint,
        out string? error)
    {
        constraint = null;
        error = null;
        switch (Mode)
        {
            case NumericConstraintMode.Ignore:
                constraint = NumericConstraint.Ignored();
                return true;
            case NumericConstraintMode.RangeInclusive:
                if (!TryParseRequired(Minimum, owner.Text.MinimumLabel, out var lower, out var lowerError))
                {
                    error = FormatError(conditionLabel, slotIndex, lowerError!);
                    return false;
                }

                if (!TryParseRequired(Maximum, owner.Text.MaximumLabel, out var upper, out var upperError))
                {
                    error = FormatError(conditionLabel, slotIndex, upperError!);
                    return false;
                }

                if (lower > upper)
                {
                    error = string.Format(
                        CultureInfo.CurrentCulture,
                        owner.Text.RangeOrderErrorFormat,
                        conditionLabel,
                        slotIndex + 1);
                    return false;
                }

                constraint = NumericConstraint.Between(lower, upper);
                return true;
            case NumericConstraintMode.AtLeast:
                if (!TryParseRequired(Minimum, owner.Text.MinimumLabel, out var minimumValue, out var minimumError))
                {
                    error = FormatError(conditionLabel, slotIndex, minimumError!);
                    return false;
                }

                constraint = NumericConstraint.AtLeast(minimumValue);
                return true;
            case NumericConstraintMode.AtMost:
                if (!TryParseRequired(Maximum, owner.Text.MaximumLabel, out var maximumValue, out var maximumError))
                {
                    error = FormatError(conditionLabel, slotIndex, maximumError!);
                    return false;
                }

                constraint = NumericConstraint.AtMost(maximumValue);
                return true;
            case NumericConstraintMode.Exactly:
                if (!TryParseRequired(Expected, owner.Text.ExactValueLabel, out var exactValue, out var exactError))
                {
                    error = FormatError(conditionLabel, slotIndex, exactError!);
                    return false;
                }

                constraint = NumericConstraint.Exactly(exactValue);
                return true;
            default:
                error = FormatError(conditionLabel, slotIndex, Mode.ToString());
                return false;
        }
    }

    private string FormatError(string conditionLabel, int slotIndex, string field) =>
        string.Format(
            CultureInfo.CurrentCulture,
            owner.Text.InvalidNumberFormat,
            conditionLabel,
            slotIndex + 1,
            $"{field} ");

    private static bool TryParseRequired(
        string value,
        string field,
        out decimal number,
        out string? error)
    {
        if (TryParse(value, out number))
        {
            error = null;
            return true;
        }

        error = field;
        return false;
    }

    private static decimal? ParseOrNull(string value) =>
        TryParse(value, out var result) ? result : null;

    private static bool TryParse(string value, out decimal number) =>
        decimal.TryParse(
            value.Trim(),
            NumberStyles.AllowLeadingSign | NumberStyles.AllowDecimalPoint,
            CultureInfo.InvariantCulture,
            out number) ||
        decimal.TryParse(
            value.Trim(),
            NumberStyles.AllowLeadingSign | NumberStyles.AllowDecimalPoint,
            CultureInfo.CurrentCulture,
            out number);

    private static string Format(decimal? value) =>
        value?.ToString(CultureInfo.InvariantCulture) ?? string.Empty;
}

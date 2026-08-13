using PoeAlarm.App.RulesUi;
using PoeAlarm.Core.Rules;

var tests = new (string Name, Action Run)[]
{
    ("editing uses a detached copy", EditingUsesDetachedCopy),
    ("numeric slots build all constraint modes", NumericSlotsBuildAllConstraintModes),
    ("temporary invalid template keeps slot constraints", InvalidTemplateKeepsSlotConstraints),
    ("POE2 three-line template uses the supplied span", Poe2ThreeLineTemplateUsesSuppliedSpan),
    ("invalid numeric input is rejected before compile", InvalidNumericInputIsRejected),
    ("group and condition minimums are enforced", MinimumCountsAreEnforced),
    ("group maximum is enforced", GroupMaximumIsEnforced),
    ("condition maximum is enforced", ConditionMaximumIsEnforced),
    ("empty rule set receives an editable starter group", EmptyRuleSetGetsStarterGroup),
    ("player-facing copy stays plain and language-consistent", PlayerFacingCopyStaysPlain),
};

var passed = 0;
foreach (var test in tests)
{
    try
    {
        test.Run();
        passed++;
        Console.WriteLine($"PASS {test.Name}");
    }
    catch (Exception exception)
    {
        Console.Error.WriteLine($"FAIL {test.Name}: {exception.Message}");
    }
}

Console.WriteLine($"{passed}/{tests.Length} editor tests passed.");
return passed == tests.Length ? 0 : 1;

static void EditingUsesDetachedCopy()
{
    var originalConstraint = NumericConstraint.AtLeast(15);
    var source = RuleSet(
        new AcceptableResultGroup(
            "Original result",
            ResultGroupMode.AtLeast,
            [new AffixCondition("Original affix", "Adds # to # Physical Damage", [originalConstraint])],
            threshold: 1));
    var editor = new RuleEditorViewModel(source, isEnglish: true, maximumPhysicalLineSpan: 3);

    editor.RuleSetName = "Changed";
    editor.Groups[0].Name = "Changed result";
    editor.Groups[0].Conditions[0].Name = "Changed affix";
    editor.Groups[0].Conditions[0].Template = "#% increased Attack Speed";

    Assert(source.Name == "Test rules", "The source rule-set name changed.");
    Assert(source.Groups[0].Name == "Original result", "The source group changed.");
    Assert(source.Groups[0].Conditions[0].Name == "Original affix", "The source condition changed.");
    Assert(
        source.Groups[0].Conditions[0].Template == "Adds # to # Physical Damage",
        "The source template changed.");
    Assert(originalConstraint.Minimum == 15, "The source constraint changed.");
}

static void NumericSlotsBuildAllConstraintModes()
{
    var source = RuleSet(
        new AcceptableResultGroup(
            "Damage",
            ResultGroupMode.All,
            [new AffixCondition("Damage range", "Adds # to # Physical Damage")],
            threshold: 1));
    var editor = new RuleEditorViewModel(source, isEnglish: true, maximumPhysicalLineSpan: 3);
    var condition = editor.Groups[0].Conditions[0];
    Assert(condition.NumericSlots.Count == 2, "Expected two numeric slots.");

    condition.NumericSlots[0].Mode = NumericConstraintMode.RangeInclusive;
    condition.NumericSlots[0].Minimum = "10.5";
    condition.NumericSlots[0].Maximum = "20.5";
    condition.NumericSlots[1].Mode = NumericConstraintMode.Exactly;
    condition.NumericSlots[1].Expected = "42";

    Assert(editor.TryBuildDefinition(out var definition, out var errors), string.Join(" | ", errors));
    var constraints = definition!.Groups[0].Conditions[0].NumericConstraints;
    Assert(constraints[0] == NumericConstraint.Between(10.5m, 20.5m), "Range was not built.");
    Assert(constraints[1] == NumericConstraint.Exactly(42), "Exact constraint was not built.");

    condition.NumericSlots[0].Mode = NumericConstraintMode.AtLeast;
    condition.NumericSlots[0].Minimum = "11";
    condition.NumericSlots[1].Mode = NumericConstraintMode.AtMost;
    condition.NumericSlots[1].Maximum = "99";
    Assert(editor.TryBuildDefinition(out definition, out errors), string.Join(" | ", errors));
    constraints = definition!.Groups[0].Conditions[0].NumericConstraints;
    Assert(constraints[0] == NumericConstraint.AtLeast(11), "AtLeast was not built.");
    Assert(constraints[1] == NumericConstraint.AtMost(99), "AtMost was not built.");
}

static void InvalidNumericInputIsRejected()
{
    var editor = new RuleEditorViewModel(
        RuleSet(new AcceptableResultGroup(
            "Speed",
            ResultGroupMode.Any,
            [new AffixCondition("Speed", "#% increased Attack Speed")],
            threshold: 1)),
        isEnglish: true,
        maximumPhysicalLineSpan: 3);
    var slot = editor.Groups[0].Conditions[0].NumericSlots[0];
    slot.Mode = NumericConstraintMode.RangeInclusive;
    slot.Minimum = "25";
    slot.Maximum = "not-a-number";

    Assert(!editor.TryBuildDefinition(out _, out var errors), "Invalid input was accepted.");
    Assert(errors.Count == 1, "Invalid input should produce one focused error.");

    slot.Maximum = "20";
    Assert(!editor.TryBuildDefinition(out _, out errors), "An inverted range was accepted.");
    Assert(errors.Count == 1, "Inverted range should produce one focused error.");
}

static void Poe2ThreeLineTemplateUsesSuppliedSpan()
{
    const string threeLineTemplate = "Gain # Rage when you\nHit a Rare or Unique Enemy, no\nmore than once every # seconds";
    var source = RuleSet(new AcceptableResultGroup(
        "POE2",
        ResultGroupMode.Any,
        [new AffixCondition("Rage", threeLineTemplate)],
        threshold: 1));

    var poe2Editor = new RuleEditorViewModel(
        source,
        isEnglish: true,
        maximumPhysicalLineSpan: 3);
    Assert(
        poe2Editor.TryBuildDefinition(out var definition, out var poe2Errors),
        $"POE2 span rejected its three-line template: {string.Join(" | ", poe2Errors)}");
    var lines = threeLineTemplate.Split('\n')
        .Select(static line => line.Replace("#", "3", StringComparison.Ordinal))
        .ToArray();
    Assert(
        RuleCompiler.Compile(definition!, maximumPhysicalLineSpan: 3).Evaluate(lines).IsMatch,
        "The saved POE2 definition did not match its three physical lines at span 3.");
    Assert(
        !RuleCompiler.Compile(definition!, maximumPhysicalLineSpan: 2).Evaluate(lines).IsMatch,
        "The test fixture does not distinguish the POE2 three-line scan span.");
}

static void InvalidTemplateKeepsSlotConstraints()
{
    var editor = new RuleEditorViewModel(
        RuleSet(new AcceptableResultGroup(
            "Speed",
            ResultGroupMode.Any,
            [new AffixCondition(
                "Speed",
                "#% increased Attack Speed",
                [NumericConstraint.AtLeast(22)])],
            threshold: 1)),
        isEnglish: true,
        maximumPhysicalLineSpan: 3);
    var condition = editor.Groups[0].Conditions[0];
    Assert(condition.NumericSlots[0].Minimum == "22", "Initial constraint was not loaded.");

    condition.Template = string.Empty;
    Assert(condition.NumericSlots.Count == 0, "Invalid template retained visible slots.");
    condition.Template = "#% increased Attack Speed";
    Assert(condition.NumericSlots.Count == 1, "Restored template did not rebuild slots.");
    Assert(condition.NumericSlots[0].Mode == NumericConstraintMode.AtLeast, "Constraint mode was lost.");
    Assert(condition.NumericSlots[0].Minimum == "22", "Constraint value was lost.");
}

static void MinimumCountsAreEnforced()
{
    var editor = new RuleEditorViewModel(
        RuleSet(new AcceptableResultGroup(
            "Only",
            ResultGroupMode.Any,
            [new AffixCondition("Only", "#% increased Attack Speed")],
            threshold: 1)),
        isEnglish: false,
        maximumPhysicalLineSpan: 3);

    Assert(!editor.TryRemoveGroup(editor.Groups[0], out _), "Removed the only result.");
    Assert(
        !editor.TryRemoveCondition(editor.Groups[0], editor.Groups[0].Conditions[0], out _),
        "Removed the only condition.");
}

static void GroupMaximumIsEnforced()
{
    var groups = Enumerable.Range(1, RuleSetDefinition.MaximumGroups)
        .Select(index => new AcceptableResultGroup(
            $"Group {index}",
            ResultGroupMode.Any,
            [new AffixCondition($"Affix {index}", "#% increased Attack Speed")],
            threshold: 1))
        .ToArray();
    var editor = new RuleEditorViewModel(
        new RuleSetDefinition("Groups", groups),
        isEnglish: true,
        maximumPhysicalLineSpan: 3);

    Assert(!editor.TryAddGroup(out _), "Added a ninth result.");
    Assert(editor.Groups.Count == RuleSetDefinition.MaximumGroups, "Result count changed.");
}

static void ConditionMaximumIsEnforced()
{
    var conditions = Enumerable.Range(1, RuleSetDefinition.MaximumConditions)
        .Select(index => new AffixCondition($"Affix {index}", "#% increased Attack Speed"))
        .ToArray();
    var editor = new RuleEditorViewModel(
        RuleSet(new AcceptableResultGroup(
            "Full",
            ResultGroupMode.Any,
            conditions,
            threshold: 1)),
        isEnglish: true,
        maximumPhysicalLineSpan: 3);

    Assert(!editor.TryAddCondition(editor.Groups[0], out _), "Added a 33rd condition.");
    Assert(editor.TotalConditionCount == RuleSetDefinition.MaximumConditions, "Condition count changed.");
}

static void EmptyRuleSetGetsStarterGroup()
{
    var editor = new RuleEditorViewModel(
        new RuleSetDefinition("Empty", []),
        isEnglish: true,
        maximumPhysicalLineSpan: 3);
    Assert(editor.Groups.Count == 1, "Starter result was not added.");
    Assert(editor.Groups[0].Conditions.Count == 1, "Starter condition was not added.");
    Assert(!editor.TryBuildDefinition(out _, out _), "An empty starter template was saved.");
}

static void PlayerFacingCopyStaysPlain()
{
    var chinese = RuleEditorStrings.For(isEnglish: false);
    var english = RuleEditorStrings.For(isEnglish: true);

    Assert(!chinese.DisplayValueWarning.Contains("Catalyst", StringComparison.OrdinalIgnoreCase) &&
           !chinese.DisplayValueWarning.Contains("tooltip", StringComparison.OrdinalIgnoreCase) &&
           !chinese.OrExplanation.Contains("OR", StringComparison.Ordinal),
        "Chinese rule-editor copy contains avoidable English jargon.");
    Assert(!chinese.TemplateHelp.Contains("数值槽", StringComparison.Ordinal) &&
           !chinese.NumericSlotsFoundFormat.Contains("数值槽", StringComparison.Ordinal),
        "Chinese rule-editor copy exposes the numeric-slot implementation term.");
    Assert(!english.DisplayValueWarning.Any(character =>
            character is >= '\u3400' and <= '\u9FFF'),
        "English rule-editor copy contains Chinese characters.");
}

static RuleSetDefinition RuleSet(AcceptableResultGroup group) =>
    new("Test rules", [group]);

static void Assert(bool condition, string message)
{
    if (!condition)
    {
        throw new InvalidOperationException(message);
    }
}

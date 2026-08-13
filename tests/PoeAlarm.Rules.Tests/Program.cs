using System.Diagnostics;
using System.Text.Json;
using PoeAlarm.Core.Rules;

var tests = new (string Name, Action Run)[]
{
    ("Numeric values can be ignored", NumericValuesCanBeIgnored),
    ("Numeric range boundaries are inclusive", NumericRangeBoundariesAreInclusive),
    ("At-least boundary is inclusive", AtLeastBoundaryIsInclusive),
    ("At-most boundary is inclusive", AtMostBoundaryIsInclusive),
    ("Exact decimal and signed values match", ExactDecimalAndSignedValuesMatch),
    ("Traditional Chinese OCR decimal glyph keeps one numeric slot", TraditionalChineseOcrDecimalGlyphKeepsOneNumericSlot),
    ("Every numeric slot has its own constraint", EveryNumericSlotHasItsOwnConstraint),
    ("Missing and extra numeric slots are rejected", MissingAndExtraNumericSlotsAreRejected),
    ("Displayed roll range uses the rolled value", DisplayedRollRangeUsesRolledValue),
    ("Rule definitions survive JSON round trip", RuleDefinitionsSurviveJsonRoundTrip),
    ("Malformed deserialized definitions return validation errors", MalformedDeserializedDefinitionsReturnValidationErrors),
    ("Acceptable result groups are OR branches", AcceptableResultGroupsAreOrBranches),
    ("Several acceptable groups preserve OR evidence", SeveralAcceptableGroupsPreserveOrEvidence),
    ("Any mode requires one condition", AnyModeRequiresOneCondition),
    ("All mode requires every condition", AllModeRequiresEveryCondition),
    ("At-least mode honors its threshold", AtLeastModeHonorsItsThreshold),
    ("One observed affix cannot satisfy two conditions", OneObservedAffixCannotSatisfyTwoConditions),
    ("Separate identical affixes can satisfy duplicate conditions", SeparateIdenticalAffixesCanSatisfyDuplicateConditions),
    ("Specific numeric condition receives the qualifying observation", SpecificNumericConditionReceivesQualifyingObservation),
    ("Overlapping conditions require a complete one-to-one assignment", OverlappingConditionsRequireCompleteOneToOneAssignment),
    ("Overlapping physical spans cannot count one wrapped modifier twice", OverlappingPhysicalSpansCannotCountOneWrappedModifierTwice),
    ("Assisted transcripts participate without losing physical identity", AssistedTranscriptsParticipateWithoutLosingPhysicalIdentity),
    ("Primary and assisted alternatives from one band count once", PrimaryAndAssistedAlternativesFromOneBandCountOnce),
    ("Wrapped assisted bands collapse transitively", WrappedAssistedBandsCollapseTransitively),
    ("Wrapped assisted alternatives union every source band", WrappedAssistedAlternativesUnionEverySourceBand),
    ("Wrapped affixes are one logical observation", WrappedAffixesAreOneLogicalObservation),
    ("Blank lines remain logical boundaries", BlankLinesRemainLogicalBoundaries),
    ("Specific affix semantics stay distinct", SpecificAffixSemanticsStayDistinct),
    ("Invalid rules are rejected with validation details", InvalidRulesAreRejectedWithValidationDetails),
    ("Evaluation explains matches and numeric failures", EvaluationExplainsMatchesAndNumericFailures),
    ("Twenty-condition evaluation remains sub-millisecond per call", TwentyConditionEvaluationRemainsFast),
    ("Dense duplicate assignment remains bounded", DenseDuplicateAssignmentRemainsBounded),
};

var failures = new List<string>();
foreach (var (name, run) in tests)
{
    try
    {
        run();
        Console.WriteLine($"PASS  {name}");
    }
    catch (Exception exception)
    {
        failures.Add($"{name}: {exception.Message}");
        Console.WriteLine($"FAIL  {name}: {exception.Message}");
    }
}

Console.WriteLine($"{tests.Length - failures.Count}/{tests.Length} rule-engine tests passed.");
if (failures.Count > 0)
{
    Environment.ExitCode = 1;
}

static void NumericValuesCanBeIgnored()
{
    var explicitIgnore = OneCondition("#% increased Physical Damage", NumericConstraint.Ignored());
    True(explicitIgnore.Evaluate(["12% increased Physical Damage"]).IsMatch);
    True(explicitIgnore.Evaluate(["179% increased Physical Damage"]).IsMatch);

    var implicitIgnore = OneCondition("#% increased Physical Damage");
    True(implicitIgnore.Evaluate(["1% increased Physical Damage"]).IsMatch);
}

static void NumericRangeBoundariesAreInclusive()
{
    var rules = OneCondition(
        "#% increased Physical Damage",
        NumericConstraint.Range(170m, 179m));

    True(rules.Evaluate(["170% increased Physical Damage"]).IsMatch);
    True(rules.Evaluate(["179% increased Physical Damage"]).IsMatch);
    False(rules.Evaluate(["169.99% increased Physical Damage"]).IsMatch);
    False(rules.Evaluate(["179.01% increased Physical Damage"]).IsMatch);
}

static void AtLeastBoundaryIsInclusive()
{
    var rules = OneCondition(
        "#% increased Physical Damage",
        NumericConstraint.AtLeast(170m));

    False(rules.Evaluate(["169.99% increased Physical Damage"]).IsMatch);
    True(rules.Evaluate(["170% increased Physical Damage"]).IsMatch);
    True(rules.Evaluate(["999% increased Physical Damage"]).IsMatch);
}

static void AtMostBoundaryIsInclusive()
{
    var rules = OneCondition("+# to maximum Life", NumericConstraint.AtMost(70m));

    True(rules.Evaluate(["+69.5 to maximum Life"]).IsMatch);
    True(rules.Evaluate(["+70 to maximum Life"]).IsMatch);
    False(rules.Evaluate(["+70.01 to maximum Life"]).IsMatch);
}

static void ExactDecimalAndSignedValuesMatch()
{
    var decimalRules = OneCondition(
        "Regenerate # Life per second",
        NumericConstraint.Exactly(0.8m));
    True(decimalRules.Evaluate(["Regenerate 0.8 Life per second"]).IsMatch);
    False(decimalRules.Evaluate(["Regenerate 0.81 Life per second"]).IsMatch);

    var signedRules = OneCondition(
        "-#% to Fire Resistance",
        NumericConstraint.Exactly(-12m));
    True(signedRules.Evaluate(["-12% to Fire Resistance"]).IsMatch);
    False(signedRules.Evaluate(["-11% to Fire Resistance"]).IsMatch);
}

static void TraditionalChineseOcrDecimalGlyphKeepsOneNumericSlot()
{
    var rules = OneCondition(
        "+(3.11—3.8)%暴擊率",
        NumericConstraint.AtLeast(3.1m));

    var hit = rules.Evaluate(["+ 3 · 73 % 暴 擊 率"]);
    True(hit.IsMatch);
    Equal(3.73m, hit.Groups.Single().Conditions.Single().ActualValues.Single());

    False(rules.Evaluate(["+ 3 ∙ 09 % 暴 擊 率"]).IsMatch);
    True(rules.Evaluate(["+3⋅73%暴擊率"]).IsMatch);

    var twoValues = OneCondition(
        "增加 (3—4) 點力量和 (73—74) 點敏捷",
        NumericConstraint.AtLeast(3m),
        NumericConstraint.AtLeast(73m));
    False(twoValues.Evaluate(["增加 3・73 點力量和敏捷"]).IsMatch);
}

static void EveryNumericSlotHasItsOwnConstraint()
{
    var rules = OneCondition(
        "Adds # to # Physical Damage",
        NumericConstraint.Range(37m, 55m),
        NumericConstraint.AtLeast(90m));

    True(rules.Evaluate(["Adds 37 to 90 Physical Damage"]).IsMatch);
    True(rules.Evaluate(["Adds 55 to 94 Physical Damage"]).IsMatch);
    False(rules.Evaluate(["Adds 36 to 94 Physical Damage"]).IsMatch);
    False(rules.Evaluate(["Adds 55 to 89 Physical Damage"]).IsMatch);

    var secondSlotOnly = OneCondition(
        "Adds # to # Physical Damage",
        NumericConstraint.Ignored(),
        NumericConstraint.Exactly(94m));
    True(secondSlotOnly.Evaluate(["Adds 1 to 94 Physical Damage"]).IsMatch);
}

static void MissingAndExtraNumericSlotsAreRejected()
{
    var rules = OneCondition(
        "Adds # to # Physical Damage",
        NumericConstraint.Ignored(),
        NumericConstraint.Ignored());

    False(rules.Evaluate(["Adds 50 Physical Damage"]).IsMatch);
    False(rules.Evaluate(["Adds 10 to 20 to 30 Physical Damage"]).IsMatch);
}

static void DisplayedRollRangeUsesRolledValue()
{
    var rules = OneCondition(
        "#% increased Physical Damage",
        NumericConstraint.Exactly(179m));

    var hit = rules.Evaluate(["179(170-179)% increased Physical Damage"]);
    True(hit.IsMatch);
    Equal(179m, hit.Groups.Single().Conditions.Single().ActualValues.Single());
    False(rules.Evaluate(["178(170-179)% increased Physical Damage"]).IsMatch);

    var twoSlots = OneCondition(
        "Adds # to # Physical Damage",
        NumericConstraint.Exactly(55m),
        NumericConstraint.Exactly(94m));
    True(twoSlots.Evaluate([
        "Adds 55(37-55) to 94(63-94) Physical Damage"]).IsMatch);
}

static void RuleDefinitionsSurviveJsonRoundTrip()
{
    var definition = new RuleSetDefinition(
        "mirror weapon",
        [new AcceptableResultGroup(
            "T1 physical and speed",
            ResultGroupMode.AtLeast,
            [Condition(
                "physical",
                "#% increased Physical Damage",
                NumericConstraint.Range(170m, 179m)),
             Condition(
                "attack speed",
                "#% increased Attack Speed",
                NumericConstraint.AtLeast(20.5m)),
             Condition(
                "life",
                "+# to maximum Life",
                NumericConstraint.Ignored())],
            threshold: 2)]);
    var options = new JsonSerializerOptions { WriteIndented = true };

    var json = JsonSerializer.Serialize(definition, options);
    var restored = JsonSerializer.Deserialize<RuleSetDefinition>(json);
    NotNull(restored);
    Equal(RuleSetDefinition.CurrentSchemaVersion, restored!.SchemaVersion);
    Equal(definition.Name, restored.Name);
    Equal(1, restored.Groups.Count);
    Equal(ResultGroupMode.AtLeast, restored.Groups[0].Mode);
    Equal(2, restored.Groups[0].RequiredCount);
    Equal(NumericConstraintMode.RangeInclusive,
        restored.Groups[0].Conditions[0].NumericConstraints[0].Mode);
    Equal(170m, restored.Groups[0].Conditions[0].NumericConstraints[0].Minimum);
    Equal(179m, restored.Groups[0].Conditions[0].NumericConstraints[0].Maximum);
    Equal(NumericConstraintMode.AtLeast,
        restored.Groups[0].Conditions[1].NumericConstraints[0].Mode);
    Equal(20.5m, restored.Groups[0].Conditions[1].NumericConstraints[0].Minimum);

    var rules = RuleCompiler.Compile(restored);
    True(rules.Evaluate([
        "179(170-179)% increased Physical Damage",
        "21.5% increased Attack Speed"]).IsMatch);
}

static void MalformedDeserializedDefinitionsReturnValidationErrors()
{
    const string nullGroups = """
        { "SchemaVersion": 1, "Name": "bad", "Groups": null }
        """;
    const string nullConditions = """
        {
          "SchemaVersion": 1,
          "Name": "bad",
          "Groups": [
            { "Name": "bad group", "Mode": "Any", "RequiredCount": 1, "Conditions": null }
          ]
        }
        """;
    const string nullConstraints = """
        {
          "SchemaVersion": 1,
          "Name": "bad",
          "Groups": [
            {
              "Name": "bad group",
              "Mode": "Any",
              "RequiredCount": 1,
              "Conditions": [
                { "Name": "bad condition", "Template": "+# to maximum Life", "NumericConstraints": null }
              ]
            }
          ]
        }
        """;

    AssertInvalidJsonDefinition(nullGroups, "acceptable results");
    AssertInvalidJsonDefinition(nullConditions, "conditions");
    AssertInvalidJsonDefinition(nullConstraints, "numeric");

    var nullGroupEntry = new RuleSetDefinition
    {
        Name = "bad",
        Groups = [null!],
    };
    ValidationContains(nullGroupEntry, "acceptable result");

    var nullConditionEntry = new RuleSetDefinition
    {
        Name = "bad",
        Groups =
        [
            new AcceptableResultGroup
            {
                Name = "bad group",
                Conditions = [null!],
            },
        ],
    };
    ValidationContains(nullConditionEntry, "condition");

    var nullConstraintEntry = new RuleSetDefinition
    {
        Name = "bad",
        Groups =
        [
            new AcceptableResultGroup
            {
                Name = "bad group",
                Conditions =
                [
                    new AffixCondition
                    {
                        Name = "bad condition",
                        Template = "+# to maximum Life",
                        NumericConstraints = [null!],
                    },
                ],
            },
        ],
    };
    ValidationContains(nullConstraintEntry, "numeric");
}

static void AcceptableResultGroupsAreOrBranches()
{
    var rules = Compile(
        new AcceptableResultGroup(
            "phys",
            ResultGroupMode.All,
            [Condition("phys-damage", "#% increased Physical Damage", NumericConstraint.AtLeast(170m)),
             Condition("attack-speed", "#% increased Attack Speed")]),
        new AcceptableResultGroup(
            "spell",
            ResultGroupMode.All,
            [Condition("spell-crit", "#% increased Spell Critical Strike Chance") ]));

    var result = rules.Evaluate(["82% increased Spell Critical Strike Chance"]);
    True(result.IsMatch);
    Equal("spell", result.MatchedGroup?.Name);
    False(rules.Evaluate(["179% increased Physical Damage"]).IsMatch);
}

static void SeveralAcceptableGroupsPreserveOrEvidence()
{
    var rules = Compile(
        new AcceptableResultGroup(
            "impossible physical pair",
            ResultGroupMode.All,
            [Condition("physical", "#% increased Physical Damage", NumericConstraint.AtLeast(170m)),
             Condition("speed", "#% increased Attack Speed", NumericConstraint.AtLeast(25m))]),
        new AcceptableResultGroup(
            "spell critical pair",
            ResultGroupMode.All,
            [Condition("spell crit", "#% increased Spell Critical Strike Chance"),
             Condition("spell multi", "+#% to Critical Strike Multiplier for Spells")]),
        new AcceptableResultGroup(
            "single life fallback",
            ResultGroupMode.Any,
            [Condition("life", "+# to maximum Life", NumericConstraint.AtLeast(70m))]));

    var spellResult = rules.Evaluate([
        "82% increased Spell Critical Strike Chance",
        "+30% to Critical Strike Multiplier for Spells"]);
    True(spellResult.IsMatch);
    Equal("spell critical pair", spellResult.MatchedGroupId);
    Equal(3, spellResult.Groups.Count);
    False(spellResult.Groups[0].IsMatch);
    True(spellResult.Groups[1].IsMatch);
    False(spellResult.Groups[2].IsMatch);

    var fallbackResult = rules.Evaluate(["+70 to maximum Life"]);
    True(fallbackResult.IsMatch);
    Equal("single life fallback", fallbackResult.MatchedGroupId);
    False(rules.Evaluate(["169% increased Physical Damage", "24% increased Attack Speed"]).IsMatch);
}

static void AnyModeRequiresOneCondition()
{
    var rules = Compile(new AcceptableResultGroup(
        "any",
        ResultGroupMode.Any,
        [Condition("life", "+# to maximum Life"), Condition("mana", "+# to maximum Mana")]));

    True(rules.Evaluate(["+70 to maximum Life"]).IsMatch);
    False(rules.Evaluate(["+# to maximum Energy Shield"]).IsMatch);
}

static void AllModeRequiresEveryCondition()
{
    var rules = Compile(new AcceptableResultGroup(
        "all",
        ResultGroupMode.All,
        [Condition("life", "+# to maximum Life"), Condition("mana", "+# to maximum Mana")]));

    False(rules.Evaluate(["+70 to maximum Life"]).IsMatch);
    True(rules.Evaluate(["+70 to maximum Life", "+60 to maximum Mana"]).IsMatch);
}

static void AtLeastModeHonorsItsThreshold()
{
    var rules = Compile(new AcceptableResultGroup(
        "two-of-three",
        ResultGroupMode.AtLeast,
        [Condition("global-crit", "#% increased Global Critical Strike Chance"),
         Condition("spell-crit", "#% increased Spell Critical Strike Chance"),
         Condition("attack-speed", "#% increased Attack Speed")],
        threshold: 2));

    False(rules.Evaluate(["25% increased Attack Speed"]).IsMatch);
    True(rules.Evaluate([
        "25% increased Attack Speed",
        "82% increased Spell Critical Strike Chance"]).IsMatch);
}

static void OneObservedAffixCannotSatisfyTwoConditions()
{
    var rules = Compile(new AcceptableResultGroup(
        "duplicates",
        ResultGroupMode.AtLeast,
        [Condition("life-a", "+# to maximum Life"), Condition("life-b", "+# to maximum Life")],
        threshold: 2));

    var result = rules.Evaluate(["+70 to maximum Life"]);
    False(result.IsMatch);
    Equal(1, result.Groups.Single().MatchedCount);
}

static void SeparateIdenticalAffixesCanSatisfyDuplicateConditions()
{
    var rules = Compile(new AcceptableResultGroup(
        "duplicates",
        ResultGroupMode.All,
        [Condition("regen-a", "Regenerate # Life per second"),
         Condition("regen-b", "Regenerate # Life per second")]));

    var result = rules.Evaluate([
        "Regenerate 7 Life per second",
        "Regenerate 9 Life per second"]);
    True(result.IsMatch);
    Equal(2, result.Groups.Single().MatchedCount);
}

static void SpecificNumericConditionReceivesQualifyingObservation()
{
    var rules = Compile(new AcceptableResultGroup(
        "specific-first",
        ResultGroupMode.All,
        [Condition("any-roll", "#% increased Physical Damage"),
         Condition("t1-roll", "#% increased Physical Damage", NumericConstraint.AtLeast(170m))]));

    var result = rules.Evaluate([
        "179% increased Physical Damage",
        "100% increased Physical Damage"]);
    True(result.IsMatch);
    var evidence = result.Groups.Single().Conditions;
    Equal(100m, evidence.Single(condition => condition.Name == "any-roll").ActualValues.Single());
    Equal(179m, evidence.Single(condition => condition.Name == "t1-roll").ActualValues.Single());
}

static void OverlappingConditionsRequireCompleteOneToOneAssignment()
{
    var rules = Compile(new AcceptableResultGroup(
        "constrained assignment",
        ResultGroupMode.All,
        [Condition("any roll", "#% increased Physical Damage"),
         Condition("T1 roll", "#% increased Physical Damage", NumericConstraint.AtLeast(170m)),
         Condition("perfect roll", "#% increased Physical Damage", NumericConstraint.Exactly(179m))]));

    var onlyTwoObservations = rules.Evaluate([
        "179% increased Physical Damage",
        "170% increased Physical Damage"]);
    False(onlyTwoObservations.IsMatch);
    Equal(2, onlyTwoObservations.Groups.Single().MatchedCount);

    var completeAssignment = rules.Evaluate([
        "179% increased Physical Damage",
        "170% increased Physical Damage",
        "100% increased Physical Damage"]);
    True(completeAssignment.IsMatch);
    var conditions = completeAssignment.Groups.Single().Conditions;
    Equal(100m, conditions.Single(condition => condition.Name == "any roll").ActualValues.Single());
    Equal(170m, conditions.Single(condition => condition.Name == "T1 roll").ActualValues.Single());
    Equal(179m, conditions.Single(condition => condition.Name == "perfect roll").ActualValues.Single());
}

static void OverlappingPhysicalSpansCannotCountOneWrappedModifierTwice()
{
    var rules = Compile(new AcceptableResultGroup(
        "overlapping spans",
        ResultGroupMode.All,
        [Condition("short affix", "#% increased Attack Speed"),
         Condition(
             "wrapped affix",
             "#% increased Attack Speed if you've dealt a Critical Strike Recently")]));

    var oneWrappedModifier = rules.Evaluate([
        "8% increased Attack Speed",
        "if you've dealt a Critical Strike Recently"]);
    False(oneWrappedModifier.IsMatch);
    Equal(1, oneWrappedModifier.Groups.Single().MatchedCount);

    var twoSeparateModifiers = rules.Evaluate([
        "8% increased Attack Speed",
        "if you've dealt a Critical Strike Recently",
        "",
        "7% increased Attack Speed"]);
    True(twoSeparateModifiers.IsMatch);
    var observations = twoSeparateModifiers.Groups.Single().Conditions
        .Select(condition => condition.Observation)
        .ToArray();
    True(observations.All(observation => observation is not null));
    False(observations[0]!.StartLineIndex == observations[1]!.StartLineIndex);
}

static void AssistedTranscriptsParticipateWithoutLosingPhysicalIdentity()
{
    var rules = OneCondition(
        "#% increased Physical Damage",
        NumericConstraint.AtLeast(170m));

    var result = rules.Evaluate(
        ["unrelated primary OCR"],
        [new AssistedModifierObservation(
            "band-1",
            "179% increased Physical Damage",
            "#% increased Physical Damage")],
        [new PhysicalLineIdentity(0, "band-0")]);

    True(result.IsMatch);
    Equal("band-1", result.MatchedGroup?.Conditions.Single().Observation?.PhysicalBandId);
}

static void PrimaryAndAssistedAlternativesFromOneBandCountOnce()
{
    var rules = Compile(new AcceptableResultGroup(
        "same physical modifier",
        ResultGroupMode.All,
        [Condition("primary wording", "#% increased Physical Damage"),
         Condition("assisted wording", "#% increased Attack Speed")]));

    var result = rules.Evaluate(
        ["179% increased Physical Damage"],
        [new AssistedModifierObservation(
            "band-1",
            "27% increased Attack Speed",
            "#% increased Attack Speed")],
        [new PhysicalLineIdentity(0, "band-1")]);

    False(result.IsMatch);
    Equal(1, result.Groups.Single().MatchedCount);
}

static void WrappedAssistedBandsCollapseTransitively()
{
    var rules = Compile(new AcceptableResultGroup(
        "one wrapped physical modifier",
        ResultGroupMode.All,
        [Condition("primary first row", "#% increased Physical Damage"),
         Condition("assisted wrapped", "#% increased Attack Speed Recently"),
         Condition("primary second row", "+# to maximum Life")]));

    var result = rules.Evaluate(
        ["179% increased Physical Damage", "+70 to maximum Life"],
        [new AssistedModifierObservation(
            "wrapped-band",
            "27% increased Attack Sped Recently",
            "#% increased Attack Speed Recently",
            ["band-a", "band-b"])],
        [new PhysicalLineIdentity(0, "band-a"), new PhysicalLineIdentity(1, "band-b")]);

    False(result.IsMatch);
    Equal(1, result.Groups.Single().MatchedCount);
}

static void WrappedAssistedAlternativesUnionEverySourceBand()
{
    var rules = Compile(new AcceptableResultGroup(
        "wrapped identity",
        ResultGroupMode.AtLeast,
        [Condition("first primary", "First half"),
         Condition("second primary", "Second half"),
         Condition("assisted whole", "Whole recovered modifier")],
        threshold: 2));

    var result = rules.Evaluate(
        ["First half", "Second half"],
        [new AssistedModifierObservation(
            "wrapped-band",
            "Whole rec0vered modifier",
            "Whole recovered modifier",
            ["row-a", "row-b"])],
        [new PhysicalLineIdentity(0, "row-a"),
         new PhysicalLineIdentity(1, "row-b")]);

    False(result.IsMatch);
    Equal(1, result.Groups.Single().MatchedCount);
}

static void WrappedAffixesAreOneLogicalObservation()
{
    var rules = OneCondition("#% increased Attack Speed if you've dealt a Critical Strike Recently");
    var result = rules.Evaluate([
        "8% increased Attack Speed if you've dealt",
        "a Critical Strike Recently"]);

    True(result.IsMatch);
    var evidence = result.Groups.Single().Conditions.Single();
    Equal(0, evidence.Observation?.StartLineIndex);
    Equal(2, evidence.Observation?.PhysicalLineCount);
}

static void BlankLinesRemainLogicalBoundaries()
{
    var rules = OneCondition("#% increased Attack Speed Recently");
    False(rules.Evaluate(["8% increased Attack", "", "Speed Recently"]).IsMatch);
}

static void SpecificAffixSemanticsStayDistinct()
{
    var rules = Compile(new AcceptableResultGroup(
        "specific",
        ResultGroupMode.Any,
        [Condition("attack", "#% increased Attack Critical Strike Chance"),
         Condition("spell", "#% increased Spell Critical Strike Chance"),
         Condition("global", "#% increased Global Critical Strike Chance")]));

    var result = rules.Evaluate(["82% increased Spell Critical Strike Chance"]);
    True(result.IsMatch);
    True(result.Groups.Single().Conditions.Single(condition => condition.Name == "spell").IsMatched);
    False(result.Groups.Single().Conditions.Single(condition => condition.Name == "attack").IsMatched);
    False(result.Groups.Single().Conditions.Single(condition => condition.Name == "global").IsMatched);
    False(rules.Evaluate(["82% increased Spell Critical Strike Damage"]).IsMatch);
}

static void InvalidRulesAreRejectedWithValidationDetails()
{
    var exception = Throws<RuleValidationException>(() => RuleCompiler.Compile(
        new RuleSetDefinition(
            "invalid",
            [new AcceptableResultGroup(
                "bad-threshold",
                ResultGroupMode.AtLeast,
                [Condition("only", "+# to maximum Life")],
                threshold: 2)])));
    True(exception.Errors.Count > 0, "Expected actionable validation errors.");

    Throws<RuleValidationException>(() => RuleCompiler.Compile(
        new RuleSetDefinition(
            "invalid-range",
            [new AcceptableResultGroup(
                "range",
                ResultGroupMode.Any,
                [Condition("range", "+# to maximum Life", NumericConstraint.Range(80m, 70m))])])));

    Throws<RuleValidationException>(() => RuleCompiler.Compile(
        new RuleSetDefinition(
            "empty-result",
            [new AcceptableResultGroup(
                "empty",
                ResultGroupMode.Any,
                [])])));
}

static void EvaluationExplainsMatchesAndNumericFailures()
{
    var rules = OneCondition(
        "#% increased Physical Damage",
        NumericConstraint.Range(170m, 179m));

    var hit = rules.Evaluate(["179% increased Physical Damage"]);
    var hitEvidence = hit.Groups.Single().Conditions.Single();
    True(hit.IsMatch);
    True(hitEvidence.TextMatched);
    True(hitEvidence.NumericConstraintsMatched);
    True(hitEvidence.IsMatch);
    Equal("179% increased Physical Damage", hitEvidence.ObservedText);
    Equal(179m, hitEvidence.ActualValues.Single());
    True(hitEvidence.NumericSlots.Single().IsMatch);

    var miss = rules.Evaluate(["169% increased Physical Damage"]);
    var missEvidence = miss.Groups.Single().Conditions.Single();
    False(miss.IsMatch);
    True(missEvidence.TextMatched);
    False(missEvidence.NumericConstraintsMatched);
    False(missEvidence.IsMatch);
    True(!string.IsNullOrWhiteSpace(missEvidence.FailureReason));
    Equal(169m, missEvidence.ActualValues.Single());
    False(missEvidence.NumericSlots.Single().IsMatch);
}

static void TwentyConditionEvaluationRemainsFast()
{
    var conditions = Enumerable.Range(0, 20)
        .Select(index => Condition(
            $"condition-{index}",
            $"+# to maximum TestStat{index}",
            NumericConstraint.AtLeast(index)))
        .ToArray();
    var rules = Compile(new AcceptableResultGroup(
        "twenty",
        ResultGroupMode.AtLeast,
        conditions,
        threshold: 6));
    // A rare 20-condition profile is evaluated against a realistic six-affix tooltip.
    // The matcher still evaluates the complete compiled condition set on every call.
    var lines = Enumerable.Range(0, 6)
        .Select(index => $"+{index + 100} to maximum TestStat{index}")
        .ToArray();

    for (var index = 0; index < 200; index++)
    {
        True(rules.Evaluate(lines).IsMatch);
    }

    const int measuredEvaluations = 2_000;
    var matched = 0;
    var started = Stopwatch.GetTimestamp();
    for (var index = 0; index < measuredEvaluations; index++)
    {
        var result = rules.Evaluate(lines);
        matched += result.IsMatch ? 1 : 0;
    }

    var perEvaluation = Stopwatch.GetElapsedTime(started).TotalMilliseconds / measuredEvaluations;
    Equal(measuredEvaluations, matched);
    Console.WriteLine($"      warm 20-condition evaluator: {perEvaluation:F3} ms/evaluation");
    True(
        perEvaluation < 1.0,
        $"Expected < 1 ms/evaluation, observed {perEvaluation:F3} ms.");
}

static void DenseDuplicateAssignmentRemainsBounded()
{
    var conditions = Enumerable.Range(0, RuleSetDefinition.MaximumConditions)
        .Select(index => Condition($"duplicate-{index}", "+# to maximum Life"))
        .ToArray();
    var rules = Compile(new AcceptableResultGroup(
        "dense duplicates",
        ResultGroupMode.All,
        conditions));
    var lines = Enumerable.Range(0, RuleSetDefinition.MaximumConditions - 1)
        .Select(index => $"+{100 + index} to maximum Life")
        .ToArray();

    var started = Stopwatch.GetTimestamp();
    var result = rules.Evaluate(lines);
    var elapsed = Stopwatch.GetElapsedTime(started);
    Console.WriteLine($"      dense 32x31 assignment: {elapsed.TotalMilliseconds:F3} ms");

    False(result.IsMatch);
    Equal(RuleSetDefinition.MaximumConditions - 1, result.Groups.Single().MatchedCount);
    True(
        elapsed < TimeSpan.FromMilliseconds(100),
        $"Dense assignment took {elapsed.TotalMilliseconds:F3} ms.");
}

static CompiledRuleSet OneCondition(string template, params NumericConstraint[] constraints) =>
    Compile(new AcceptableResultGroup(
        "result",
        ResultGroupMode.Any,
        [Condition("target", template, constraints)]));

static AffixCondition Condition(
    string id,
    string template,
    params NumericConstraint[] constraints) =>
    new(id, template, constraints);

static CompiledRuleSet Compile(params AcceptableResultGroup[] groups) =>
    RuleCompiler.Compile(new RuleSetDefinition("test rules", groups));

static void AssertInvalidJsonDefinition(string json, string expectedErrorFragment)
{
    var definition = JsonSerializer.Deserialize<RuleSetDefinition>(json);
    NotNull(definition);
    ValidationContains(definition!, expectedErrorFragment);
}

static void ValidationContains(RuleSetDefinition definition, string expectedErrorFragment)
{
    var exception = Throws<RuleValidationException>(() => RuleCompiler.Compile(definition));
    True(
        exception.Errors.Any(error =>
            error.Contains(expectedErrorFragment, StringComparison.OrdinalIgnoreCase)),
        $"Expected a validation error containing '{expectedErrorFragment}', got: " +
        string.Join(" | ", exception.Errors));
}

static TException Throws<TException>(Action action)
    where TException : Exception
{
    try
    {
        action();
    }
    catch (TException exception)
    {
        return exception;
    }

    throw new InvalidOperationException($"Expected {typeof(TException).Name}.");
}

static void True(bool value, string? message = null)
{
    if (!value)
    {
        throw new InvalidOperationException(message ?? "Expected true, got false.");
    }
}

static void False(bool value, string? message = null) => True(!value, message ?? "Expected false, got true.");

static void NotNull<T>(T? value)
{
    if (value is null)
    {
        throw new InvalidOperationException("Expected a non-null value.");
    }
}

static void Equal<T>(T expected, T actual)
{
    if (!EqualityComparer<T>.Default.Equals(expected, actual))
    {
        throw new InvalidOperationException($"Expected '{expected}', got '{actual}'.");
    }
}

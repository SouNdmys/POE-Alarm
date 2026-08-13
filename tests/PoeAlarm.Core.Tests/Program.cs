using PoeAlarm.Core.Matching;

var tests = new (string Name, Action Run)[]
{
    ("PoEDB percentage range canonicalization", PoedbPercentageRangeCanonicalization),
    ("Numeric render variants match", NumericRenderVariantsMatch),
    ("Plain numeric ranges match", PlainNumericRangesMatch),
    ("Presentation differences normalize", PresentationDifferencesNormalize),
    ("Full-width NFKC normalization", FullWidthNfkcNormalization),
    ("OCR glyph confusions are tolerated", OcrGlyphConfusionsAreTolerated),
    ("Four-letter OCR glyph confusions are tolerated", FourLetterOcrGlyphConfusionsAreTolerated),
    ("Missing OCR word boundaries are recovered", MissingOcrWordBoundariesAreRecovered),
    ("Boundary digit noise is narrowly recovered", BoundaryDigitNoiseIsNarrowlyRecovered),
    ("Merged OCR recovery preserves full semantics", MergedOcrRecoveryPreservesFullSemantics),
    ("Merged OCR recovery preserves numeric structure", MergedOcrRecoveryPreservesNumericStructure),
    ("In-word hyphens may disappear in OCR", InWordHyphensMayDisappearInOcr),
    ("Wrapped logical affix matches", WrappedLogicalAffixMatches),
    ("One through four lines are supported", OneThroughFourLinesAreSupported),
    ("Blank line is a logical boundary", BlankLineIsALogicalBoundary),
    ("Substring and additional words are rejected", SubstringAndAdditionalWordsAreRejected),
    ("Attack and Cast are distinct", () => AssertSemanticDifference("Attack", "Cast")),
    ("Dagger and Claw are distinct", () => AssertSemanticDifference("Dagger", "Claw")),
    ("Cold and Fire are distinct", () => AssertSemanticDifference("Cold", "Fire")),
    ("dealt and killed are distinct", () => AssertSemanticDifference("dealt", "killed")),
    ("you've and haven't are distinct", () => AssertSemanticDifference("you've", "haven't")),
    ("increased and reduced are distinct", () => AssertSemanticDifference("increased", "reduced")),
    ("Percent and plain numbers are distinct", PercentAndPlainNumbersAreDistinct),
    ("Negative and non-negative slots are distinct", NegativeAndNonNegativeSlotsAreDistinct),
    ("Values and tiers are intentionally ignored", ValuesAndTiersAreIgnored),
    ("POE2 standalone and rolled values are ignored", Poe2ValuesAreIgnored),
    ("POE2 profile supports up to eight wrapped lines", Poe2EightLineTargetsAreSupported),
    ("Invalid line spans are rejected", InvalidLineSpansAreRejected),
    ("Traditional Chinese numeric slots normalize", TraditionalChineseNumericSlotsNormalize),
    ("Traditional Chinese OCR spaces and wrapping normalize", TraditionalChineseSpacingAndWrappingNormalize),
    ("Traditional Chinese full semantics stay strict", TraditionalChineseFullSemanticsStayStrict),
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

Console.WriteLine($"{tests.Length - failures.Count}/{tests.Length} matcher tests passed.");
if (failures.Count > 0)
{
    Environment.ExitCode = 1;
}

static void PoedbPercentageRangeCanonicalization()
{
    var canonical = AffixCanonicalizer.Normalize(
        "(6—8)% increased Attack Speed if you've dealt a Critical Strike Recently");

    Equal(
        "<PCT> increased attack speed if you've dealt a critical strike recently",
        canonical.Text);
}

static void NumericRenderVariantsMatch()
{
    var matcher = new FullLineAffixMatcher(
        "(6—8)% increased Attack Speed if you've dealt a Critical Strike Recently");

    True(matcher.IsMatch("#% increased Attack Speed if you've dealt a Critical Strike Recently"));
    True(matcher.IsMatch("8% increased Attack Speed if you've dealt a Critical Strike Recently"));
    True(matcher.IsMatch("(6-8)% increased Attack Speed if you've dealt a Critical Strike Recently"));
    True(matcher.IsMatch("8(6-8)% increased Attack Speed if you've dealt a Critical Strike Recently"));
}

static void PlainNumericRangesMatch()
{
    var matcher = new FullLineAffixMatcher(
        "(4—5) to (9—10) Added Cold Damage with Dagger Attacks");

    Equal("<NUM> to <NUM> added cold damage with dagger attacks", matcher.Template.Text);
    True(matcher.IsMatch("# to # Added Cold Damage with Dagger Attacks"));
    True(matcher.IsMatch("5 to 10 Added Cold Damage with Dagger Attacks"));
    True(matcher.IsMatch("5(4-5) to 10(9-10) Added Cold Damage with Dagger Attacks"));
}

static void PresentationDifferencesNormalize()
{
    var matcher = new FullLineAffixMatcher(
        "(6-8)% INCREASED ATTACK SPEED IF YOU’VE DEALT A CRITICAL STRIKE RECENTLY");

    True(matcher.IsMatch("  8%  increased Attack Speed\r\nif you've dealt a Critical Strike Recently. "));
}

static void FullWidthNfkcNormalization()
{
    var matcher = new FullLineAffixMatcher("#% increased attack speed");
    True(matcher.IsMatch("８％ ＩＮＣＲＥＡＳＥＤ ＡＴＴＡＣＫ ＳＰＥＥＤ"));
}

static void OcrGlyphConfusionsAreTolerated()
{
    var matcher = new FullLineAffixMatcher(
        "#% increased Attack Speed if you've dealt a Critical Strike Recently");

    True(matcher.IsMatch(
        "8% increased Attack Speed if you've dealt a CRIT1CAL STRlKE RECENTIY"));
}

static void FourLetterOcrGlyphConfusionsAreTolerated()
{
    True(new FullLineAffixMatcher("+# to maximum Life").IsMatch("+70 to maximum L1fe"));
    True(new FullLineAffixMatcher("Adds # Fire Damage").IsMatch("Adds 8 Flre Damage"));
    True(new FullLineAffixMatcher("on Kill").IsMatch("on Ki11"));
}

static void MissingOcrWordBoundariesAreRecovered()
{
    var matcher = new FullLineAffixMatcher(
        "1 to (19-20) Added Lightning Damage with Dagger Attacks");

    True(matcher.IsMatch(
        "1(1-2) TO 24(23-24) ADDED LIGHTNING DAMAGEWITHDAGGERATTACKS"));
    True(matcher.IsMatch(
        "1(1-2) TO 24(23-24) ADDEDLIGHTNINGDAMAGEWITHDAGGERATTACKS"));
}

static void BoundaryDigitNoiseIsNarrowlyRecovered()
{
    var matcher = new FullLineAffixMatcher(
        "1 to (19-20) Added Lightning Damage with Dagger Attacks");

    True(matcher.IsMatch(
        "1(1-2) TO 24(23-24) ADDED LIGHTNING DAMAGEWITH2DAGGERATTACKS"));
    False(matcher.IsMatch(
        "1(1-2) TO 24(23-24) ADDED LIGHTNING DAMAG2EWITHDAGGERATTACKS"));
    False(matcher.IsMatch(
        "1(1-2) TO 24(23-24) ADDED2LIGHTNINGDAMAGEWITH2DAGGERATTACKS"));
    False(matcher.IsMatch(
        "1(1-2) TO 24(23-24) ADDED LIGHTNING DAMAGE WITH 2 DAGGER ATTACKS"));
}

static void MergedOcrRecoveryPreservesFullSemantics()
{
    var matcher = new FullLineAffixMatcher(
        "1 to (19-20) Added Lightning Damage with Dagger Attacks");

    False(matcher.IsMatch(
        "1(1-2) TO 24(23-24) ADDED LIGHTNING DAMAGEWITH2CLAWATTACKS"));
    False(matcher.IsMatch(
        "1(1-2) TO 24(23-24) ADDED LIGHTNING DAMAGEWITH2DAGGERCASTS"));
    False(matcher.IsMatch(
        "1(1-2) TO 24(23-24) ADDED LIGHTNING DAMAGEWITHTWODAGGERATTACKS"));
}

static void MergedOcrRecoveryPreservesNumericStructure()
{
    var matcher = new FullLineAffixMatcher(
        "1 to (19-20) Added Lightning Damage with Dagger Attacks");

    False(matcher.IsMatch(
        "24(23-24) ADDED LIGHTNING DAMAGEWITH2DAGGERATTACKS"));
}

static void InWordHyphensMayDisappearInOcr()
{
    var matcher = new FullLineAffixMatcher("#% reduced Mana Cost of Non-Channelling Skills");
    True(matcher.IsMatch("7% reduced Mana Cost of NonChannelling Skills"));
}

static void WrappedLogicalAffixMatches()
{
    var matcher = new FullLineAffixMatcher(
        "#% increased Attack Speed if you've dealt a Critical Strike Recently");
    string[] lines =
    [
        "Adds 10 to 17 Cold Damage to Attacks",
        "8(6-8)% increased Attack Speed if you've dealt",
        "a Critical Strike Recently",
        "5% increased Attack Speed",
    ];

    True(matcher.TryFindMatch(lines, out var match));
    NotNull(match);
    Equal(1, match!.StartLineIndex);
    Equal(2, match.PhysicalLineCount);
}

static void OneThroughFourLinesAreSupported()
{
    var matcher = new FullLineAffixMatcher("#% increased Attack Speed Recently");
    string[] lines = ["8% increased", "Attack", "Speed", "Recently"];

    True(matcher.TryFindMatch(lines, out var match));
    Equal(4, match!.PhysicalLineCount);
}

static void BlankLineIsALogicalBoundary()
{
    var matcher = new FullLineAffixMatcher("#% increased Attack Speed Recently");
    string[] lines = ["8% increased Attack", "", "Speed Recently"];

    False(matcher.TryFindMatch(lines, out _));
}

static void SubstringAndAdditionalWordsAreRejected()
{
    var matcher = new FullLineAffixMatcher("#% increased Attack Speed");

    False(matcher.IsMatch("prefix 8% increased Attack Speed"));
    False(matcher.IsMatch("8% increased Attack Speed Recently"));
    False(matcher.IsMatch("increased Attack"));
}

static void AssertSemanticDifference(string expectedWord, string otherWord)
{
    var matcher = new FullLineAffixMatcher($"#% {expectedWord} speed recently");
    False(matcher.IsMatch($"8% {otherWord} speed recently"));
}

static void PercentAndPlainNumbersAreDistinct()
{
    var matcher = new FullLineAffixMatcher("#% increased Attack Speed");
    False(matcher.IsMatch("8 increased Attack Speed"));
}

static void NegativeAndNonNegativeSlotsAreDistinct()
{
    var matcher = new FullLineAffixMatcher("+#% to Fire Resistance");
    True(matcher.IsMatch("35% to Fire Resistance"));
    False(matcher.IsMatch("-35% to Fire Resistance"));
}

static void ValuesAndTiersAreIgnored()
{
    var matcher = new FullLineAffixMatcher("+# to Armour and # Life per second");
    True(matcher.IsMatch("+159 to Armour and 3.5 Life per second"));
}

static void Poe2ValuesAreIgnored()
{
    var projectiles = new FullLineAffixMatcher(
        "Monsters fire 2 additional Projectiles");
    True(projectiles.IsMatch("Monsters fire 2 additional Projectiles"));
    True(projectiles.IsMatch("Monsters fire 3 additional Projectiles"));

    var reward = new FullLineAffixMatcher(
        "20% more Waystones found in Area");
    True(reward.IsMatch("20% more Waystones found in Area"));
    True(reward.IsMatch("25% more Waystones found in Area"));

    var mixed = new FullLineAffixMatcher(
        "(10-15)% increased Monster Damage while Monsters fire 2 additional Projectiles");
    True(mixed.IsMatch(
        "13% increased Monster Damage while Monsters fire 2 additional Projectiles"));
    True(mixed.IsMatch(
        "13% increased Monster Damage while Monsters fire 3 additional Projectiles"));
}

static void Poe2EightLineTargetsAreSupported()
{
    var matcher = new FullLineAffixMatcher(
        "Monsters deal increased Damage and fire additional Projectiles while the Area contains dangerous terrain",
        maximumPhysicalLineSpan: 8);
    string[] lines =
    [
        "Monsters deal",
        "increased Damage",
        "and fire",
        "additional Projectiles",
        "while the Area",
        "contains",
        "dangerous",
        "terrain",
    ];

    True(matcher.TryFindMatch(lines, out var match));
    Equal(8, match!.PhysicalLineCount);
}

static void InvalidLineSpansAreRejected()
{
    var matcher = new FullLineAffixMatcher("#% increased Attack Speed");
    Throws<ArgumentOutOfRangeException>(() => matcher.TryFindMatch(["8% increased Attack Speed"], out _, 9));
}

static void TraditionalChineseNumericSlotsNormalize()
{
    var matcher = new FullLineAffixMatcher("若你近期有暴擊，增加 (6—8)% 攻擊速度");

    True(matcher.IsMatch("若你近期有暴擊，增加8%攻擊速度"));
    True(matcher.IsMatch("若你近期有暴擊，增加 7％ 攻擊速度"));
    True(matcher.IsMatch("若你近期有暴擊·增加8%攻擊速度"));

    var flatValueMatcher = new FullLineAffixMatcher("增加 (10—20) 點護甲");
    True(flatValueMatcher.IsMatch("增加17點護甲"));

    var decimalMatcher = new FullLineAffixMatcher("+(3.11—3.8)%暴擊率");
    True(decimalMatcher.IsMatch("+ 3 · 73 % 暴 擊 率"));
    True(decimalMatcher.IsMatch("+3 ∙ 73%暴擊率"));

    var listSeparatorMatcher = new FullLineAffixMatcher("增加 (3—4) 點力量和 (73—74) 點敏捷");
    True(listSeparatorMatcher.IsMatch("增加 3・73 點力量和敏捷") is false);
}

static void TraditionalChineseSpacingAndWrappingNormalize()
{
    var matcher = new FullLineAffixMatcher("若你近期有暴擊，增加 (6—8)% 攻擊速度");

    True(matcher.IsMatch("若 你 近 期 有 暴 擊，增 加 8% 攻 擊 速 度"));
    True(matcher.TryFindMatch(
        ["若你近期有暴擊，增加8%", "攻擊速度"],
        out var wrapped));
    Equal(2, wrapped!.PhysicalLineCount);
}

static void TraditionalChineseFullSemanticsStayStrict()
{
    var matcher = new FullLineAffixMatcher("若你近期有暴擊，增加 (6—8)% 攻擊速度");

    False(matcher.IsMatch("若你近期有擊殺，增加8%攻擊速度"));
    False(matcher.IsMatch("若你近期有暴擊，增加8%施放速度"));
    False(matcher.IsMatch("若你近期有暴擊，增加8%攻擊傷害"));
}

static void True(bool condition)
{
    if (!condition)
    {
        throw new InvalidOperationException("Expected true, but was false.");
    }
}

static void False(bool condition) => True(!condition);

static void Equal<T>(T expected, T actual)
    where T : notnull
{
    if (!EqualityComparer<T>.Default.Equals(expected, actual))
    {
        throw new InvalidOperationException($"Expected '{expected}', but got '{actual}'.");
    }
}

static void NotNull(object? value)
{
    if (value is null)
    {
        throw new InvalidOperationException("Expected a value, but got null.");
    }
}

static void Throws<TException>(Action action)
    where TException : Exception
{
    try
    {
        action();
    }
    catch (TException)
    {
        return;
    }

    throw new InvalidOperationException($"Expected {typeof(TException).Name}.");
}

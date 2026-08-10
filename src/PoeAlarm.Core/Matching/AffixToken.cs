namespace PoeAlarm.Core.Matching;

/// <summary>
/// A token in an affix after values and harmless presentation differences are removed.
/// </summary>
public readonly record struct AffixToken(AffixTokenKind Kind, string Text)
{
    public override string ToString() => Text;
}

public enum AffixTokenKind
{
    Word,
    Number,
    Percent,
    NegativeNumber,
    NegativePercent,
}

/// <summary>
/// The stable representation used by the matcher and shown in diagnostics.
/// </summary>
public sealed class CanonicalAffix
{
    private readonly AffixToken[] _tokens;

    internal CanonicalAffix(AffixToken[] tokens)
    {
        _tokens = tokens;
        Text = string.Join(' ', tokens.Select(static token => token.Text));
    }

    public IReadOnlyList<AffixToken> Tokens => _tokens;

    public string Text { get; }

    public bool HasWords => _tokens.Any(static token => token.Kind == AffixTokenKind.Word);
}

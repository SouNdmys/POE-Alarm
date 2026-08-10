using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;

namespace PoeAlarm.Core.Matching;

/// <summary>
/// Converts a PoEDB template or an in-game OCR result into the same token stream.
/// Numeric values are deliberately discarded, while their unit (plain or percent)
/// remains part of the stream.
/// </summary>
public static class AffixCanonicalizer
{
    private const string Number = @"\d+(?:[\.,]\d+)?";

    private static readonly Regex NumericExpression = new(
        $@"(?:
                [+-]?{Number}\s*\(\s*[+-]?{Number}\s*-\s*[+-]?{Number}\s*\)
              | \(\s*[+-]?{Number}\s*-\s*[+-]?{Number}\s*\)
              | [+-]?{Number}\s*-\s*[+-]?{Number}
              | [+-]?\s*\#
              | [+-]?{Number}
            )
            \s*(?<percent>%)?",
        RegexOptions.Compiled | RegexOptions.CultureInvariant | RegexOptions.IgnorePatternWhitespace);

    public static CanonicalAffix Normalize(string? text)
    {
        if (string.IsNullOrWhiteSpace(text))
        {
            return new CanonicalAffix([]);
        }

        var normalized = NormalizePresentation(text);
        var tokens = new List<AffixToken>();

        for (var index = 0; index < normalized.Length;)
        {
            var current = normalized[index];
            if (char.IsWhiteSpace(current))
            {
                index++;
                continue;
            }

            if (IsWordStart(normalized, index))
            {
                var end = index + 1;
                while (end < normalized.Length && IsWordCharacter(normalized[end]))
                {
                    end++;
                }

                var word = normalized[index..end].Trim('\'');
                if (word.Length > 0)
                {
                    tokens.Add(new AffixToken(AffixTokenKind.Word, word));
                }

                index = end;
                continue;
            }

            var numeric = NumericExpression.Match(normalized, index);
            if (numeric.Success && numeric.Index == index)
            {
                var isPercent = numeric.Groups["percent"].Success;
                var isNegative = Regex.IsMatch(
                    numeric.Value,
                    @"^\s*\(?\s*-",
                    RegexOptions.CultureInvariant);
                var kind = (isNegative, isPercent) switch
                {
                    (true, true) => AffixTokenKind.NegativePercent,
                    (true, false) => AffixTokenKind.NegativeNumber,
                    (false, true) => AffixTokenKind.Percent,
                    _ => AffixTokenKind.Number,
                };
                var tokenText = kind switch
                {
                    AffixTokenKind.NegativePercent => "<NEG_PCT>",
                    AffixTokenKind.NegativeNumber => "<NEG_NUM>",
                    AffixTokenKind.Percent => "<PCT>",
                    _ => "<NUM>",
                };
                tokens.Add(new AffixToken(kind, tokenText));
                index += numeric.Length;
                continue;
            }

            // Styling punctuation does not identify an affix. All lexical text still
            // participates in the exact token sequence comparison.
            index++;
        }

        return new CanonicalAffix([.. tokens]);
    }

    private static string NormalizePresentation(string value)
    {
        var source = value.Normalize(NormalizationForm.FormKC);
        var builder = new StringBuilder(source.Length);

        for (var index = 0; index < source.Length; index++)
        {
            var character = source[index];
            if (IsDash(character))
            {
                var joinsLetters = index > 0 &&
                                   index + 1 < source.Length &&
                                   char.IsLetter(source[index - 1]) &&
                                   char.IsLetter(source[index + 1]);
                if (!joinsLetters)
                {
                    builder.Append('-');
                }

                continue;
            }

            builder.Append(character switch
            {
                '\u2018' or '\u2019' or '\u201B' or '\u02BC' or '\uFF07' => '\'',
                '\u00A0' or '\u2007' or '\u202F' => ' ',
                _ => char.ToLower(character, CultureInfo.InvariantCulture),
            });
        }

        return builder.ToString();
    }

    private static bool IsDash(char character) =>
        character is '-' or '\u2010' or '\u2011' or '\u2012' or '\u2013' or '\u2014' or '\u2015' or '\u2212';

    private static bool IsWordStart(string text, int index)
    {
        if (char.IsLetter(text[index]))
        {
            return true;
        }

        // OCR sometimes substitutes a digit inside a word (for example CRIT1CAL).
        // Treat an alphanumeric run containing a letter as a word, not as a value.
        if (!char.IsDigit(text[index]))
        {
            return false;
        }

        for (var cursor = index + 1; cursor < text.Length && IsWordCharacter(text[cursor]); cursor++)
        {
            if (char.IsLetter(text[cursor]))
            {
                return true;
            }
        }

        return false;
    }

    private static bool IsWordCharacter(char character) =>
        char.IsLetterOrDigit(character) || character == '\'';
}

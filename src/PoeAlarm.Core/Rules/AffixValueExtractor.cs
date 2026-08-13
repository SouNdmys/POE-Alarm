using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;

namespace PoeAlarm.Core.Rules;

/// <summary>
/// Extracts values in the same order as numeric tokens produced by the product matcher.
/// A displayed roll such as 179(170-179) yields 179; a range or # placeholder has no
/// observed magnitude and yields null.
/// </summary>
public static partial class AffixValueExtractor
{
    private const string Number = @"\d+(?:[\.,]\d+)?";

    private static readonly Regex NumericExpression = new(
        $@"(?x)
            (?:
                (?<rolled>[+-]?{Number})\s*\(\s*[+-]?{Number}\s*-\s*[+-]?{Number}\s*\)
              | \(\s*[+-]?{Number}\s*-\s*[+-]?{Number}\s*\)
              | [+-]?{Number}\s*-\s*[+-]?{Number}
              | [+-]?\s*\#
              | (?<single>[+-]?{Number})
            )
            \s*%?",
        RegexOptions.Compiled | RegexOptions.CultureInvariant);

    public static IReadOnlyList<decimal?> Extract(string? text)
    {
        if (string.IsNullOrWhiteSpace(text))
        {
            return [];
        }

        var normalized = NormalizePresentation(text);
        var values = new List<decimal?>();
        for (var index = 0; index < normalized.Length;)
        {
            var current = normalized[index];
            if (char.IsWhiteSpace(current) || IsHanIdeograph(current))
            {
                index++;
                continue;
            }

            if (IsLatinWordStart(normalized, index))
            {
                index++;
                while (index < normalized.Length && IsLatinWordCharacter(normalized[index]))
                {
                    index++;
                }

                continue;
            }

            var numeric = NumericExpression.Match(normalized, index);
            if (numeric.Success && numeric.Index == index)
            {
                var magnitude = numeric.Groups["rolled"].Success
                    ? numeric.Groups["rolled"].Value
                    : numeric.Groups["single"].Success
                        ? numeric.Groups["single"].Value
                        : null;
                values.Add(ParseMagnitude(magnitude));
                index += numeric.Length;
                continue;
            }

            index++;
        }

        return values;
    }

    private static decimal? ParseMagnitude(string? text)
    {
        if (string.IsNullOrWhiteSpace(text))
        {
            return null;
        }

        var invariant = text.Replace(',', '.');
        return decimal.TryParse(
            invariant,
            NumberStyles.AllowLeadingSign | NumberStyles.AllowDecimalPoint,
            CultureInfo.InvariantCulture,
            out var value)
            ? value
            : null;
    }

    private static string NormalizePresentation(string value)
    {
        var source = value.Normalize(NormalizationForm.FormKC);
        var builder = new StringBuilder(source.Length);
        foreach (var character in source)
        {
            builder.Append(IsDash(character) ? '-' : character);
        }

        return builder.ToString();
    }

    private static bool IsLatinWordStart(string text, int index)
    {
        if (char.IsLetter(text[index]) && !IsHanIdeograph(text[index]))
        {
            return true;
        }

        if (!char.IsDigit(text[index]))
        {
            return false;
        }

        for (var cursor = index + 1;
             cursor < text.Length && IsLatinWordCharacter(text[cursor]);
             cursor++)
        {
            if (char.IsLetter(text[cursor]) && !IsHanIdeograph(text[cursor]))
            {
                return true;
            }
        }

        return false;
    }

    private static bool IsLatinWordCharacter(char character) =>
        !IsHanIdeograph(character) &&
        (char.IsLetterOrDigit(character) || character == '\'');

    private static bool IsDash(char character) =>
        character is '-' or '\u2010' or '\u2011' or '\u2012' or '\u2013' or '\u2014' or '\u2015' or '\u2212';

    private static bool IsHanIdeograph(char character) =>
        character is >= '\u3400' and <= '\u4DBF' or
        >= '\u4E00' and <= '\u9FFF' or
        >= '\uF900' and <= '\uFAFF';
}

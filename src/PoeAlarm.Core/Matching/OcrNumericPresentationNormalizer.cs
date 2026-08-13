using System.Text;

namespace PoeAlarm.Core.Matching;

/// <summary>
/// Repairs decimal separators that OCR presents as a spaced dot-like glyph. The rewrite is
/// deliberately narrow: it only applies between two digits, so ordinary punctuation remains
/// untouched.
/// </summary>
public static class OcrNumericPresentationNormalizer
{
    public static string Normalize(string value)
    {
        var source = value.Normalize(NormalizationForm.FormKC);
        StringBuilder? builder = null;
        var copyFrom = 0;

        for (var digitIndex = 0; digitIndex < source.Length; digitIndex++)
        {
            if (!char.IsDigit(source[digitIndex]))
            {
                continue;
            }

            var separatorIndex = digitIndex + 1;
            while (separatorIndex < source.Length && char.IsWhiteSpace(source[separatorIndex]))
            {
                separatorIndex++;
            }

            if (separatorIndex >= source.Length || !IsDecimalSeparator(source[separatorIndex]))
            {
                continue;
            }

            var nextDigitIndex = separatorIndex + 1;
            while (nextDigitIndex < source.Length && char.IsWhiteSpace(source[nextDigitIndex]))
            {
                nextDigitIndex++;
            }

            if (nextDigitIndex >= source.Length || !char.IsDigit(source[nextDigitIndex]))
            {
                continue;
            }

            builder ??= new StringBuilder(source.Length);
            builder.Append(source, copyFrom, digitIndex - copyFrom + 1);
            builder.Append('.');
            copyFrom = nextDigitIndex;
            digitIndex = nextDigitIndex - 1;
        }

        if (builder is null)
        {
            return source;
        }

        builder.Append(source, copyFrom, source.Length - copyFrom);
        return builder.ToString();
    }

    private static bool IsDecimalSeparator(char character) =>
        character is '.' or ',' or
            '\u00B7' or // middle dot: Windows zh-Hant OCR commonly emits this for POE decimals
            '\u2022' or // bullet
            '\u2027' or // hyphenation point
            '\u2219' or // bullet operator
            '\u22C5' or // dot operator
            '\u3002';   // ideographic full stop
}

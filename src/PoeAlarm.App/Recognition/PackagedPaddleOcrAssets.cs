using System.IO;
using System.Reflection;
using System.Text;

namespace PoeAlarm.App.Recognition;

internal static class PackagedPaddleOcrAssets
{
    internal const string ModelResourceName = "PoeAlarm.Ocr.PP-OCRv5_mobile_rec.onnx";
    internal const string DictionaryResourceName = "PoeAlarm.Ocr.ppocrv5_dict.txt";

    public static PackagedPaddleOcrAssetData Load()
    {
        var assembly = typeof(PackagedPaddleOcrAssets).Assembly;
        using var modelStream = OpenRequiredResource(assembly, ModelResourceName);
        using var dictionaryStream = OpenRequiredResource(assembly, DictionaryResourceName);

        var modelBytes = new byte[checked((int)modelStream.Length)];
        modelStream.ReadExactly(modelBytes);
        var dictionaryBytes = new byte[checked((int)dictionaryStream.Length)];
        dictionaryStream.ReadExactly(dictionaryBytes);

        var dictionary = new List<string>();
        using var reader = new StreamReader(
            new MemoryStream(dictionaryBytes, writable: false),
            new UTF8Encoding(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true),
            detectEncodingFromByteOrderMarks: true,
            leaveOpen: false);
        string? line;
        while ((line = reader.ReadLine()) is not null)
        {
            // Dictionary entries can themselves be whitespace. Do not trim or discard lines.
            dictionary.Add(line);
        }

        if (dictionary.Count == 0)
        {
            throw new InvalidDataException("The packaged PP-OCRv5 dictionary is empty.");
        }

        return new PackagedPaddleOcrAssetData(modelBytes, dictionaryBytes, dictionary);
    }

    private static Stream OpenRequiredResource(Assembly assembly, string resourceName) =>
        assembly.GetManifestResourceStream(resourceName)
        ?? throw new FileNotFoundException(
            $"Packaged OCR resource '{resourceName}' is missing from PoeAlarm.exe.");
}

internal sealed record PackagedPaddleOcrAssetData(
    byte[] ModelBytes,
    byte[] DictionaryBytes,
    IReadOnlyList<string> Dictionary);

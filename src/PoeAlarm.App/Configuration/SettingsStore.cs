using System.IO;
using System.Text.Json;

namespace PoeAlarm.App.Configuration;

public sealed class SettingsStore
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        WriteIndented = true,
        PropertyNameCaseInsensitive = true,
    };

    private readonly string _settingsPath;
    private bool _loadedFutureSchema;

    public SettingsStore(string? settingsPath = null)
    {
        _settingsPath = settingsPath ?? Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "PoeAlarm",
            "settings.json");
    }

    public async Task<AppSettings> LoadAsync(CancellationToken cancellationToken = default)
    {
        if (!File.Exists(_settingsPath))
        {
            return new AppSettings().Normalize();
        }

        try
        {
            var json = await File.ReadAllTextAsync(_settingsPath, cancellationToken);
            using var document = JsonDocument.Parse(json);
            if (document.RootElement.TryGetProperty("SchemaVersion", out var schemaElement) &&
                schemaElement.ValueKind == JsonValueKind.Number &&
                schemaElement.TryGetInt32(out var schemaVersion) &&
                schemaVersion > AppSettings.CurrentSchemaVersion)
            {
                // Never normalize or overwrite a settings file produced by a newer app. Returning
                // safe defaults keeps this binary usable while SaveAsync remains read-only.
                _loadedFutureSchema = true;
                return new AppSettings().Normalize();
            }

            _loadedFutureSchema = false;
            var settings = JsonSerializer.Deserialize<AppSettings>(json, JsonOptions);
            return (settings ?? new AppSettings()).Normalize();
        }
        catch (JsonException)
        {
            return new AppSettings().Normalize();
        }
        catch (IOException)
        {
            return new AppSettings().Normalize();
        }
        catch (UnauthorizedAccessException)
        {
            return new AppSettings().Normalize();
        }
    }

    public async Task SaveAsync(AppSettings settings, CancellationToken cancellationToken = default)
    {
        if (_loadedFutureSchema)
        {
            throw new SettingsSchemaTooNewException();
        }

        var directory = Path.GetDirectoryName(_settingsPath)
                        ?? throw new InvalidOperationException("The settings path has no parent directory.");
        Directory.CreateDirectory(directory);

        var temporaryPath = _settingsPath + ".tmp";
        await using (var stream = File.Create(temporaryPath))
        {
            await JsonSerializer.SerializeAsync(
                stream,
                settings.Normalize(),
                JsonOptions,
                cancellationToken);
        }

        File.Move(temporaryPath, _settingsPath, overwrite: true);
    }
}

public sealed class SettingsSchemaTooNewException : IOException
{
    public SettingsSchemaTooNewException()
        : base("The settings file belongs to a newer POE Alarm version and was left unchanged.")
    {
    }
}

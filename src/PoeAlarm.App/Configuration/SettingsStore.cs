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

    /// <summary>
    /// Describes whether this process can safely write the settings file it most recently read.
    /// The UI can inspect this after <see cref="LoadAsync"/> and present an upgrade message before
    /// the first attempted save.
    /// </summary>
    public SettingsStoreCompatibility Compatibility { get; private set; } =
        SettingsStoreCompatibility.Compatible;

    /// <summary>The unsupported on-disk schema, when <see cref="IsReadOnly"/> is true.</summary>
    public int? DetectedSchemaVersion { get; private set; }

    /// <summary>
    /// True when a newer POE Alarm settings schema was detected. SaveAsync also checks the file
    /// again immediately before replacement, so callers cannot bypass the guard by skipping load.
    /// </summary>
    public bool IsReadOnly => Compatibility == SettingsStoreCompatibility.FutureSchemaReadOnly;

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
            MarkCompatible();
            return new AppSettings().Normalize();
        }

        try
        {
            var json = await File.ReadAllTextAsync(_settingsPath, cancellationToken);
            using var document = JsonDocument.Parse(json);
            if (TryGetFutureSchemaVersion(document.RootElement, out var schemaVersion))
            {
                // Never normalize or overwrite a settings file produced by a newer app. Returning
                // safe defaults keeps this binary usable while SaveAsync remains read-only.
                MarkFutureSchemaReadOnly(schemaVersion);
                return new AppSettings().Normalize();
            }

            MarkCompatible();
            var settings = JsonSerializer.Deserialize<AppSettings>(json, JsonOptions);
            return (settings ?? new AppSettings()).Normalize();
        }
        catch (JsonException)
        {
            // Preserve a previously detected future-schema lock. A transient malformed read must
            // never downgrade this instance from read-only to writable.
            if (!IsReadOnly)
            {
                MarkCompatible();
            }

            return new AppSettings().Normalize();
        }
        catch (IOException)
        {
            if (!IsReadOnly)
            {
                MarkCompatible();
            }

            return new AppSettings().Normalize();
        }
        catch (UnauthorizedAccessException)
        {
            if (!IsReadOnly)
            {
                MarkCompatible();
            }

            return new AppSettings().Normalize();
        }
    }

    public async Task SaveAsync(AppSettings settings, CancellationToken cancellationToken = default)
    {
        if (IsReadOnly)
        {
            throw CreateSchemaTooNewException();
        }

        // Saving is intentionally guarded even when this SettingsStore instance has never loaded
        // the file. This prevents a stale or newly-created application window from replacing
        // fields written by a future binary.
        await ThrowIfFutureSchemaExistsOnDiskAsync(cancellationToken);

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

        // Recheck after serialization to narrow the load/save race. A newer process that wrote a
        // future schema while the temporary file was being produced retains ownership of it.
        try
        {
            await ThrowIfFutureSchemaExistsOnDiskAsync(cancellationToken);
            File.Move(temporaryPath, _settingsPath, overwrite: true);
        }
        finally
        {
            // A future-schema race or failed atomic replace must not leave a stale temporary file
            // that could later be mistaken for a completed settings write.
            try
            {
                File.Delete(temporaryPath);
            }
            catch (IOException)
            {
                // The original settings file remains authoritative; cleanup is best effort.
            }
            catch (UnauthorizedAccessException)
            {
                // See above. Never convert cleanup failure into an overwrite attempt.
            }
        }
    }

    private async Task ThrowIfFutureSchemaExistsOnDiskAsync(CancellationToken cancellationToken)
    {
        if (!File.Exists(_settingsPath))
        {
            return;
        }

        try
        {
            var json = await File.ReadAllTextAsync(_settingsPath, cancellationToken);
            using var document = JsonDocument.Parse(json);
            if (!TryGetFutureSchemaVersion(document.RootElement, out var schemaVersion))
            {
                return;
            }

            MarkFutureSchemaReadOnly(schemaVersion);
            throw CreateSchemaTooNewException();
        }
        catch (JsonException)
        {
            // Malformed settings retain the existing recovery contract. This guard is specifically
            // for a valid document that declares a schema newer than this binary understands.
        }
    }

    private static bool TryGetFutureSchemaVersion(JsonElement root, out int schemaVersion)
    {
        schemaVersion = default;
        if (root.ValueKind != JsonValueKind.Object)
        {
            return false;
        }

        var found = false;
        foreach (var property in root.EnumerateObject())
        {
            if (!property.Name.Equals("SchemaVersion", StringComparison.OrdinalIgnoreCase) ||
                property.Value.ValueKind != JsonValueKind.Number ||
                !property.Value.TryGetInt32(out var candidate))
            {
                continue;
            }

            // JsonSerializer is case-insensitive. Treat the highest duplicate declaration as the
            // effective safety boundary so casing or duplicate properties cannot downgrade it.
            if (!found || candidate > schemaVersion)
            {
                schemaVersion = candidate;
            }

            found = true;
        }

        return found && schemaVersion > AppSettings.CurrentSchemaVersion;
    }

    private void MarkCompatible()
    {
        Compatibility = SettingsStoreCompatibility.Compatible;
        DetectedSchemaVersion = null;
    }

    private void MarkFutureSchemaReadOnly(int schemaVersion)
    {
        Compatibility = SettingsStoreCompatibility.FutureSchemaReadOnly;
        DetectedSchemaVersion = schemaVersion;
    }

    private SettingsSchemaTooNewException CreateSchemaTooNewException() =>
        new(DetectedSchemaVersion ?? AppSettings.CurrentSchemaVersion + 1);
}

public enum SettingsStoreCompatibility
{
    Compatible,
    FutureSchemaReadOnly,
}

public sealed class SettingsSchemaTooNewException : IOException
{
    public SettingsSchemaTooNewException(int detectedSchemaVersion)
        : base(
            $"The settings file uses schema {detectedSchemaVersion}, newer than the supported " +
            $"schema {AppSettings.CurrentSchemaVersion}, and was left unchanged.")
    {
        DetectedSchemaVersion = detectedSchemaVersion;
    }

    public int DetectedSchemaVersion { get; }

    public int SupportedSchemaVersion => AppSettings.CurrentSchemaVersion;
}

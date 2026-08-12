using PoeAlarm.App.Capture;
using PoeAlarm.App.Input;
using PoeAlarm.App.Monitoring;
using System.Text.Json.Serialization;

namespace PoeAlarm.App.Configuration;

public sealed record AppSettings
{
    public GameProfile SelectedGameProfile { get; init; } = GameProfile.Poe1;

    /// <summary>
    /// Per-game crafting state. This remains nullable solely so SettingsStore can distinguish an
    /// old flat settings file from the profile-aware format and migrate it without losing data.
    /// </summary>
    public GameProfileSettingsSet? Profiles { get; init; }

    public string UiLanguage { get; init; } = "zh-CN";

    /// <summary>
    /// Keeps the neutral, click-through HUD visible while monitoring is stopped so the user does
    /// not need to remember whether the next crafting round has been armed.
    /// </summary>
    public bool KeepHudVisible { get; init; } = true;

    public HudPlacement HudPlacement { get; init; } = HudPlacement.Default;

    /// <summary>
    /// Allows tutorials and normal screen recordings to include the HUD and hit alert. This is
    /// enabled by default; hiding overlays from capture remains an explicit opt-in.
    /// </summary>
    public bool AllowOverlayCapture { get; init; } = true;

    public string? CustomAlertSoundPath { get; init; }

    /// <summary>Global shortcut used from inside the game to arm the next monitoring round.</summary>
    public string StartMonitoringHotKey { get; init; } =
        global::PoeAlarm.App.Input.StartMonitoringHotKey.DefaultSettingValue;

    /// <summary>
    /// Legacy 0.x fields. They are read for a one-time POE1 migration and omitted from every new
    /// settings file. Keeping their original JSON names makes upgrades transparent.
    /// </summary>
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? TargetAffix { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public ScreenRegion? CaptureRegion { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? OcrScale { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? OcrLanguage { get; init; }

    public AppSettings Normalize()
    {
        var selectedProfile = Enum.IsDefined(SelectedGameProfile)
            ? SelectedGameProfile
            : GameProfile.Poe1;
        var profiles = Profiles is null
            ? new GameProfileSettingsSet
            {
                Poe1 = new GameProfileSettings
                {
                    TargetAffix = TargetAffix?.Trim() ?? string.Empty,
                    CaptureRegion = CaptureRegion is { IsValid: true } region ? region : null,
                    OcrLanguage = GameProfileSettings.NormalizeOcrLanguage(OcrLanguage),
                },
                Poe2 = new GameProfileSettings(),
            }
            : Profiles.Normalize();

        return this with
        {
            SelectedGameProfile = selectedProfile,
            Profiles = profiles,
            TargetAffix = null,
            CaptureRegion = null,
            OcrScale = null,
            OcrLanguage = null,
            StartMonitoringHotKey = global::PoeAlarm.App.Input.StartMonitoringHotKey.Normalize(
                StartMonitoringHotKey),
        };
    }
}

[JsonConverter(typeof(JsonStringEnumConverter))]
public enum GameProfile
{
    Poe1,
    Poe2,
}

public sealed record GameProfileSettings
{
    public string TargetAffix { get; init; } = string.Empty;

    public ScreenRegion? CaptureRegion { get; init; }

    public string OcrLanguage { get; init; } = "en";

    public GameProfileSettings Normalize() => this with
    {
        TargetAffix = TargetAffix?.Trim() ?? string.Empty,
        CaptureRegion = CaptureRegion is { IsValid: true } region ? region : null,
        OcrLanguage = NormalizeOcrLanguage(OcrLanguage),
    };

    internal static string NormalizeOcrLanguage(string? language) =>
        string.Equals(language, "zh-TW", StringComparison.OrdinalIgnoreCase)
            ? "zh-TW"
            : "en";
}

public sealed record GameProfileSettingsSet
{
    public GameProfileSettings? Poe1 { get; init; } = new();

    public GameProfileSettings? Poe2 { get; init; } = new();

    public GameProfileSettings Get(GameProfile profile) => profile == GameProfile.Poe2
        ? (Poe2 ?? new GameProfileSettings()).Normalize()
        : (Poe1 ?? new GameProfileSettings()).Normalize();

    public GameProfileSettingsSet With(GameProfile profile, GameProfileSettings settings) =>
        profile == GameProfile.Poe2
            ? this with { Poe2 = settings.Normalize() }
            : this with { Poe1 = settings.Normalize() };

    public GameProfileSettingsSet Normalize() => this with
    {
        Poe1 = (Poe1 ?? new GameProfileSettings()).Normalize(),
        Poe2 = (Poe2 ?? new GameProfileSettings()).Normalize(),
    };
}

using System.Windows.Input;

namespace PoeAlarm.App.Input;

/// <summary>Supported global shortcuts for arming the next monitoring round.</summary>
internal readonly record struct StartMonitoringHotKey(
    string SettingValue,
    ModifierKeys Modifiers,
    Key Key)
{
    public const string DefaultSettingValue = "Ctrl+Shift+F10";

    public static IReadOnlyList<StartMonitoringHotKey> Options { get; } =
    [
        new(DefaultSettingValue, ModifierKeys.Control | ModifierKeys.Shift, Key.F10),
        new("Ctrl+Alt+F10", ModifierKeys.Control | ModifierKeys.Alt, Key.F10),
        new("Alt+F10", ModifierKeys.Alt, Key.F10),
    ];

    public static StartMonitoringHotKey Parse(string? value) =>
        Options.FirstOrDefault(
            option => string.Equals(
                option.SettingValue,
                value?.Trim(),
                StringComparison.OrdinalIgnoreCase)) is { Key: not Key.None } parsed
            ? parsed
            : Options[0];

    public static string Normalize(string? value) => Parse(value).SettingValue;
}

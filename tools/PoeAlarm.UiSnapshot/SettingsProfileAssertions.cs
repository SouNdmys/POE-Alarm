using System.IO;
using System.Text.Json;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Configuration;

internal static class SettingsProfileAssertions
{
    public static async Task RunAsync()
    {
        var directory = Path.Combine(
            Path.GetTempPath(),
            "PoeAlarm.SettingsProfileAssertions",
            Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(directory);
        var settingsPath = Path.Combine(directory, "settings.json");

        try
        {
            await File.WriteAllTextAsync(
                settingsPath,
                """
                {
                  "TargetAffix": "legacy poe1 target",
                  "CaptureRegion": { "X": 12, "Y": 34, "Width": 560, "Height": 420 },
                  "OcrScale": 1,
                  "OcrLanguage": "zh-TW",
                  "UiLanguage": "en",
                  "KeepHudVisible": false
                }
                """);

            var store = new SettingsStore(settingsPath);
            var migrated = await store.LoadAsync();
            Assert(migrated.SelectedGameProfile == GameProfile.Poe1, "Legacy selected profile");
            Assert(migrated.Profiles is not null, "Legacy profiles created");
            var poe1 = migrated.Profiles!.Get(GameProfile.Poe1);
            Assert(poe1.TargetAffix == "legacy poe1 target", "Legacy POE1 target retained");
            Assert(poe1.CaptureRegion == new ScreenRegion(12, 34, 560, 420),
                "Legacy POE1 region retained");
            Assert(poe1.OcrLanguage == "zh-TW", "Legacy POE1 OCR language retained");
            var emptyPoe2 = migrated.Profiles.Get(GameProfile.Poe2);
            Assert(emptyPoe2.TargetAffix.Length == 0, "Migration does not copy target to POE2");
            Assert(emptyPoe2.CaptureRegion is null, "Migration does not copy region to POE2");
            Assert(emptyPoe2.OcrLanguage == "en", "POE2 receives a clean default language");
            Assert(
                migrated.StartMonitoringHotKey == "Ctrl+Shift+F10",
                "Legacy settings receive the default start hotkey");

            var profiles = migrated.Profiles.With(
                GameProfile.Poe2,
                new GameProfileSettings
                {
                    TargetAffix = "poe2 tablet target",
                    CaptureRegion = new ScreenRegion(700, 80, 640, 920),
                    OcrLanguage = "en",
                });
            var updated = migrated with
            {
                SelectedGameProfile = GameProfile.Poe2,
                Profiles = profiles,
                StartMonitoringHotKey = "Ctrl+Alt+F10",
            };
            await store.SaveAsync(updated);

            var reloaded = await store.LoadAsync();
            Assert(reloaded.SelectedGameProfile == GameProfile.Poe2, "Selected profile round-trip");
            Assert(
                reloaded.StartMonitoringHotKey == "Ctrl+Alt+F10",
                "Start hotkey round-trip");
            Assert(reloaded.Profiles!.Get(GameProfile.Poe1) == poe1,
                "Saving POE2 does not alter migrated POE1 settings");
            var poe2 = reloaded.Profiles.Get(GameProfile.Poe2);
            Assert(poe2.TargetAffix == "poe2 tablet target", "POE2 target round-trip");
            Assert(poe2.CaptureRegion == new ScreenRegion(700, 80, 640, 920),
                "POE2 region round-trip");

            using var document = JsonDocument.Parse(await File.ReadAllTextAsync(settingsPath));
            Assert(!document.RootElement.TryGetProperty("TargetAffix", out _),
                "Saved settings remove the legacy root TargetAffix");
            Assert(!document.RootElement.TryGetProperty("CaptureRegion", out _),
                "Saved settings remove the legacy root CaptureRegion");
            Assert(document.RootElement.TryGetProperty("Profiles", out _),
                "Saved settings use profile-aware storage");

            Console.WriteLine("Settings profile assertions: 18/18 passed");
        }
        finally
        {
            if (Directory.Exists(directory))
            {
                Directory.Delete(directory, recursive: true);
            }
        }
    }

    private static void Assert(bool condition, string description)
    {
        if (!condition)
        {
            throw new InvalidOperationException($"Settings assertion failed: {description}");
        }
    }
}

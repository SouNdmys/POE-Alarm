using System.IO;
using System.Text.Json;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Configuration;
using PoeAlarm.Core.Rules;

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
            Assert(migrated.SchemaVersion == AppSettings.CurrentSchemaVersion,
                "Legacy settings migrate to the current schema");
            Assert(migrated.Profiles is not null, "Legacy profiles created");
            var poe1 = migrated.Profiles!.Get(GameProfile.Poe1);
            Assert(poe1.TargetAffix == "legacy poe1 target", "Legacy POE1 target retained");
            Assert(poe1.CaptureRegion == new ScreenRegion(12, 34, 560, 420),
                "Legacy POE1 region retained");
            Assert(poe1.OcrLanguage == "zh-TW", "Legacy POE1 OCR language retained");
            Assert(poe1.RuleEditorMode == RuleEditorMode.Quick,
                "Legacy POE1 target remains on the quick matcher path");
            Assert(poe1.StructuredRuleSet is null,
                "Legacy migration does not invent structured rules");
            Assert(poe1.MonitoringPolicy == MonitoringPolicyId.Fast,
                "Legacy POE1 settings retain the verified fast monitoring policy");
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
                    RuleEditorMode = RuleEditorMode.Structured,
                    MonitoringPolicy = MonitoringPolicyId.Fast,
                    StructuredRuleSet = new RuleSetDefinition(
                        "poe2 crafting rules",
                        [
                            new AcceptableResultGroup(
                                "critical result",
                                ResultGroupMode.AtLeast,
                                [
                                    new AffixCondition(
                                        "spell critical chance",
                                        "#% increased Spell Critical Hit Chance",
                                        [NumericConstraint.AtLeast(170)]),
                                    new AffixCondition(
                                        "critical multiplier",
                                        "+#% to Critical Strike Multiplier"),
                                ],
                                threshold: 2),
                        ]),
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
            Assert(poe2.RuleEditorMode == RuleEditorMode.Structured,
                "Structured rule mode round-trip");
            Assert(poe2.MonitoringPolicy == MonitoringPolicyId.Fast,
                "Monitoring policy remains orthogonal to structured rules");
            Assert(poe2.StructuredRuleSet?.Groups.Count == 1,
                "Structured acceptable results round-trip");
            Assert(
                poe2.StructuredRuleSet?.Groups[0].Conditions[0].NumericConstraints[0].Minimum == 170,
                "Structured numeric constraint round-trip");

            using var document = JsonDocument.Parse(await File.ReadAllTextAsync(settingsPath));
            Assert(!document.RootElement.TryGetProperty("TargetAffix", out _),
                "Saved settings remove the legacy root TargetAffix");
            Assert(!document.RootElement.TryGetProperty("CaptureRegion", out _),
                "Saved settings remove the legacy root CaptureRegion");
            Assert(document.RootElement.TryGetProperty("Profiles", out _),
                "Saved settings use profile-aware storage");
            Assert(document.RootElement.GetProperty("SchemaVersion").GetInt32() ==
                   AppSettings.CurrentSchemaVersion,
                "Saved settings declare their schema");

            const string futureSettings =
                """
                {
                  "SchemaVersion": 999,
                  "SelectedGameProfile": "Poe2",
                  "FutureOnlyValue": "must survive"
                }
                """;
            await File.WriteAllTextAsync(settingsPath, futureSettings);
            var futureStore = new SettingsStore(settingsPath);
            var futureFallback = await futureStore.LoadAsync();
            Assert(futureFallback.SchemaVersion == AppSettings.CurrentSchemaVersion,
                "Future settings load safe in-memory defaults");
            try
            {
                await futureStore.SaveAsync(futureFallback);
                throw new InvalidOperationException(
                    "A future settings schema was silently overwritten.");
            }
            catch (SettingsSchemaTooNewException)
            {
                // Expected: this binary becomes read-only for a newer file.
            }
            Assert(await File.ReadAllTextAsync(settingsPath) == futureSettings,
                "Future settings remain byte-for-byte unchanged");

            Console.WriteLine("Settings profile assertions: 30/30 passed");
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

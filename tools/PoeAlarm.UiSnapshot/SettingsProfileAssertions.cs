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
            const string poe1RuleName = "schema2 poe1 quick backup";
            const string poe2RuleName = "schema2 poe2 structured rules";
            await File.WriteAllTextAsync(
                settingsPath,
                $$"""
                {
                  "SchemaVersion": 2,
                  "SelectedGameProfile": "Poe2",
                  "Profiles": {
                    "Poe1": {
                      "TargetAffix": "poe1 independent target",
                      "OcrLanguage": "zh-TW",
                      "RuleEditorMode": "Quick",
                      "StructuredRuleSet": {
                        "SchemaVersion": 1,
                        "Name": "{{poe1RuleName}}",
                        "Groups": []
                      }
                    },
                    "Poe2": {
                      "TargetAffix": "poe2 independent target",
                      "OcrLanguage": "en",
                      "RuleEditorMode": "Structured",
                      "StructuredRuleSet": {
                        "SchemaVersion": 1,
                        "Name": "{{poe2RuleName}}",
                        "Groups": [
                          {
                            "Name": "schema2 result",
                            "Mode": "All",
                            "RequiredCount": 1,
                            "Conditions": [
                              {
                                "Name": "schema2 target",
                                "Template": "+#% to Critical Strike Multiplier",
                                "NumericConstraints": [
                                  { "Mode": "AtLeast", "Minimum": 35 }
                                ]
                              }
                            ]
                          }
                        ]
                      }
                    }
                  }
                }
                """);

            var schema2Store = new SettingsStore(settingsPath);
            var schema2Migrated = await schema2Store.LoadAsync();
            Assert(schema2Migrated.SchemaVersion == AppSettings.CurrentSchemaVersion,
                "Schema 2 migrates in memory to schema 3");
            Assert(!schema2Store.IsReadOnly, "Schema 2 remains writable");
            var schema2Poe1 = schema2Migrated.Profiles!.Get(GameProfile.Poe1);
            var schema2Poe2 = schema2Migrated.Profiles.Get(GameProfile.Poe2);
            Assert(schema2Poe1.TargetAffix == "poe1 independent target" &&
                   schema2Poe1.StructuredRuleSet?.Name == poe1RuleName,
                "Schema 2 preserves POE1 quick target and its own stored rules");
            Assert(schema2Poe2.TargetAffix == "poe2 independent target" &&
                   schema2Poe2.StructuredRuleSet?.Name == poe2RuleName &&
                   schema2Poe2.StructuredRuleSet.Groups[0].Conditions[0]
                       .NumericConstraints[0].Minimum == 35,
                "Schema 2 preserves POE2 rule values independently");
            Assert(schema2Poe1.RuleEditorMode == RuleEditorMode.Quick &&
                   schema2Poe2.RuleEditorMode == RuleEditorMode.Structured,
                "Schema 2 preserves each profile's rule mode independently");
            await schema2Store.SaveAsync(schema2Migrated);

            await File.WriteAllTextAsync(
                settingsPath,
                """
                {
                  "SchemaVersion": 3,
                  "SelectedGameProfile": "Poe2",
                  "Profiles": {
                    "Poe1": {
                      "TargetAffix": "legacy guarded poe1",
                      "OcrLanguage": "zh-TW",
                      "RuleEditorMode": "Quick",
                      "MonitoringPolicy": "Guarded"
                    },
                    "Poe2": {
                      "TargetAffix": "legacy guarded poe2",
                      "OcrLanguage": "en",
                      "RuleEditorMode": "Structured",
                      "MonitoringPolicy": "Guarded",
                      "StructuredRuleSet": {
                        "SchemaVersion": 1,
                        "Name": "guarded migration rules",
                        "Groups": []
                      }
                    }
                  }
                }
                """);
            var guardedStore = new SettingsStore(settingsPath);
            var guardedMigrated = await guardedStore.LoadAsync();
            var guardedPoe1 = guardedMigrated.Profiles!.Get(GameProfile.Poe1);
            var guardedPoe2 = guardedMigrated.Profiles.Get(GameProfile.Poe2);
            Assert(!guardedStore.IsReadOnly &&
                   guardedPoe1.TargetAffix == "legacy guarded poe1" &&
                   guardedPoe1.OcrLanguage == "zh-TW" &&
                   guardedPoe2.RuleEditorMode == RuleEditorMode.Structured &&
                   guardedPoe2.StructuredRuleSet?.Name == "guarded migration rules",
                "Schema 3 Guarded settings load on the fast path without losing profile data");
            await guardedStore.SaveAsync(guardedMigrated);
            using (var guardedDocument =
                   JsonDocument.Parse(await File.ReadAllTextAsync(settingsPath)))
            {
                Assert(!guardedDocument.RootElement.GetProperty("Profiles").GetProperty("Poe1")
                           .TryGetProperty("MonitoringPolicy", out _) &&
                       !guardedDocument.RootElement.GetProperty("Profiles").GetProperty("Poe2")
                           .TryGetProperty("MonitoringPolicy", out _),
                    "Saving a schema 3 Guarded file removes only the retired policy field");
            }

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
            Assert(!document.RootElement.GetProperty("Profiles").GetProperty("Poe1")
                       .TryGetProperty("MonitoringPolicy", out _) &&
                   !document.RootElement.GetProperty("Profiles").GetProperty("Poe2")
                       .TryGetProperty("MonitoringPolicy", out _),
                "Saved settings omit the retired monitoring policy");
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
            Assert(futureStore.Compatibility == SettingsStoreCompatibility.FutureSchemaReadOnly &&
                   futureStore.IsReadOnly &&
                   futureStore.DetectedSchemaVersion == 999,
                "Future settings expose an explicit read-only compatibility state");
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

            var saveWithoutLoadStore = new SettingsStore(settingsPath);
            try
            {
                await saveWithoutLoadStore.SaveAsync(new AppSettings());
                throw new InvalidOperationException(
                    "Saving without loading silently overwrote a future settings schema.");
            }
            catch (SettingsSchemaTooNewException exception)
            {
                Assert(exception.DetectedSchemaVersion == 999 &&
                       exception.SupportedSchemaVersion == AppSettings.CurrentSchemaVersion,
                    "Future-schema exception exposes detected and supported versions");
            }
            Assert(saveWithoutLoadStore.IsReadOnly &&
                   saveWithoutLoadStore.DetectedSchemaVersion == 999,
                "Save-time disk guard publishes the read-only compatibility state");
            Assert(!File.Exists(settingsPath + ".tmp"),
                "Rejected future-schema save removes its temporary file");
            Assert(await File.ReadAllTextAsync(settingsPath) == futureSettings,
                "Save without load leaves future settings byte-for-byte unchanged");

            const string futureSettingsWithLowercaseSchema =
                """
                {
                  "schemaversion": 1000,
                  "SchemaVersion": 1,
                  "FutureOnlyValue": "duplicate schema declaration must survive"
                }
                """;
            await File.WriteAllTextAsync(settingsPath, futureSettingsWithLowercaseSchema);
            var duplicateSchemaStore = new SettingsStore(settingsPath);
            await duplicateSchemaStore.LoadAsync();
            Assert(duplicateSchemaStore.IsReadOnly &&
                   duplicateSchemaStore.DetectedSchemaVersion == 1000,
                "Case-insensitive duplicate schema declarations cannot downgrade the guard");
            Assert(await File.ReadAllTextAsync(settingsPath) == futureSettingsWithLowercaseSchema,
                "Duplicate future schema settings remain byte-for-byte unchanged");

            Console.WriteLine("Settings profile assertions: 41/41 passed");
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

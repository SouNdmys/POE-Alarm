using System.Windows;
using PoeAlarm.App.Alerts;
using PoeAlarm.App.Recognition;

namespace PoeAlarm.App;

public partial class App : Application
{
    protected override void OnStartup(StartupEventArgs e)
    {
        var audioSelfTestIndex = Array.FindIndex(
            e.Args,
            argument => argument.Equals("--audio-self-test", StringComparison.OrdinalIgnoreCase));
        if (audioSelfTestIndex >= 0)
        {
            // Establish the WPF dispatcher lifecycle before requesting shutdown. Calling
            // Shutdown before base.OnStartup leaves a headless WinExe process alive on some
            // Windows/.NET combinations even though the diagnostic report has been written.
            ShutdownMode = ShutdownMode.OnExplicitShutdown;
            base.OnStartup(e);
            MainWindow?.Close();
            var exitCode = audioSelfTestIndex + 1 < e.Args.Length
                ? AlertSoundSelfTest.Run(
                    e.Args[audioSelfTestIndex + 1],
                    audioSelfTestIndex + 2 < e.Args.Length
                        ? e.Args[audioSelfTestIndex + 2]
                        : null)
                : 2;
            Shutdown(exitCode);
            return;
        }

        var selfTestIndex = Array.FindIndex(
            e.Args,
            argument => argument.Equals("--ocr-self-test", StringComparison.OrdinalIgnoreCase));
        if (selfTestIndex >= 0)
        {
            ShutdownMode = ShutdownMode.OnExplicitShutdown;
            base.OnStartup(e);
            MainWindow?.Close();
            var exitCode = selfTestIndex + 1 < e.Args.Length
                ? PaddleOcrSelfTest.Run(e.Args[selfTestIndex + 1])
                : 2;
            Shutdown(exitCode);
            return;
        }

        base.OnStartup(e);
    }
}

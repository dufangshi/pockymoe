using RemoteCodex.DeviceManager.Infrastructure;
using RemoteCodex.DeviceManager.Services;
using RemoteCodex.DeviceManager.UI;

namespace RemoteCodex.DeviceManager;

internal static class Program
{
    [STAThread]
    private static async Task Main(string[] args)
    {
        if (!OperatingSystem.IsWindows())
        {
            return;
        }

        if (args.Contains("--self-test", StringComparer.OrdinalIgnoreCase))
        {
            Environment.ExitCode = await SelfTest.RunAsync();
            return;
        }

        var nativeIndex = Array.FindIndex(args, value => value == "--native-self-test");
        if (nativeIndex >= 0)
        {
            Environment.ExitCode = nativeIndex + 1 < args.Length ? await SelfTest.RunNativeAsync(args[nativeIndex + 1]) : 1;
            return;
        }

        var previewIndex = Array.FindIndex(args, value => value.Equals("--render-preview", StringComparison.OrdinalIgnoreCase));
        if (previewIndex >= 0)
        {
            Environment.ExitCode = previewIndex + 1 < args.Length
                ? SelfTest.RenderPreview(args[previewIndex + 1])
                : 1;
            return;
        }

        var installedPath = AppPaths.InstalledExecutablePath;
        if (!args.Contains("--installed", StringComparer.OrdinalIgnoreCase)
            && !PathEquals(Environment.ProcessPath, installedPath))
        {
            try
            {
                SelfInstaller.InstallAndRestart(installedPath, args);
            }
            catch (Exception exception)
            {
                MessageBox.Show(
                    $"Pockymoe Device could not install itself.\n\n{exception.Message}",
                    ProductManifest.ProductName,
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Error);
            }
            return;
        }

        using var singleInstance = new SingleInstanceCoordinator();
        if (!singleInstance.IsPrimary)
        {
            await singleInstance.NotifyPrimaryAsync();
            return;
        }

        ApplicationConfiguration.Initialize();
        var logger = new AppLogger();
        Application.ThreadException += (_, eventArgs) => logger.Error("Unhandled UI error", eventArgs.Exception);
        AppDomain.CurrentDomain.UnhandledException += (_, eventArgs) =>
        {
            if (eventArgs.ExceptionObject is Exception exception)
            {
                logger.Error("Unhandled application error", exception);
            }
        };

        var background = args.Contains("--background", StringComparer.OrdinalIgnoreCase);
        using var context = new DeviceManagerContext(logger, background);
        singleInstance.StartListening(context.ShowMainWindow);
        Application.Run(context);
    }

    private static bool PathEquals(string? left, string right)
    {
        if (string.IsNullOrWhiteSpace(left))
        {
            return false;
        }

        return string.Equals(
            Path.GetFullPath(left),
            Path.GetFullPath(right),
            StringComparison.OrdinalIgnoreCase);
    }
}

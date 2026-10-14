namespace RemoteCodex.DeviceManager.Infrastructure;

internal static class AppPaths
{
    private static string? TestRoot { get; } = Environment.GetEnvironmentVariable(
        "POCKYMOE_DEVICE_MANAGER_TEST_ROOT") ?? Environment.GetEnvironmentVariable("REMOTE_CODEX_DEVICE_MANAGER_TEST_ROOT");

    public static bool IsTestEnvironment => TestRoot is { Length: > 0 };

    private static string LocalApplicationDataRoot { get; } = TestRoot is { Length: > 0 }
        ? Path.Combine(TestRoot, "local-app-data")
        : Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);

    private static string UserProfileRoot { get; } = TestRoot is { Length: > 0 }
        ? Path.Combine(TestRoot, "user-profile")
        : Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);

    public static string LocalRoot { get; } = Path.Combine(LocalApplicationDataRoot, "RemoteCodex");

    public static string DeviceManagerRoot { get; } = Path.Combine(LocalRoot, "DeviceManager");
    public static string InstalledExecutablePath { get; } = Path.Combine(DeviceManagerRoot, "RemoteCodex.DeviceManager.exe");
    public static string RuntimeRoot { get; } = Path.Combine(LocalRoot, "runtime");
    public static string AppRoot { get; } = Path.Combine(LocalRoot, "app");
    public static string DownloadsRoot { get; } = Path.Combine(LocalRoot, "downloads");
    public static string LogsRoot { get; } = Path.Combine(LocalRoot, "logs");
    public static string LogPath { get; } = Path.Combine(LogsRoot, "device-manager.log");
    public static string SettingsPath { get; } = Path.Combine(DeviceManagerRoot, "settings.json");
    public static string RuntimeStatePath { get; } = Path.Combine(DeviceManagerRoot, "runtime-state.json");

    public static string RelayDataRoot { get; } = Path.Combine(UserProfileRoot, ".remote-codex");
    public static string NativeRoot { get; } = Path.Combine(UserProfileRoot, ".local", "share", "remote-codex", "native");
    public static string NativeCurrentPath { get; } = Path.Combine(NativeRoot, "current.json");
    public static string UserHome => UserProfileRoot;
    public static string DefaultWorkspaceRoot { get; } = UserProfileRoot;

    public static string RelayConfigPath { get; } = Path.Combine(RelayDataRoot, "relay-supervisor.json");
    public static string RelayLogPath { get; } = Path.Combine(RelayDataRoot, "logs", "relay-supervisor.log");

    public static void EnsureDirectories()
    {
        Directory.CreateDirectory(DeviceManagerRoot);
        Directory.CreateDirectory(RuntimeRoot);
        Directory.CreateDirectory(AppRoot);
        Directory.CreateDirectory(DownloadsRoot);
        Directory.CreateDirectory(LogsRoot);
    }
}

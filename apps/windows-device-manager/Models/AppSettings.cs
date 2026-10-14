using System.Text.Json;
using System.Text.Json.Serialization;
using RemoteCodex.DeviceManager.Infrastructure;

namespace RemoteCodex.DeviceManager.Models;

internal sealed record AppSettings
{
    public string RelayUrl { get; init; } = ProductManifest.DefaultRelayUrl;
    public string WorkspaceRoot { get; init; } = AppPaths.DefaultWorkspaceRoot;
    public int SupervisorPort { get; init; } = ProductManifest.DefaultSupervisorPort;
    public bool StartWithWindows { get; init; } = true;
    public bool KeepOnline { get; init; }
    public string Theme { get; init; } = "plum-dark";

    public static AppSettings Load(AppLogger logger)
    {
        try
        {
            if (!File.Exists(AppPaths.SettingsPath))
            {
                return new AppSettings();
            }

            return JsonSerializer.Deserialize<AppSettings>(File.ReadAllText(AppPaths.SettingsPath), JsonOptions)
                ?? new AppSettings();
        }
        catch (Exception exception)
        {
            logger.Warning($"Ignoring invalid device manager settings: {exception.Message}");
            return new AppSettings();
        }
    }

    public void Save()
    {
        AppPaths.EnsureDirectories();
        var temporaryPath = $"{AppPaths.SettingsPath}.{Environment.ProcessId}.tmp";
        File.WriteAllText(temporaryPath, JsonSerializer.Serialize(this, JsonOptions));
        File.Move(temporaryPath, AppPaths.SettingsPath, overwrite: true);
    }

    private static JsonSerializerOptions JsonOptions { get; } = new()
    {
        WriteIndented = true,
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Skip,
    };
}

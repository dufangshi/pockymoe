using System.Text.Json;
using System.Text.Json.Serialization;
using RemoteCodex.DeviceManager.Infrastructure;

namespace RemoteCodex.DeviceManager.Models;

// Keep legacy fields so upgrading the tray app can read existing installations.
internal sealed record RuntimeState(string NodePath, string RemoteCodexEntryPath,
    string CodexCommandPath, DateTimeOffset UpdatedAt, string? RemoteCodexVersion = null,
    string? NativeBinaryPath = null, string? Distribution = null)
{
    public bool IsNative => Distribution == "github-release";
    public bool IsUsable => IsNative ? ResolveNative() is not null
        : File.Exists(NodePath) && File.Exists(RemoteCodexEntryPath);
    [JsonIgnore] public string? ManagedNpmPrefix => new FileInfo(RemoteCodexEntryPath).Directory?.Parent?.Parent?.Parent?.FullName;
    public static NativeInstallation? ResolveNative()
    {
        try
        {
            var installed = JsonSerializer.Deserialize<NativeInstallation>(File.ReadAllText(AppPaths.NativeCurrentPath), JsonOptions);
            if (installed is null || !Version.TryParse(installed.Version, out _)) return null;
            var releases = Path.GetFullPath(Path.Combine(AppPaths.NativeRoot, "releases")) + Path.DirectorySeparatorChar;
            return Path.GetFullPath(installed.Executable).StartsWith(releases, StringComparison.OrdinalIgnoreCase)
                && Path.GetFullPath(installed.WebDist).StartsWith(releases, StringComparison.OrdinalIgnoreCase)
                && File.Exists(installed.Executable) && File.Exists(Path.Combine(installed.WebDist, "index.html")) ? installed : null;
        }
        catch { return null; }
    }
    public static RuntimeState? FromNative() => ResolveNative() is { } n
        ? new("", "", "", DateTimeOffset.UtcNow, n.Version, n.Executable, "github-release") : null;
    public void Save()
    {
        AppPaths.EnsureDirectories();
        var temporary = $"{AppPaths.RuntimeStatePath}.{Environment.ProcessId}.tmp";
        File.WriteAllText(temporary, JsonSerializer.Serialize(this, JsonOptions));
        File.Move(temporary, AppPaths.RuntimeStatePath, true);
    }
    public static RuntimeState? Load(AppLogger logger)
    {
        try
        {
            return FromNative() ?? (File.Exists(AppPaths.RuntimeStatePath)
            ? JsonSerializer.Deserialize<RuntimeState>(File.ReadAllText(AppPaths.RuntimeStatePath), JsonOptions) : null);
        }
        catch (Exception e) { logger.Warning($"Invalid runtime state: {e.Message}"); return null; }
    }
    internal static JsonSerializerOptions JsonOptions { get; } = new() { PropertyNamingPolicy = JsonNamingPolicy.CamelCase, WriteIndented = true };
}
internal sealed record NativeInstallation(string Version, string Executable, string WebDist);

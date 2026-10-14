using System.Text.RegularExpressions;

namespace RemoteCodex.DeviceManager.Infrastructure;

internal sealed partial class AppLogger
{
    private const long MaximumLogBytes = 2 * 1024 * 1024;
    private readonly object _sync = new();

    public AppLogger()
    {
        AppPaths.EnsureDirectories();
        RotateIfNeeded();
    }

    public void Info(string message) => Write("INFO", message);

    public void Warning(string message) => Write("WARN", message);

    public void Error(string message, Exception? exception = null)
    {
        Write("ERROR", exception is null ? message : $"{message}: {exception.Message}");
    }

    private void Write(string level, string message)
    {
        var safeMessage = Redact(message).ReplaceLineEndings(" ");
        var line = $"{DateTimeOffset.Now:O} [{level}] {safeMessage}{Environment.NewLine}";
        lock (_sync)
        {
            File.AppendAllText(AppPaths.LogPath, line);
        }
    }

    private static void RotateIfNeeded()
    {
        try
        {
            if (!File.Exists(AppPaths.LogPath) || new FileInfo(AppPaths.LogPath).Length < MaximumLogBytes)
            {
                return;
            }

            File.Move(AppPaths.LogPath, $"{AppPaths.LogPath}.1", overwrite: true);
        }
        catch
        {
            // Logging remains best effort when another instance owns the file.
        }
    }

    internal static string Redact(string value)
    {
        var output = DeviceTokenPattern().Replace(value, "rcd_[redacted]");
        output = SecretAssignmentPattern().Replace(output, "$1=[redacted]");
        return AuthorizationPattern().Replace(output, "$1 [redacted]");
    }

    [GeneratedRegex(@"rcd_[A-Za-z0-9_-]+", RegexOptions.CultureInvariant)]
    private static partial Regex DeviceTokenPattern();

    [GeneratedRegex(@"(?i)\b((?:POCKYMOE_|REMOTE_CODEX_)[A-Za-z_]*(?:TOKEN|PASSWORD|SECRET)|TOKEN|PASSWORD|SECRET|controlToken)\s*[:=]\s*[^\s,;]+", RegexOptions.CultureInvariant)]
    private static partial Regex SecretAssignmentPattern();

    [GeneratedRegex(@"(?i)\b(Authorization)\s+[^\s,;]+", RegexOptions.CultureInvariant)]
    private static partial Regex AuthorizationPattern();
}

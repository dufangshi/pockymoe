using System.Text.Json;
using RemoteCodex.DeviceManager.Infrastructure;

namespace RemoteCodex.DeviceManager.Models;

internal sealed record RelayConfigurationSnapshot(
    bool HasToken,
    string? RelayUrl,
    string? WorkspaceRoot,
    int? SupervisorPort)
{
    public static RelayConfigurationSnapshot Load(AppLogger logger)
    {
        try
        {
            if (!File.Exists(AppPaths.RelayConfigPath))
            {
                return new(false, null, null, null);
            }

            using var document = JsonDocument.Parse(File.ReadAllText(AppPaths.RelayConfigPath));
            var root = document.RootElement;
            return new RelayConfigurationSnapshot(
                HasNonEmptyString(root, "REMOTE_CODEX_RELAY_AGENT_TOKEN"),
                GetString(root, "REMOTE_CODEX_RELAY_SERVER_URL"),
                GetString(root, "REMOTE_CODEX_WORKSPACE_ROOT") ?? GetString(root, "WORKSPACE_ROOT"),
                GetInt32(root, "REMOTE_CODEX_RELAY_SUPERVISOR_PORT"));
        }
        catch (Exception exception)
        {
            logger.Warning($"Ignoring invalid relay configuration: {exception.Message}");
            return new(false, null, null, null);
        }
    }

    private static bool HasNonEmptyString(JsonElement root, string propertyName)
        => !string.IsNullOrWhiteSpace(GetString(root, propertyName));

    private static string? GetString(JsonElement root, string propertyName)
        => (root.TryGetProperty(propertyName.Replace("REMOTE_CODEX_", "POCKYMOE_"), out var property) || root.TryGetProperty(propertyName, out property)) && property.ValueKind == JsonValueKind.String
            ? property.GetString()
            : null;

    private static int? GetInt32(JsonElement root, string propertyName)
    {
        if (!root.TryGetProperty(propertyName.Replace("REMOTE_CODEX_", "POCKYMOE_"), out var property) && !root.TryGetProperty(propertyName, out property))
        {
            return null;
        }

        if (property.ValueKind == JsonValueKind.Number && property.TryGetInt32(out var number))
        {
            return number;
        }

        return property.ValueKind == JsonValueKind.String && int.TryParse(property.GetString(), out number)
            ? number
            : null;
    }
}

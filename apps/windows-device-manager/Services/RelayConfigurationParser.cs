using System.Text.RegularExpressions;

namespace RemoteCodex.DeviceManager.Services;

internal sealed record ParsedRelayConfiguration(string RelayUrl, string DeviceToken, int? SupervisorPort,
    string? SetupCode = null, string? AccountKey = null, string? EnrollmentSecret = null, string? WebOrigin = null);

internal static partial class RelayConfigurationParser
{
    public static bool TryParse(string text, out ParsedRelayConfiguration? configuration, out string error)
    {
        configuration = null;
        error = "Paste the PowerShell setup command from the Devices page.";
        if (string.IsNullOrWhiteSpace(text)) return false;
        var normalized = Regex.Replace(text, @"\([_:/])", "$1");
        string? Env(string suffix) => Assignment(normalized, "POCKYMOE_" + suffix) ?? Assignment(normalized, "REMOTE_CODEX_" + suffix);
        string? Flag(string name) => Extract(normalized, $@"(?<![A-Za-z0-9_])--?{Regex.Escape(name)}\s+");
        var relay = Flag("Relay") ?? Env("RELAY_SERVER_URL");
        var token = Flag("Token") ?? Env("RELAY_AGENT_TOKEN") ?? "";
        var code = Flag("Code");
        var account = Flag("AccountKey") ?? Flag("account-key");
        var enroll = Env("ENROLL_SECRET");
        var web = Flag("WebOrigin") ?? Flag("web-origin");
        var portText = Flag("Port") ?? Env("RELAY_SUPERVISOR_PORT");
        if (string.IsNullOrWhiteSpace(relay) || (string.IsNullOrWhiteSpace(token) == string.IsNullOrWhiteSpace(code))) return false;
        var http = relay.Replace("wss://", "https://").Replace("ws://", "http://");
        if (!Uri.TryCreate(http, UriKind.Absolute, out var uri) || (uri.Scheme != "https" && !(uri.Scheme == "http" && uri.IsLoopback))
            || uri.UserInfo.Length > 0 || uri.Query.Length > 0 || uri.Fragment.Length > 0) { error = "Invalid Relay URL."; return false; }
        int? port = null;
        if (portText is not null)
        {
            if (!int.TryParse(portText, out var n) || n is < 1024 or > 65535) { error = "Invalid local port."; return false; }
            port = n;
        }
        if (string.IsNullOrWhiteSpace(account) != string.IsNullOrWhiteSpace(enroll)) { error = "The setup command is missing account enrollment information."; return false; }
        configuration = new(uri.AbsoluteUri.TrimEnd('/'), token, port, code, account, enroll, web);
        error = "";
        return true;
    }
    private static string? Assignment(string text, string name) => Extract(text, $@"\b{Regex.Escape(name)}\s*=\s*");
    private static string? Extract(string text, string prefix)
    {
        var match = Regex.Match(text, prefix + "(?:'(?<value>(?:[^']|'')*)'|\"(?<value>[^\"]*)\"|(?<value>[^\\s;\\}]+))", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
        return match.Success ? match.Groups["value"].Value.Replace("''", "'").Trim() : null;
    }
}

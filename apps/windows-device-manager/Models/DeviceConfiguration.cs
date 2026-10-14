namespace RemoteCodex.DeviceManager.Models;

internal sealed record DeviceConfiguration(string RelayUrl, string DeviceToken, string WorkspaceRoot,
    int SupervisorPort, string? SetupCode = null, string? AccountKey = null,
    string? EnrollmentSecret = null, string? WebOrigin = null)
{
    public string HttpRelayUrl => RelayUrl.Trim().Replace("wss://", "https://").Replace("ws://", "http://").TrimEnd('/');
    public IReadOnlyList<string> Validate(bool hasSavedToken)
    {
        var errors = new List<string>();
        if (!Uri.TryCreate(HttpRelayUrl, UriKind.Absolute, out var uri)
            || (uri.Scheme != "https" && !(uri.Scheme == "http" && uri.IsLoopback))
            || !string.IsNullOrEmpty(uri.UserInfo) || !string.IsNullOrEmpty(uri.Query) || !string.IsNullOrEmpty(uri.Fragment))
            errors.Add("Use an HTTPS Relay URL (HTTP is allowed only for localhost).");
        if (string.IsNullOrWhiteSpace(DeviceToken) && string.IsNullOrWhiteSpace(SetupCode) && !hasSavedToken)
            errors.Add("Paste a device setup command from the Devices page.");
        if (!string.IsNullOrWhiteSpace(DeviceToken) && !string.IsNullOrWhiteSpace(SetupCode)) errors.Add("Use a token or a setup code, not both.");
        if (string.IsNullOrWhiteSpace(AccountKey) != string.IsNullOrWhiteSpace(EnrollmentSecret)) errors.Add("Account enrollment requires both the account key and enrollment secret.");
        if (WebOrigin is { Length: > 0 } origin && (!Uri.TryCreate(origin, UriKind.Absolute, out var web) || (web.Scheme != "https" && !(web.Scheme == "http" && web.IsLoopback)))) errors.Add("Invalid Web origin.");
        if (string.IsNullOrWhiteSpace(WorkspaceRoot) || !Path.IsPathFullyQualified(WorkspaceRoot)) errors.Add("Choose an absolute workspace path.");
        if (SupervisorPort is < 1024 or > 65535) errors.Add("Local port must be between 1024 and 65535.");
        return errors;
    }
}

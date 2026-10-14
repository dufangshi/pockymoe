using System.Security.Cryptography;
using System.Text.RegularExpressions;
using RemoteCodex.DeviceManager.Infrastructure;
using RemoteCodex.DeviceManager.Models;

namespace RemoteCodex.DeviceManager.Services;

internal sealed class RuntimeProvisioner(AppLogger logger, ProcessRunner runner)
{
    private static readonly HttpClient Client = CreateClient();
    private static HttpClient CreateClient()
    {
        var client = new HttpClient { Timeout = TimeSpan.FromMinutes(10) };
        client.DefaultRequestHeaders.UserAgent.ParseAdd("Pockymoe-DeviceManager/0.2.0");
        return client;
    }
    public async Task<string> GetLatestRemoteCodexVersionAsync(CancellationToken cancellationToken)
    {
        var raw = (await Client.GetStringAsync($"{ProductManifest.ReleaseRepository}/releases/latest/download/runtime-version.txt", cancellationToken)).Trim();
        if (!Regex.IsMatch(raw, @"^[0-9]+\.[0-9]+\.[0-9]+$") || !Version.TryParse(raw, out _))
            throw new InvalidOperationException("GitHub returned an invalid runtime version.");
        if (!SupportsNativeMaintenance(raw))
            throw new InvalidOperationException($"Windows requires Pockymoe {ProductManifest.MinimumNativeRuntimeVersion} or newer. Retry after GitHub latest has refreshed.");
        return raw;
    }
    public Task<RuntimeState?> TryRecoverBundledRuntimeAsync(CancellationToken cancellationToken) => Task.FromResult(RuntimeState.FromNative());
    public static bool IsNewerVersion(string candidate, string current) => Version.TryParse(candidate, out var a) && Version.TryParse(current, out var b) && a > b;

    internal static bool SupportsNativeMaintenance(string version) => Version.TryParse(version, out var parsed)
        && parsed >= Version.Parse(ProductManifest.MinimumNativeRuntimeVersion);

    internal static string Checksum(string text, string asset)
    {
        var matches = Regex.Matches(text, $@"(?m)^([0-9a-fA-F]{{64}})\s+\*?{Regex.Escape(asset)}\s*$");
        if (matches.Count != 1) throw new InvalidOperationException("Missing or ambiguous runtime checksum.");
        return matches[0].Groups[1].Value.ToLowerInvariant();
    }
    public async Task<RuntimeState> SetupAsync(DeviceConfiguration configuration,
        IProgress<ProvisioningProgress> progress, CancellationToken cancellationToken)
    {
        AppPaths.EnsureDirectories();
        progress.Report(new("GitHub", "Checking the current native release…", ProvisioningStepState.Running, 8));
        var version = await GetLatestRemoteCodexVersionAsync(cancellationToken);
        var installed = RuntimeState.ResolveNative();
        if (installed is not null && IsNewerVersion(installed.Version, version))
            throw new InvalidOperationException("The installed runtime is newer than GitHub latest. Automatic downgrade is disabled.");
        const string asset = "remote-codex-win32-x64-msvc-cli.exe";
        var baseUrl = $"{ProductManifest.ReleaseRepository}/releases/download/v{version}";
        var sum = Checksum(await Client.GetStringAsync($"{baseUrl}/SHA256SUMS", cancellationToken), asset);
        var stage = Path.Combine(AppPaths.DownloadsRoot, Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(stage);
        try
        {
            var binary = Path.Combine(stage, "pockymoe.exe");
            progress.Report(new("Download", $"Downloading Pockymoe {version}…", ProvisioningStepState.Running, 25));
            using (var response = await Client.GetAsync($"{baseUrl}/{asset}", HttpCompletionOption.ResponseHeadersRead, cancellationToken))
            {
                response.EnsureSuccessStatusCode();
                await using var stream = File.Create(binary);
                await response.Content.CopyToAsync(stream, cancellationToken);
            }
            await using (var stream = File.OpenRead(binary))
                if (Convert.ToHexString(await SHA256.HashDataAsync(stream, cancellationToken)).ToLowerInvariant() != sum)
                    throw new InvalidOperationException("Runtime checksum mismatch. Nothing was installed.");
            var actual = await runner.RunAsync(binary, ["version"], TimeSpan.FromSeconds(15), cancellationToken: cancellationToken);
            if (!actual.Success || actual.StandardOutput.Trim() != version) throw new InvalidOperationException("Downloaded runtime version mismatch.");
            progress.Report(new("Setup", "Installing the verified runtime and Web UI…", ProvisioningStepState.Running, 60));
            var args = new List<string> { "setup", "--relay", configuration.HttpRelayUrl, "--port", configuration.SupervisorPort.ToString() };
            if (configuration.SetupCode is { Length: > 0 } code) args.AddRange(["--code", code]);
            else args.AddRange(["--token", configuration.DeviceToken]);
            if (configuration.AccountKey is { Length: > 0 } key) args.AddRange(["--account-key", key]);
            if (configuration.WebOrigin is { Length: > 0 } origin) args.AddRange(["--web-origin", origin]);
            var env = new Dictionary<string, string?>
            {
                ["POCKYMOE_ENROLL_SECRET"] = configuration.EnrollmentSecret,
                ["WORKSPACE_ROOT"] = configuration.WorkspaceRoot,
                ["HOME"] = AppPaths.UserHome,
                ["USERPROFILE"] = AppPaths.UserHome
            };
            var result = await runner.RunAsync(binary, args, TimeSpan.FromMinutes(10), env, cancellationToken: cancellationToken);
            if (!result.Success) throw new InvalidOperationException("Native setup failed. " + AppLogger.Redact(result.CombinedOutput));
            var runtime = RuntimeState.FromNative() ?? throw new InvalidOperationException("Native setup did not produce a valid installation.");
            runtime.Save();
            logger.Info("Native setup completed.");
            return runtime;
        }
        finally { Directory.Delete(stage, true); }
    }
}

using System.Diagnostics;
using System.Text.Json;
using RemoteCodex.DeviceManager.Infrastructure;
using RemoteCodex.DeviceManager.Models;

namespace RemoteCodex.DeviceManager.Services;

internal sealed class RelaySupervisorService(ProcessRunner runner, AppLogger logger)
{
    internal static Dictionary<string, JsonElement> ReadConfiguration()
    {
        var raw = JsonSerializer.Deserialize<Dictionary<string, JsonElement>>(File.ReadAllText(AppPaths.RelayConfigPath)) ?? [];
        foreach (var key in raw.Keys.ToArray())
            if (key.StartsWith("REMOTE_CODEX_")) raw.TryAdd(key.Replace("REMOTE_CODEX_", "POCKYMOE_"), raw[key]);
        return raw;
    }
    internal static string Saved(string key)
    {
        var config = ReadConfiguration();
        return config.TryGetValue(key, out var value) ? value.ToString() : "";
    }
    private static int Port => int.TryParse(Saved("POCKYMOE_RELAY_SUPERVISOR_PORT"), out var port) ? port : ProductManifest.DefaultSupervisorPort;
    private static HttpClient LocalClient() => new(new HttpClientHandler { UseProxy = false, AllowAutoRedirect = false }) { Timeout = TimeSpan.FromSeconds(20) };
    public async Task<JsonElement> ManagementAsync(string action, bool post, CancellationToken cancellationToken)
    {
        using var client = LocalClient();
        var root = $"http://127.0.0.1:{Port}";
        var credentials = new { username = Saved("POCKYMOE_ADMIN_USERNAME"), password = Saved("POCKYMOE_ADMIN_PASSWORD") };
        using var login = await client.PostAsync(root + "/api/auth/login", new StringContent(JsonSerializer.Serialize(credentials), System.Text.Encoding.UTF8, "application/json"), cancellationToken);
        login.EnsureSuccessStatusCode();
        using var response = post ? await client.PostAsync(root + "/api/management/supervisor" + action, new StringContent("{}", System.Text.Encoding.UTF8, "application/json"), cancellationToken)
            : await client.GetAsync(root + "/api/management/supervisor" + action, cancellationToken);
        if (!response.IsSuccessStatusCode) throw new InvalidOperationException($"Device management returned {(int)response.StatusCode}. Use Web Settings to inspect the device.");
        using var body = JsonDocument.Parse(await response.Content.ReadAsStringAsync(cancellationToken));
        return body.RootElement.Clone();
    }
    public async Task<SupervisorState> GetStateAsync(RuntimeState runtime, CancellationToken cancellationToken)
    {
        if (!runtime.IsNative)
        {
            var status = await runner.RunAsync(runtime.NodePath, [runtime.RemoteCodexEntryPath, "relay-supervisor", "status"], TimeSpan.FromSeconds(10), cancellationToken: cancellationToken);
            return ParseState(status);
        }
        try
        {
            var health = await HealthAsync(cancellationToken);
            return health.TryGetProperty("relayConnected", out var connected) && connected.ValueKind == JsonValueKind.False ? SupervisorState.Starting : SupervisorState.Running;
        }
        catch (HttpRequestException) { return SupervisorState.Stopped; }
        catch (TaskCanceledException) when (!cancellationToken.IsCancellationRequested) { return SupervisorState.Unknown; }
    }
    internal async Task<JsonElement> HealthAsync(CancellationToken cancellationToken)
    {
        using var client = LocalClient(); client.Timeout = TimeSpan.FromSeconds(3);
        using var response = await client.GetAsync($"http://127.0.0.1:{Port}/healthz", cancellationToken);
        response.EnsureSuccessStatusCode();
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync(cancellationToken));
        if (!doc.RootElement.TryGetProperty("status", out var status) || status.GetString() != "ok") throw new HttpRequestException("Unexpected local service.");
        return doc.RootElement.Clone();
    }
    internal static SupervisorState ParseState(ProcessResult result) => result.Success && result.CombinedOutput.Split(['\r', '\n']).Any(s => s.Trim().Equals("Relay supervisor: running", StringComparison.OrdinalIgnoreCase)) ? SupervisorState.Running : SupervisorState.Stopped;
    public async Task StartAsync(RuntimeState runtime, DeviceConfiguration configuration, CancellationToken cancellationToken)
    {
        if (!runtime.IsNative) throw new InvalidOperationException("This installation uses the retired npm runtime. Disconnect it, then paste its current setup command to migrate.");
        var installed = RuntimeState.ResolveNative() ?? throw new InvalidOperationException("Native installation is missing.");
        // Resolve current.json on every launch: updates must never restart a pinned old executable.
        var state = await GetStateAsync(runtime, cancellationToken);
        if (state is SupervisorState.Running or SupervisorState.Starting) { await ManagementAsync("", false, cancellationToken); return; }
        if (MaintenancePending()) throw new InvalidOperationException("Supervisor maintenance is still in progress. Wait for it to finish.");
        var script = Path.Combine(AppPaths.RuntimeRoot, $"start-{Guid.NewGuid():N}.ps1");
        static string Quote(string value) => "'" + value.Replace("'", "''") + "'";
        await File.WriteAllTextAsync(script, $"$ErrorActionPreference='Stop'\nRemove-Item -LiteralPath $PSCommandPath -Force\n$env:HOME={Quote(AppPaths.UserHome)}\n$env:USERPROFILE={Quote(AppPaths.UserHome)}\nRemove-Item Env:POCKYMOE_ENROLL_SECRET -ErrorAction SilentlyContinue\n& {Quote(installed.Executable)} device-run --config {Quote(AppPaths.RelayConfigPath)} *> {Quote(AppPaths.RelayLogPath)}\nexit $LASTEXITCODE\n", cancellationToken);
        Directory.CreateDirectory(Path.GetDirectoryName(AppPaths.RelayLogPath)!);
        var commandLine = $"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{script}\"";
        var result = await runner.RunAsync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", "$r=Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{CommandLine=" + Quote(commandLine) + "}; exit $r.ReturnValue"], TimeSpan.FromSeconds(20), cancellationToken: cancellationToken);
        if (!result.Success) { File.Delete(script); throw new InvalidOperationException("Unable to launch the native Supervisor."); }
        for (var i = 0; i < 60; i++)
        {
            if (await GetStateAsync(runtime, cancellationToken) == SupervisorState.Running) { await ManagementAsync("", false, cancellationToken); return; }
            await Task.Delay(1000, cancellationToken);
        }
        throw new InvalidOperationException("Supervisor did not connect. Open its logs for details.");
    }
    public async Task ConfigureWorkspaceAsync(string folder, CancellationToken cancellationToken)
    {
        var config = JsonSerializer.Deserialize<Dictionary<string, JsonElement>>(File.ReadAllText(AppPaths.RelayConfigPath))!;
        var normalized = Path.GetFullPath(folder);
        var previous = config.TryGetValue("POCKYMOE_WORKSPACE_ROOT", out var value) || config.TryGetValue("REMOTE_CODEX_WORKSPACE_ROOT", out value) || config.TryGetValue("WORKSPACE_ROOT", out value) ? value.GetString() : null;
        if (previous == normalized) return;
        config.Remove("POCKYMOE_WORKSPACE_ROOT");
        config["REMOTE_CODEX_WORKSPACE_ROOT"] = JsonSerializer.SerializeToElement(normalized);
        var temporary = AppPaths.RelayConfigPath + ".manager.tmp";
        await File.WriteAllTextAsync(temporary, JsonSerializer.Serialize(config, RuntimeState.JsonOptions), cancellationToken);
        File.Move(temporary, AppPaths.RelayConfigPath, true);
        // Maintain the native one-use-code resumption receipt after this preference-only change.
        var receipt = AppPaths.RelayConfigPath + ".setup.json";
        if (File.Exists(receipt))
        {
            var data = JsonSerializer.Deserialize<Dictionary<string, JsonElement>>(File.ReadAllText(receipt))!;
            data["configHash"] = JsonSerializer.SerializeToElement(Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(File.ReadAllBytes(AppPaths.RelayConfigPath))).ToLowerInvariant());
            await File.WriteAllTextAsync(receipt + ".tmp", JsonSerializer.Serialize(data), cancellationToken);
            File.Move(receipt + ".tmp", receipt, true);
        }
        // Supervisor maintenance owns draining/recovery when applying the preference.
        await ManagementAsync("/restart", true, cancellationToken);
        for (var i = 0; i < 120; i++)
        {
            await Task.Delay(1000, cancellationToken);
            try
            {
                var current = await ManagementAsync("", false, cancellationToken);
                if (current.TryGetProperty("job", out var job) && job.TryGetProperty("phase", out var phase))
                {
                    if (phase.GetString() == "failed") throw new InvalidOperationException("Workspace restart failed. Open Web Settings for recovery details.");
                    if (phase.GetString() == "completed") return;
                }
            }
            catch (Exception e) when (e is HttpRequestException or TaskCanceledException) { }
        }
        throw new TimeoutException("Workspace preference saved; restart is still pending in Web Settings.");
    }

    internal static bool MaintenancePending()
    {
        try
        {
            var db = Saved("POCKYMOE_DATABASE_PATH");
            if (string.IsNullOrEmpty(db)) db = Saved("DATABASE_URL");
            var statusFile = Path.Combine(Path.GetDirectoryName(db)!, "updates", "supervisor-update.json");
            using var doc = JsonDocument.Parse(File.ReadAllText(statusFile));
            var phase = doc.RootElement.GetProperty("phase").GetString();
            return phase is "scheduled" or "preparing" or "installing" or "restarting" or "verifying";
        }
        catch { return false; }
    }
    public async Task StopAsync(RuntimeState runtime, CancellationToken cancellationToken)
    {
        if (!runtime.IsNative)
        {
            var result = await runner.RunAsync(runtime.NodePath, [runtime.RemoteCodexEntryPath, "relay-supervisor", "stop"], TimeSpan.FromSeconds(30), cancellationToken: cancellationToken);
            if (!result.Success) throw new InvalidOperationException("Legacy Supervisor could not be stopped.");
            return;
        }
        if (MaintenancePending()) throw new InvalidOperationException("Wait for Supervisor maintenance to finish before disconnecting.");
        if (await GetStateAsync(runtime, cancellationToken) == SupervisorState.Stopped) return;
        // Authenticate first and only terminate the exact native executable reported by this device.
        var status = await ManagementAsync("", false, cancellationToken);
        var pid = status.GetProperty("processId").GetInt32();
        var expected = status.GetProperty("path").GetString();
        using var process = Process.GetProcessById(pid);
        if (!string.Equals(process.MainModule?.FileName, expected, StringComparison.OrdinalIgnoreCase)
            || expected is null || !Path.GetFullPath(expected).StartsWith(Path.GetFullPath(AppPaths.NativeRoot) + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("The local process is not owned by this native installation.");
        process.Kill(entireProcessTree: true);
        await process.WaitForExitAsync(cancellationToken);
        logger.Info("Native Supervisor disconnected by the user.");
    }
}

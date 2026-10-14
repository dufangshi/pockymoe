using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using RemoteCodex.DeviceManager.Models;
using RemoteCodex.DeviceManager.Services;
using RemoteCodex.DeviceManager.UI;

namespace RemoteCodex.DeviceManager.Infrastructure;

internal static class SelfTest
{
    private static void Require(bool passed, string message) { if (!passed) throw new InvalidOperationException(message); }
    public static async Task<int> RunAsync()
    {
        try
        {
            Require(AppPaths.IsTestEnvironment, "Self-tests require an isolated test root.");
            AppPaths.EnsureDirectories(); Directory.CreateDirectory(AppPaths.RelayDataRoot);
            var current = "$env:POCKYMOE_ENROLL_SECRET='synthetic-enroll'; try { & ([scriptblock]::Create('unused')) -Relay 'https://relay.example.test' -Token 'synthetic-token' -Port 45680 -WebOrigin 'https://web.example.test' -AccountKey 'synthetic-account' } finally { Remove-Item Env:POCKYMOE_ENROLL_SECRET }";
            Require(RelayConfigurationParser.TryParse(current, out var parsed, out _) && parsed is { EnrollmentSecret: "synthetic-enroll", AccountKey: "synthetic-account", DeviceToken: "synthetic-token", SupervisorPort: 45680, WebOrigin: "https://web.example.test" }, "Current account-bound setup command");
            Require(RelayConfigurationParser.TryParse("$env:REMOTE_CODEX_RELAY_SERVER_URL='wss://relay.example.test'; $env:REMOTE_CODEX_RELAY_AGENT_TOKEN='rcd_synthetic'; $env:REMOTE_CODEX_RELAY_SUPERVISOR_PORT='45680'", out parsed, out _) && parsed?.RelayUrl == "https://relay.example.test", "Legacy setup command");
            Require(RelayConfigurationParser.TryParse("pockymoe setup --relay https://relay.example.test --code synthetic-code --port 45680", out parsed, out _) && parsed?.SetupCode == "synthetic-code", "Setup code parsing");
            Require(!RelayConfigurationParser.TryParse("setup -Relay 'https://relay.example.test' -Token a -Code b", out _, out _), "Ambiguous credentials rejected");
            Require(!RelayConfigurationParser.TryParse("setup -Relay 'https://relay.example.test' -Token a -AccountKey b", out _, out _), "Incomplete account binding rejected");
            var sum = new string('a', 64); Require(RuntimeProvisioner.Checksum(sum + "  remote-codex-win32-x64-msvc-cli.exe\n", "remote-codex-win32-x64-msvc-cli.exe") == sum, "Checksum parsing");
            try { RuntimeProvisioner.Checksum(sum + "  binary\n" + sum + "  binary\n", "binary"); throw new Exception("Duplicate checksum accepted"); } catch (InvalidOperationException) { }
            Require(RuntimeProvisioner.IsNewerVersion("0.13.2", "0.12.77") && !RuntimeProvisioner.IsNewerVersion("0.12.77", "0.13.2"), "Version ordering");
            var release = Path.Combine(AppPaths.NativeRoot, "releases", "9.1.0"); Directory.CreateDirectory(Path.Combine(release, "web"));
            File.WriteAllText(Path.Combine(release, "web", "index.html"), "fixture"); File.WriteAllText(Path.Combine(release, "pockymoe.exe"), "fixture");
            var installed = new NativeInstallation("9.1.0", Path.Combine(release, "pockymoe.exe"), Path.Combine(release, "web"));
            File.WriteAllText(AppPaths.NativeCurrentPath, JsonSerializer.Serialize(installed, RuntimeState.JsonOptions));
            Require(RuntimeState.FromNative()?.RemoteCodexVersion == "9.1.0", "Native current discovery");
            File.WriteAllText(AppPaths.NativeCurrentPath, JsonSerializer.Serialize(installed with { Executable = Environment.ProcessPath! }, RuntimeState.JsonOptions));
            Require(RuntimeState.ResolveNative() is null, "Unmanaged executable rejected");
            File.WriteAllText(AppPaths.NativeCurrentPath, JsonSerializer.Serialize(installed, RuntimeState.JsonOptions));
            var runner = new ProcessRunner(new AppLogger());
            var service = new RelaySupervisorService(runner, new AppLogger());
            var tcp = new TcpListener(IPAddress.Loopback, 0); tcp.Start(); var port = ((IPEndPoint)tcp.LocalEndpoint).Port; tcp.Stop();
            var database = Path.Combine(AppPaths.RelayDataRoot, "preserved.sqlite"); File.WriteAllText(database, "preserve-device-and-history");
            File.WriteAllText(AppPaths.RelayConfigPath, JsonSerializer.Serialize(new Dictionary<string, object> { ["REMOTE_CODEX_RELAY_SUPERVISOR_PORT"] = port.ToString(), ["REMOTE_CODEX_ADMIN_USERNAME"] = "admin", ["REMOTE_CODEX_ADMIN_PASSWORD"] = "synthetic-password", ["REMOTE_CODEX_RELAY_AGENT_TOKEN"] = "synthetic-token", ["REMOTE_CODEX_DATABASE_PATH"] = database, ["POCKYMOE_WORKSPACE_ROOT"] = AppPaths.UserHome }));
            Require(RelayConfigurationSnapshot.Load(new AppLogger()).WorkspaceRoot == AppPaths.UserHome, "Native workspace discovery");
            using var server = new HttpListener(); server.Prefixes.Add($"http://127.0.0.1:{port}/"); server.Start();
            var served = Task.Run(async () =>
            {
                for (var i = 0; i < 2; i++)
                {
                    var request = await server.GetContextAsync();
                    var route = request.Request.Url!.AbsolutePath;
                    if (route == "/api/auth/login")
                    {
                        using var body = JsonDocument.Parse(await new StreamReader(request.Request.InputStream).ReadToEndAsync());
                        Require(body.RootElement.GetProperty("password").GetString() == "synthetic-password", "Saved local auth");
                        request.Response.Headers.Add("Set-Cookie", "session=fixture; Path=/; HttpOnly");
                    }
                    else Require(route == "/api/management/supervisor/check" && request.Request.Cookies["session"]?.Value == "fixture", "Authenticated management route");
                    var bytes = Encoding.UTF8.GetBytes("{\"runningVersion\":\"9.1.0\",\"latestVersion\":\"9.1.1\",\"canUpdate\":true}");
                    await request.Response.OutputStream.WriteAsync(bytes); request.Response.Close();
                }
            });
            var status = await service.ManagementAsync("/check", true, CancellationToken.None); await served.WaitAsync(TimeSpan.FromSeconds(10));
            Require(status.GetProperty("latestVersion").GetString() == "9.1.1", "Management response");
            Require(File.ReadAllText(database) == "preserve-device-and-history", "History preserved");
            var update = Path.Combine(AppPaths.RelayDataRoot, "updates"); Directory.CreateDirectory(update); File.WriteAllText(Path.Combine(update, "supervisor-update.json"), "{\"phase\":\"restarting\"}");
            Require(RelaySupervisorService.MaintenancePending(), "No competing restart during maintenance");
            var redacted = AppLogger.Redact("POCKYMOE_ENROLL_SECRET=synthetic-secret TOKEN=synthetic-token rcd_synthetic Authorization synthetic-auth");
            Require(!redacted.Contains("synthetic-secret") && !redacted.Contains("synthetic-token") && !redacted.Contains("synthetic-auth"), "Secret redaction");
            return 0;
        }
        catch (Exception e) { new AppLogger().Error("Self-test failed", e); return 1; }
    }
    public static async Task<int> RunNativeAsync(string binary)
    {
        System.Diagnostics.Process? relay = null;
        var logger = new AppLogger(); var runner = new ProcessRunner(logger);
        var service = new RelaySupervisorService(runner, logger);
        RuntimeState? runtime = null;
        try
        {
            Require(AppPaths.IsTestEnvironment, "Native tests require an isolated root.");
            AppPaths.EnsureDirectories(); Directory.CreateDirectory(AppPaths.RelayDataRoot);
            static int FreePort() { var listener = new TcpListener(IPAddress.Loopback, 0); listener.Start(); var port = ((IPEndPoint)listener.LocalEndpoint).Port; listener.Stop(); return port; }
            var relayPort = FreePort(); var devicePort = FreePort();
            var password = Guid.NewGuid().ToString("N");
            var start = new System.Diagnostics.ProcessStartInfo(binary) { UseShellExecute = false, CreateNoWindow = true };
            start.ArgumentList.Add("relay");
            foreach (var key in start.Environment.Keys.ToArray())
                if (key.StartsWith("POCKYMOE_") || key.StartsWith("REMOTE_CODEX_")) start.Environment.Remove(key);
            start.Environment["HOME"] = AppPaths.UserHome; start.Environment["USERPROFILE"] = AppPaths.UserHome;
            start.Environment["HOST"] = "127.0.0.1"; start.Environment["PORT"] = relayPort.ToString();
            start.Environment["POCKYMOE_RELAY_DATA_DIR"] = Path.Combine(AppPaths.UserHome, "fixture-relay");
            start.Environment["POCKYMOE_ADMIN_USERNAME"] = "fixtureadmin"; start.Environment["POCKYMOE_ADMIN_PASSWORD"] = password;
            start.Environment["POCKYMOE_RELAY_REGISTRATION_ENABLED"] = "true";
            start.Environment["POCKYMOE_RELAY_SESSION_SECRET"] = Guid.NewGuid().ToString("N");
            relay = System.Diagnostics.Process.Start(start)!;
            using var client = new HttpClient(new HttpClientHandler { UseProxy = false }) { Timeout = TimeSpan.FromSeconds(15) };
            var origin = $"http://127.0.0.1:{relayPort}";
            for (var i = 0; i < 60; i++)
            {
                try { using var health = await client.GetAsync(origin + "/healthz"); if (health.IsSuccessStatusCode) break; } catch (HttpRequestException) { }
                await Task.Delay(500);
            }
            async Task<JsonElement> Post(string route, object data)
            {
                using var response = await client.PostAsync(origin + route, new StringContent(JsonSerializer.Serialize(data), Encoding.UTF8, "application/json"));
                Require(response.IsSuccessStatusCode, $"Fixture Relay HTTP {(int)response.StatusCode} at {route}");
                using var json = JsonDocument.Parse(await response.Content.ReadAsStringAsync()); return json.RootElement.Clone();
            }
            static string B64(byte[] data) => Convert.ToBase64String(data).TrimEnd('=').Replace('+', '-').Replace('/', '_');
            static string RandomKey(int count) => B64(System.Security.Cryptography.RandomNumberGenerator.GetBytes(count));
            using var account = System.Security.Cryptography.ECDsa.Create(System.Security.Cryptography.ECCurve.NamedCurves.nistP256);
            var publicKey = B64(account.ExportSubjectPublicKeyInfo());
            var userId = Guid.NewGuid().ToString(); var authKey = RandomKey(32);
            var encrypted = new { iv = RandomKey(12), ciphertext = RandomKey(32) };
            // The Relay stores opaque ciphertext; the fixture does not need to decrypt it.
            await Post("/relay/auth/register", new
            {
                userId,
                username = "fixtureowner",
                email = "fixtureowner@example.test",
                authKey,
                recoveryAuthKey = RandomKey(32),
                vault = new
                {
                    version = 1,
                    revision = 1,
                    userId,
                    accountPublicKey = publicKey,
                    accountKeyId = B64(System.Security.Cryptography.SHA256.HashData(account.ExportSubjectPublicKeyInfo())),
                    kdf = new { name = "argon2id", memoryKiB = 65536, iterations = 3, parallelism = 1, salt = RandomKey(16) },
                    slots = new { password = encrypted, recovery = encrypted },
                    body = encrypted
                }
            });
            var login = await Post("/relay/auth/login", new { username = "fixtureowner", authKey });
            client.DefaultRequestHeaders.Authorization = new System.Net.Http.Headers.AuthenticationHeaderValue("Bearer", login.GetProperty("token").GetString());
            var device = await Post("/relay/devices", new { name = "Isolated Windows Manager acceptance" });
            var token = device.GetProperty("token").GetString()!;
            var database = Path.Combine(AppPaths.RelayDataRoot, "retained-device.sqlite");
            var saved = new Dictionary<string, object>
            {
                ["REMOTE_CODEX_RELAY_SERVER_URL"] = origin.Replace("http:", "ws:"),
                ["REMOTE_CODEX_RELAY_AGENT_TOKEN"] = token,
                ["REMOTE_CODEX_RELAY_SUPERVISOR_PORT"] = devicePort.ToString(),
                ["REMOTE_CODEX_DATABASE_PATH"] = database,
                ["REMOTE_CODEX_ADMIN_USERNAME"] = "admin",
                ["REMOTE_CODEX_ADMIN_PASSWORD"] = Guid.NewGuid().ToString("N"),
                ["REMOTE_CODEX_SESSION_SECRET"] = Guid.NewGuid().ToString("N"),
                ["retainedCustomSetting"] = "keep"
            };
            File.WriteAllText(AppPaths.RelayConfigPath, JsonSerializer.Serialize(saved));
            var configuration = new DeviceConfiguration(origin, token, AppPaths.UserHome, devicePort, AccountKey: publicKey, EnrollmentSecret: RandomKey(32), WebOrigin: origin);
            logger.Info("Native acceptance: verified account-bound setup.");
            // Retain the isolated launch template before WMI consumes it, for failure diagnostics.
            var diagnostics = Path.Combine(AppPaths.RelayDataRoot, "manager-startup-diagnostic");
            Directory.CreateDirectory(diagnostics); Directory.CreateDirectory(Path.GetDirectoryName(AppPaths.RelayLogPath)!);
            using var capture = new CancellationTokenSource();
            using var setupDeadline = new CancellationTokenSource(TimeSpan.FromMinutes(2));
            var monitor = Task.Run(async () =>
            {
                while (!capture.IsCancellationRequested)
                {
                    foreach (var file in Directory.GetFiles(Path.GetDirectoryName(AppPaths.RelayLogPath)!, "*.launch.*"))
                        try { File.Copy(file, Path.Combine(diagnostics, Path.GetFileName(file)), true); } catch (IOException) { }
                    try { await Task.Delay(50, capture.Token); } catch (OperationCanceledException) { break; }
                }
            });
            try { runtime = await new RuntimeProvisioner(logger, runner).SetupAsync(configuration, new Progress<ProvisioningProgress>(), setupDeadline.Token); }
            finally { capture.Cancel(); await monitor; }
            var state = await service.GetStateAsync(runtime, CancellationToken.None); Require(state == SupervisorState.Running, "Native setup connects Relay");
            logger.Info("Native acceptance: apply workspace through maintenance.");
            await service.ConfigureWorkspaceAsync(AppPaths.UserHome, CancellationToken.None);
            Require(RelaySupervisorService.Saved("POCKYMOE_WORKSPACE_ROOT") == AppPaths.UserHome, "Workspace preference applied");
            var status = await service.ManagementAsync("/check", true, CancellationToken.None);
            Require(status.GetProperty("canUpdate").GetBoolean(), "Native updates available");
            logger.Info("Native acceptance: authenticated Check and restart.");
            var beforePid = (await service.ManagementAsync("", false, CancellationToken.None)).GetProperty("processId").GetInt32();
            var identity = Directory.GetFiles(AppPaths.RelayDataRoot, "*.transport-identity").Single();
            var identityBytes = File.ReadAllBytes(identity);
            await service.ManagementAsync("/restart", true, CancellationToken.None);
            var restarted = false;
            for (var i = 0; i < 120; i++)
            {
                await Task.Delay(1000);
                try { var current = await service.ManagementAsync("", false, CancellationToken.None); if (current.GetProperty("processId").GetInt32() != beforePid && current.GetProperty("job").GetProperty("phase").GetString() == "completed") { restarted = true; break; } }
                catch (Exception e) when (e is HttpRequestException or TaskCanceledException) { }
            }
            Require(restarted, "Management restart completes");
            Require(File.ReadAllBytes(identity).SequenceEqual(identityBytes), "Device identity preserved after maintenance");
            await service.StopAsync(runtime, CancellationToken.None);
            // Prove relaunch follows current.json, rather than the stale RuntimeState binary.
            var installed = RuntimeState.ResolveNative()!; var next = Path.Combine(Path.GetDirectoryName(installed.Executable)!, "next-launch.exe");
            File.Copy(installed.Executable, next, true);
            File.WriteAllText(AppPaths.NativeCurrentPath, JsonSerializer.Serialize(installed with { Executable = next }, RuntimeState.JsonOptions));
            logger.Info("Native acceptance: reconnect using current.json.");
            await service.StartAsync(runtime, configuration, CancellationToken.None);
            var relaunched = await service.ManagementAsync("", false, CancellationToken.None);
            Require(string.Equals(relaunched.GetProperty("path").GetString(), next, StringComparison.OrdinalIgnoreCase), "Launch follows installed current.json");
            Require(RelaySupervisorService.Saved("POCKYMOE_DATABASE_PATH") == database && RelaySupervisorService.Saved("retainedCustomSetting") == "keep", "Legacy config/database retained");
            Require(File.ReadAllBytes(identity).SequenceEqual(identityBytes), "Device identity retained on tray reconnect");
            await service.StopAsync(runtime, CancellationToken.None); runtime = null;
            logger.Info("Native acceptance passed: setup, account binding, auth, Check, restart, reconnect, identity preservation.");
            return 0;
        }
        catch (Exception e)
        {
            logger.Error("Native acceptance failed", e);
            var diagnostics = Path.Combine(AppPaths.RelayDataRoot, "manager-startup-diagnostic");
            logger.Warning($"Isolated native startup log bytes: {(File.Exists(AppPaths.RelayLogPath) ? new FileInfo(AppPaths.RelayLogPath).Length : -1)}");
            if (Directory.Exists(diagnostics))
                foreach (var launcher in Directory.GetFiles(diagnostics, "*.launch.ps1").Take(1))
                {
                    try
                    {
                        var payload = Path.ChangeExtension(launcher, ".json");
                        var source = File.ReadAllText(launcher);
                        var originalPayload = System.Text.RegularExpressions.Regex.Match(source, @"Get-Content -LiteralPath '(?<payload>(?:[^']|'')+)'").Groups["payload"].Value;
                        Require(originalPayload.Length > 0, "Diagnostic launcher payload");
                        source = source.Replace(originalPayload, payload.Replace("'", "''"));
                        File.WriteAllText(launcher, source);
                        var diagnostic = await runner.RunAsync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", launcher], TimeSpan.FromSeconds(15));
                        logger.Warning("Isolated launcher diagnostic: " + System.Text.RegularExpressions.Regex.Replace(diagnostic.CombinedOutput, @"[A-Za-z0-9_+/=-]{24,}", "[opaque]"));
                    }
                    catch (Exception diagnostic) { logger.Warning("Isolated launcher diagnostic: " + diagnostic.GetType().Name); }
                }
            if (File.Exists(AppPaths.RelayLogPath))
                foreach (var line in File.ReadLines(AppPaths.RelayLogPath).TakeLast(20))
                    logger.Warning("Isolated Supervisor: " + line);
            return 1;
        }
        finally
        {
            if (runtime is not null) try { await service.StopAsync(runtime, CancellationToken.None); } catch { }
            if (relay is not null) { if (!relay.HasExited) relay.Kill(true); relay.Dispose(); }
        }
    }

    public static int RenderPreview(string outputPath)
    {
        try
        {
            Require(AppPaths.IsTestEnvironment, "Preview requires a test root.");
            ApplicationConfiguration.Initialize();
            var logger = new AppLogger(); var runner = new ProcessRunner(logger);
            var directory = Path.GetDirectoryName(Path.GetFullPath(outputPath))!; Directory.CreateDirectory(directory);
            foreach (var theme in new[] { "classic-light", "classic-dark", "plum-light", "plum-dark" })
                foreach (var scale in new[] { 1F, 1.5F, 2F })
                {
                    var controller = new DeviceManagerController(logger, new RuntimeProvisioner(logger, runner), new RelaySupervisorService(runner, logger), new StartupRegistrationService());
                    controller.SetTheme(theme);
                    controller.InitializeAsync().GetAwaiter().GetResult();
                    using var form = new MainForm(controller);
                    form.StartPosition = FormStartPosition.Manual; form.Location = new Point(-20000, -20000); form.ShowInTaskbar = false;
                    form.Scale(new SizeF(scale, scale)); form.Show(); Application.DoEvents(); form.PerformLayout(); form.Refresh(); Application.DoEvents();
                    using var bitmap = new Bitmap(form.Width, form.Height); form.DrawToBitmap(bitmap, new Rectangle(Point.Empty, form.Size));
                    bitmap.Save(Path.Combine(directory, $"manager-{theme}-{scale * 100:0}.png"), System.Drawing.Imaging.ImageFormat.Png);
                    foreach (var child in Descendants(form).Where(c => c.Visible && c.Parent is TableLayoutPanel))
                        Require(child.Left >= -1 && child.Right <= child.Parent!.ClientSize.Width + 1, $"Horizontal overflow ({theme}, {scale}): {child.GetType().Name} {child.Left}/{child.Right} parent {child.Parent!.ClientSize.Width}");
                    foreach (var button in Descendants(form).OfType<Button>().Where(b => b.Visible))
                    {
                        Require(button.Width >= 40 * scale && button.Height >= 20 * scale, "Action too small");
                        if (button.Parent is FlowLayoutPanel actions)
                            Require(button.Bottom <= actions.ClientSize.Height && button.Left >= 0, "Primary action clipped");
                    }
                    Require(Descendants(form).OfType<TextBox>().Single(t => t.AccessibleName == "Device token").UseSystemPasswordChar, "Token masked");
                    if (scale == 1F)
                    {
                        Descendants(form).OfType<Button>().Single(b => b.Text.StartsWith("Connection details")).PerformClick();
                        form.PerformLayout(); Application.DoEvents();
                        using var expanded = new Bitmap(form.Width, form.Height);
                        form.DrawToBitmap(expanded, new Rectangle(Point.Empty, form.Size));
                        expanded.Save(Path.Combine(directory, $"manager-{theme}-details.png"), System.Drawing.Imaging.ImageFormat.Png);
                        foreach (var child in Descendants(form).Where(c => c.Visible && c.Parent is TableLayoutPanel))
                            Require(child.Left >= -1 && child.Right <= child.Parent!.ClientSize.Width + 1, "Expanded connection details overflow");
                    }
                    form.Hide();
                }
            return 0;
        }
        catch (Exception e) { new AppLogger().Error("Preview failed", e); return 1; }
    }
    private static IEnumerable<Control> Descendants(Control root)
    {
        foreach (Control child in root.Controls) { yield return child; foreach (var next in Descendants(child)) yield return next; }
    }
}

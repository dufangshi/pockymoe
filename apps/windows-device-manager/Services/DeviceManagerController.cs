using RemoteCodex.DeviceManager.Infrastructure;
using RemoteCodex.DeviceManager.Models;

namespace RemoteCodex.DeviceManager.Services;

internal sealed class DeviceManagerController
{
    private readonly AppLogger _logger;
    private readonly RuntimeProvisioner _provisioner;
    private readonly RelaySupervisorService _supervisor;
    private readonly StartupRegistrationService _startup;
    private readonly SemaphoreSlim _operationLock = new(1, 1);

    public DeviceManagerController(
        AppLogger logger,
        RuntimeProvisioner provisioner,
        RelaySupervisorService supervisor,
        StartupRegistrationService startup)
    {
        _logger = logger;
        _provisioner = provisioner;
        _supervisor = supervisor;
        _startup = startup;
        Settings = LoadSettings();
        RelayConfiguration = RelayConfigurationSnapshot.Load(logger);
        Runtime = RuntimeState.Load(logger);
        Progress = new ProvisioningProgress("Ready", "Waiting for configuration.", ProvisioningStepState.Pending, 0);
    }

    public event EventHandler? Changed;

    public AppSettings Settings { get; private set; }
    public RelayConfigurationSnapshot RelayConfiguration { get; private set; }
    public RuntimeState? Runtime { get; private set; }
    public ProvisioningProgress Progress { get; private set; }
    public SupervisorState State { get; private set; } = SupervisorState.Unknown;
    public string? LastError { get; private set; }
    public bool IsBusy { get; private set; }
    public bool HasSavedToken => RelayConfiguration.HasToken;
    public string? AvailableRemoteCodexVersion { get; private set; }
    public string RuntimeUpdateMessage { get; private set; } = "Updates come from GitHub Releases.";
    private bool RequiresNativeMigration => Runtime?.IsNative == true
        && !RuntimeProvisioner.SupportsNativeMaintenance(RuntimeState.ResolveNative()?.Version ?? Runtime.RemoteCodexVersion ?? "0.0.0");
    private const string NativeMigrationMessage = "Older runtime: Disconnect, then Connect to migrate safely.";
    public string CurrentRemoteCodexVersion => Runtime?.RemoteCodexVersion
        ?? ProductManifest.RemoteCodexVersion;

    public async Task InitializeAsync(CancellationToken cancellationToken = default)
    {
        if (Runtime?.IsUsable != true)
        {
            Runtime = await _provisioner.TryRecoverBundledRuntimeAsync(cancellationToken);
        }
        if (Runtime?.IsUsable == true)
        {
            State = await _supervisor.GetStateAsync(Runtime, cancellationToken);
        }
        else
        {
            State = SupervisorState.Stopped;
        }
        Progress = State switch
        {
            SupervisorState.Running => new("Running", "Supervisor is connected to the Relay.", ProvisioningStepState.Complete, 100),
            SupervisorState.Starting => new("Connection", "Supervisor is reconnecting to the Relay…", ProvisioningStepState.Running, 80),
            _ => new("Ready", HasSavedToken ? "Saved registration is ready to connect." : "Waiting for a device setup command.", ProvisioningStepState.Pending, 0)
        };
        if (RequiresNativeMigration) RuntimeUpdateMessage = NativeMigrationMessage;
        OnChanged();

        if (Settings.KeepOnline && HasSavedToken && Runtime?.IsNative == true && Runtime.IsUsable && State != SupervisorState.Running)
        {
            await MaintainConnectionAsync(cancellationToken);
        }
    }

    public async Task ConnectAsync(
        DeviceConfiguration configuration,
        bool startWithWindows,
        CancellationToken cancellationToken = default)
    {
        var errors = configuration.Validate(HasSavedToken);
        if (errors.Count > 0)
        {
            throw new ArgumentException(string.Join(Environment.NewLine, errors));
        }
        EnsureWorkspaceIsWritable(configuration.WorkspaceRoot);

        await _operationLock.WaitAsync(cancellationToken);
        try
        {
            SetBusy(true);
            LastError = null;
            State = SupervisorState.Starting;
            Report(new("Setup", "Preparing this Windows device...", ProvisioningStepState.Running, 2));

            if (RequiresNativeMigration && Runtime?.IsUsable == true
                && await _supervisor.GetStateAsync(Runtime, cancellationToken) != SupervisorState.Stopped)
                throw new InvalidOperationException("Disconnect this older Windows runtime before migrating. Device registration and history will be retained.");
            if (Runtime?.IsNative == true && !RequiresNativeMigration && Runtime.IsUsable && string.IsNullOrEmpty(configuration.DeviceToken) && string.IsNullOrEmpty(configuration.SetupCode))
            {
                if (!string.Equals(configuration.HttpRelayUrl, ConfigurationFromSettings().HttpRelayUrl, StringComparison.OrdinalIgnoreCase)
                    || configuration.SupervisorPort != Settings.SupervisorPort)
                    throw new InvalidOperationException("Paste a setup command to change device registration. Saved registration will not be overwritten.");
                await _supervisor.StartAsync(Runtime, configuration, cancellationToken);
            }
            else
            {
                if (Runtime is { IsNative: false, IsUsable: true }
                    && await _supervisor.GetStateAsync(Runtime, cancellationToken) == SupervisorState.Running)
                    throw new InvalidOperationException("Disconnect the legacy device before migrating. Its configuration and data will be retained.");
                if (string.IsNullOrEmpty(configuration.DeviceToken) && string.IsNullOrEmpty(configuration.SetupCode))
                    configuration = configuration with { DeviceToken = RelaySupervisorService.Saved("POCKYMOE_RELAY_AGENT_TOKEN") };
                Runtime = await _provisioner.SetupAsync(configuration, new Progress<ProvisioningProgress>(Report), cancellationToken);
            }

            await _supervisor.ConfigureWorkspaceAsync(configuration.WorkspaceRoot, cancellationToken);
            Settings = Settings with
            {
                RelayUrl = configuration.RelayUrl.Trim(),
                WorkspaceRoot = Path.GetFullPath(configuration.WorkspaceRoot.Trim()),
                SupervisorPort = configuration.SupervisorPort,
                StartWithWindows = startWithWindows,
                KeepOnline = true,
            };
            Settings.Save();
            _startup.SetEnabled(startWithWindows);
            RelayConfiguration = RelayConfigurationSnapshot.Load(_logger);
            State = SupervisorState.Running;
            RuntimeUpdateMessage = "Updates come from GitHub Releases.";
            Report(new("Running", "Relay Supervisor is running and maintaining the outbound tunnel.", ProvisioningStepState.Complete, 100));
            _logger.Info("Device connected successfully.");
        }
        catch (OperationCanceledException)
        {
            State = SupervisorState.Stopped;
            Report(new("Cancelled", "Setup was cancelled.", ProvisioningStepState.Pending, 0));
            throw;
        }
        catch (Exception exception)
        {
            LastError = AppLogger.Redact(exception.Message);
            State = SupervisorState.Error;
            Report(new("Failed", LastError, ProvisioningStepState.Failed, Progress.Percent));
            _logger.Error("Device connection failed", exception);
            throw;
        }
        finally
        {
            SetBusy(false);
            _operationLock.Release();
        }
    }

    public async Task DisconnectAsync(CancellationToken cancellationToken = default)
    {
        await _operationLock.WaitAsync(cancellationToken);
        try
        {
            SetBusy(true);
            LastError = null;
            if (Runtime?.IsUsable == true)
            {
                await _supervisor.StopAsync(Runtime, cancellationToken);
            }
            Settings = Settings with { KeepOnline = false };
            Settings.Save();
            State = SupervisorState.Stopped;
            Report(new("Offline", "This device is disconnected.", ProvisioningStepState.Pending, 0));
            _logger.Info("Device disconnected by the user.");
        }
        catch (Exception exception)
        {
            LastError = AppLogger.Redact(exception.Message);
            State = SupervisorState.Error;
            _logger.Error("Device disconnect failed", exception);
            throw;
        }
        finally
        {
            SetBusy(false);
            _operationLock.Release();
        }
    }

    public async Task CheckRemoteCodexUpdateAsync(CancellationToken cancellationToken = default)
    {
        await _operationLock.WaitAsync(cancellationToken);
        try
        {
            SetBusy(true);
            AvailableRemoteCodexVersion = null;
            RuntimeUpdateMessage = "Checking GitHub Releases…";
            OnChanged();

            if (Runtime?.IsNative != true) throw new InvalidOperationException("Migrate this legacy device using a setup command from the Devices page.");
            Runtime = RuntimeState.FromNative() ?? Runtime;
            if (RequiresNativeMigration)
            {
                RuntimeUpdateMessage = NativeMigrationMessage; OnChanged(); return;
            }
            var status = await _supervisor.ManagementAsync("/check", true, cancellationToken);
            if (status.TryGetProperty("canUpdate", out var allowed) && allowed.ValueKind == System.Text.Json.JsonValueKind.False)
                throw new InvalidOperationException(status.GetProperty("reason").GetString());
            var latestVersion = status.GetProperty("latestVersion").GetString()!;
            if (RuntimeProvisioner.IsNewerVersion(latestVersion, CurrentRemoteCodexVersion))
            {
                AvailableRemoteCodexVersion = latestVersion;
                RuntimeUpdateMessage = $"Pockymoe {latestVersion} is available.";
            }
            else
            {
                RuntimeUpdateMessage = $"Pockymoe {CurrentRemoteCodexVersion} is up to date.";
            }
            _logger.Info("Pockymoe update check completed.");
            OnChanged();
        }
        catch (Exception exception)
        {
            RuntimeUpdateMessage = $"Update check failed: {AppLogger.Redact(exception.Message)}";
            _logger.Error("Pockymoe update check failed", exception);
            OnChanged();
            throw;
        }
        finally
        {
            SetBusy(false);
            _operationLock.Release();
        }
    }

    public async Task UpdateRemoteCodexAsync(CancellationToken cancellationToken = default)
    {
        if (Runtime?.IsNative != true || RequiresNativeMigration || string.IsNullOrEmpty(AvailableRemoteCodexVersion))
            throw new InvalidOperationException("Start a native device and check for updates first.");
        await _operationLock.WaitAsync(cancellationToken);
        try
        {
            SetBusy(true);
            var accepted = await _supervisor.ManagementAsync("/update", true, cancellationToken);
            if (accepted.TryGetProperty("canUpdate", out var allowed) && allowed.ValueKind == System.Text.Json.JsonValueKind.False)
                throw new InvalidOperationException(accepted.GetProperty("reason").GetString());
            State = SupervisorState.Starting;
            Report(new("Update", "Supervisor is updating; tasks will resume after restart…", ProvisioningStepState.Running, 70));
            for (var i = 0; i < 600; i++)
            {
                await Task.Delay(1000, cancellationToken);
                System.Text.Json.JsonElement status;
                try { status = await _supervisor.ManagementAsync("", false, cancellationToken); }
                catch (Exception e) when (e is HttpRequestException or TaskCanceledException) { continue; }
                if (status.TryGetProperty("job", out var job) && job.TryGetProperty("phase", out var phase))
                {
                    if (phase.GetString() == "failed") throw new InvalidOperationException("Supervisor update failed. Open Web Settings for the recovery details.");
                    if (phase.GetString() != "completed") continue;
                }
                if (status.GetProperty("runningVersion").GetString() != AvailableRemoteCodexVersion) continue;
                Runtime = RuntimeState.FromNative() ?? Runtime;
                Runtime.Save(); AvailableRemoteCodexVersion = null;
                State = await _supervisor.GetStateAsync(Runtime, cancellationToken);
                RuntimeUpdateMessage = $"Pockymoe {Runtime.RemoteCodexVersion} is running.";
                Report(new("Updated", RuntimeUpdateMessage, ProvisioningStepState.Complete, 100));
                return;
            }
            throw new TimeoutException("Update is still pending. Follow its progress in Web Settings.");
        }
        catch (Exception e)
        {
            LastError = AppLogger.Redact(e.Message); RuntimeUpdateMessage = LastError;
            OnChanged(); throw;
        }
        finally { SetBusy(false); _operationLock.Release(); }
    }

    public async Task MaintainConnectionAsync(CancellationToken cancellationToken = default)
    {
        if (IsBusy || !Settings.KeepOnline || !HasSavedToken || Runtime?.IsUsable != true || !Runtime.IsNative)
        {
            return;
        }

        if (!await _operationLock.WaitAsync(0, cancellationToken))
        {
            return;
        }

        try
        {
            Runtime = RuntimeState.FromNative() ?? Runtime;
            if (RelaySupervisorService.MaintenancePending())
            {
                if (State != SupervisorState.Starting)
                {
                    State = SupervisorState.Starting;
                    Report(new("Maintenance", "Supervisor maintenance is in progress; waiting for recovery…", ProvisioningStepState.Running, 70));
                }
                return;
            }
            var state = await _supervisor.GetStateAsync(Runtime, cancellationToken);
            if (state == SupervisorState.Starting || state == SupervisorState.Unknown)
            {
                if (State != state)
                {
                    State = state;
                    Report(new("Connection", state == SupervisorState.Starting ? "Supervisor is reconnecting to the Relay…" : "Waiting for the local Supervisor to respond…", ProvisioningStepState.Running, 80));
                }
                return;
            }
            if (state == SupervisorState.Running)
            {
                if (State != SupervisorState.Running)
                {
                    State = SupervisorState.Running;
                    LastError = null;
                    OnChanged();
                }
                return;
            }

            if (RequiresNativeMigration)
            {
                State = SupervisorState.Stopped; RuntimeUpdateMessage = NativeMigrationMessage;
                Report(new("Ready", "Connect to migrate the saved Windows device.", ProvisioningStepState.Pending, 0));
                return;
            }
            State = SupervisorState.Starting;
            Report(new("Reconnect", "The Supervisor stopped; reconnecting...", ProvisioningStepState.Running, 90));
            var configuration = new DeviceConfiguration(
                Settings.RelayUrl,
                string.Empty,
                Settings.WorkspaceRoot,
                Settings.SupervisorPort);
            await _supervisor.StartAsync(Runtime, configuration, cancellationToken);
            State = SupervisorState.Running;
            LastError = null;
            Report(new("Running", "Relay Supervisor is running and maintaining the outbound tunnel.", ProvisioningStepState.Complete, 100));
            _logger.Info("Device connection was restored automatically.");
        }
        catch (Exception exception)
        {
            State = SupervisorState.Error;
            LastError = AppLogger.Redact(exception.Message);
            _logger.Error("Automatic reconnect failed", exception);
            OnChanged();
        }
        finally
        {
            _operationLock.Release();
        }
    }

    public void SetTheme(string theme)
    {
        Settings = Settings with { Theme = theme }; Settings.Save(); OnChanged();
    }

    public void SetStartWithWindows(bool enabled)
    {
        _startup.SetEnabled(enabled);
        Settings = Settings with { StartWithWindows = enabled };
        Settings.Save();
        OnChanged();
    }

    public async Task StopForExitAsync(CancellationToken cancellationToken = default)
    {
        await _operationLock.WaitAsync(cancellationToken);
        try
        {
            if (Runtime?.IsUsable == true)
            {
                await _supervisor.StopAsync(Runtime, cancellationToken);
            }
            State = SupervisorState.Stopped;
            OnChanged();
        }
        finally
        {
            _operationLock.Release();
        }
    }

    private AppSettings LoadSettings()
    {
        var hasSettings = File.Exists(AppPaths.SettingsPath);
        var settings = AppSettings.Load(_logger);
        var relay = RelayConfigurationSnapshot.Load(_logger);
        if (hasSettings)
        {
            return settings with
            {
                RelayUrl = relay.RelayUrl ?? settings.RelayUrl,
                WorkspaceRoot = relay.WorkspaceRoot ?? settings.WorkspaceRoot,
                SupervisorPort = relay.SupervisorPort ?? settings.SupervisorPort,
                StartWithWindows = _startup.IsEnabled()
            };
        }

        return settings with
        {
            RelayUrl = relay.RelayUrl ?? settings.RelayUrl,
            WorkspaceRoot = relay.WorkspaceRoot ?? settings.WorkspaceRoot,
            SupervisorPort = relay.SupervisorPort ?? settings.SupervisorPort,
            StartWithWindows = _startup.IsEnabled() || settings.StartWithWindows,
            KeepOnline = relay.HasToken,
        };
    }

    private DeviceConfiguration ConfigurationFromSettings() => new(
        Settings.RelayUrl,
        string.Empty,
        Settings.WorkspaceRoot,
        Settings.SupervisorPort);

    private static void EnsureWorkspaceIsWritable(string workspaceRoot)
    {
        var fullPath = Path.GetFullPath(workspaceRoot.Trim());
        Directory.CreateDirectory(fullPath);
        var probePath = Path.Combine(fullPath, $".remote-codex-write-test-{Environment.ProcessId}");
        try
        {
            File.WriteAllText(probePath, string.Empty);
        }
        catch (Exception exception)
        {
            throw new InvalidOperationException($"Workspace root is not writable: {fullPath}", exception);
        }
        finally
        {
            File.Delete(probePath);
        }
    }

    private void Report(ProvisioningProgress progress)
    {
        Progress = progress;
        OnChanged();
    }

    private void SetBusy(bool busy)
    {
        IsBusy = busy;
        OnChanged();
    }

    private void OnChanged() => Changed?.Invoke(this, EventArgs.Empty);
}

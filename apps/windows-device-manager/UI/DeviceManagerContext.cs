using System.Diagnostics;
using RemoteCodex.DeviceManager.Infrastructure;
using RemoteCodex.DeviceManager.Models;
using RemoteCodex.DeviceManager.Services;

namespace RemoteCodex.DeviceManager.UI;

internal sealed class DeviceManagerContext : ApplicationContext
{
    private readonly AppLogger _logger;
    private readonly bool _background;
    private readonly DeviceManagerController _controller;
    private readonly MainForm _mainForm;
    private readonly NotifyIcon _trayIcon;
    private readonly ToolStripMenuItem _trayStatusItem;
    private readonly ToolStripMenuItem _connectItem;
    private readonly ToolStripMenuItem _disconnectItem;
    private readonly ToolStripMenuItem _startWithWindowsItem;
    private readonly System.Windows.Forms.Timer _monitorTimer;
    private bool _initialized;
    private bool _exiting;
    private SupervisorState _lastTrayState = SupervisorState.Unknown;

    public DeviceManagerContext(AppLogger logger, bool background)
    {
        _logger = logger;
        _background = background;
        var runner = new ProcessRunner(logger);
        var provisioner = new RuntimeProvisioner(logger, runner);
        var supervisor = new RelaySupervisorService(runner, logger);
        var startup = new StartupRegistrationService();
        _controller = new DeviceManagerController(logger, provisioner, supervisor, startup);
        _mainForm = new MainForm(_controller);
        _controller.Changed += ControllerChanged;

        _trayStatusItem = new ToolStripMenuItem("Checking status...") { Enabled = false };
        _connectItem = new ToolStripMenuItem("Connect", null, ConnectFromTray);
        _disconnectItem = new ToolStripMenuItem("Disconnect", null, DisconnectFromTray);
        _startWithWindowsItem = new ToolStripMenuItem("Start with Windows")
        {
            CheckOnClick = true,
            Checked = _controller.Settings.StartWithWindows,
        };
        _startWithWindowsItem.CheckedChanged += StartWithWindowsChanged;

        var menu = new ContextMenuStrip();
        menu.Items.AddRange([
            _trayStatusItem,
            new ToolStripSeparator(),
            new ToolStripMenuItem("Open", null, (_, _) => ShowMainWindow()),
            _connectItem,
            _disconnectItem,
            new ToolStripMenuItem("Change device token...", null, ChangeToken),
            new ToolStripMenuItem("Open Relay portal", null, (_, _) => OpenRelayPortal()),
            new ToolStripSeparator(),
            new ToolStripMenuItem("Open logs", null, (_, _) => UI.MainForm.OpenLogs()),
            _startWithWindowsItem,
            new ToolStripSeparator(),
            new ToolStripMenuItem("Exit and take device offline", null, ExitRequested),
        ]);

        _trayIcon = new NotifyIcon
        {
            ContextMenuStrip = menu,
            Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath) ?? SystemIcons.Application,
            Text = ProductManifest.ProductName,
            Visible = true,
        };
        _trayIcon.DoubleClick += (_, _) => ShowMainWindow();

        _monitorTimer = new System.Windows.Forms.Timer { Interval = 10_000 };
        _monitorTimer.Tick += MonitorTick;

        Application.Idle += InitializeOnIdle;
    }

    public void ShowMainWindow()
    {
        if (_mainForm.IsDisposed)
        {
            return;
        }
        if (_mainForm.InvokeRequired)
        {
            _mainForm.BeginInvoke(ShowMainWindow);
            return;
        }

        _mainForm.Show();
        if (_mainForm.WindowState == FormWindowState.Minimized)
        {
            _mainForm.WindowState = FormWindowState.Normal;
        }
        _mainForm.Activate();
        _mainForm.BringToFront();
    }

    protected override void Dispose(bool disposing)
    {
        if (disposing)
        {
            _monitorTimer.Dispose();
            _trayIcon.Visible = false;
            _trayIcon.Dispose();
            _mainForm.Dispose();
        }
        base.Dispose(disposing);
    }

    private async void InitializeOnIdle(object? sender, EventArgs eventArgs)
    {
        if (_initialized)
        {
            return;
        }
        _initialized = true;
        Application.Idle -= InitializeOnIdle;

        try
        {
            await _controller.InitializeAsync();
        }
        catch (Exception exception)
        {
            _logger.Error("Device manager initialization failed", exception);
        }

        if (!_background || !_controller.HasSavedToken || _controller.Runtime?.IsUsable != true)
        {
            ShowMainWindow();
            if (!_controller.HasSavedToken)
            {
                _mainForm.FocusToken();
            }
        }
        _monitorTimer.Start();
        UpdateTrayState();
    }

    private async void ConnectFromTray(object? sender, EventArgs eventArgs)
    {
        if (!_controller.HasSavedToken)
        {
            ShowMainWindow();
            _mainForm.FocusToken();
            return;
        }

        try
        {
            var configuration = new DeviceConfiguration(
                _controller.Settings.RelayUrl,
                string.Empty,
                _controller.Settings.WorkspaceRoot,
                _controller.Settings.SupervisorPort);
            await _controller.ConnectAsync(
                configuration,
                _controller.Settings.StartWithWindows);
        }
        catch (OperationCanceledException)
        {
            // The main window already reflects cancellation.
        }
        catch (Exception exception)
        {
            ShowFailure("Connection failed", exception);
        }
    }

    private async void DisconnectFromTray(object? sender, EventArgs eventArgs)
    {
        try
        {
            await _controller.DisconnectAsync();
        }
        catch (Exception exception)
        {
            ShowFailure("Disconnect failed", exception);
        }
    }

    private async void ChangeToken(object? sender, EventArgs eventArgs)
    {
        string token;
        using (var dialog = new TokenDialog(_controller.Settings.Theme))
        {
            if (dialog.ShowDialog(_mainForm.Visible ? _mainForm : null) != DialogResult.OK)
            {
                return;
            }
            token = dialog.DeviceToken;
        }

        ShowMainWindow();
        await _mainForm.ApplyTokenAsync(token);
    }

    private void StartWithWindowsChanged(object? sender, EventArgs eventArgs)
    {
        if (!_initialized || _controller.IsBusy)
        {
            return;
        }
        try
        {
            _controller.SetStartWithWindows(_startWithWindowsItem.Checked);
        }
        catch (Exception exception)
        {
            _startWithWindowsItem.CheckedChanged -= StartWithWindowsChanged;
            _startWithWindowsItem.Checked = !_startWithWindowsItem.Checked;
            _startWithWindowsItem.CheckedChanged += StartWithWindowsChanged;
            ShowFailure("Startup setting failed", exception);
        }
    }

    private async void MonitorTick(object? sender, EventArgs eventArgs)
    {
        await _controller.MaintainConnectionAsync();
    }

    private void ControllerChanged(object? sender, EventArgs eventArgs)
    {
        if (_mainForm.IsDisposed)
        {
            return;
        }
        if (_mainForm.InvokeRequired)
        {
            _mainForm.BeginInvoke(UpdateTrayState);
        }
        else
        {
            UpdateTrayState();
        }
    }

    private void UpdateTrayState()
    {
        var status = _controller.State switch
        {
            SupervisorState.Running => "Supervisor running",
            SupervisorState.Starting => "Connecting device...",
            SupervisorState.Stopped => "Device offline",
            SupervisorState.Error => "Connection needs attention",
            _ => "Checking status...",
        };
        _trayStatusItem.Text = status;
        _connectItem.Enabled = !_controller.IsBusy && _controller.State != SupervisorState.Running;
        _disconnectItem.Enabled = !_controller.IsBusy && _controller.Runtime?.IsUsable == true && _controller.State != SupervisorState.Stopped;
        _startWithWindowsItem.CheckedChanged -= StartWithWindowsChanged;
        _startWithWindowsItem.Checked = _controller.Settings.StartWithWindows;
        _startWithWindowsItem.CheckedChanged += StartWithWindowsChanged;
        _trayIcon.Text = status.Length <= 63 ? status : status[..63];
        if (_initialized && _controller.State != _lastTrayState)
        {
            if (_controller.State == SupervisorState.Running && _lastTrayState != SupervisorState.Unknown)
            {
                _trayIcon.ShowBalloonTip(3_000, ProductManifest.ProductName, "Relay Supervisor is running.", ToolTipIcon.Info);
            }
            else if (_controller.State == SupervisorState.Error)
            {
                _trayIcon.ShowBalloonTip(5_000, ProductManifest.ProductName, "The device connection needs attention.", ToolTipIcon.Warning);
            }
        }
        _lastTrayState = _controller.State;
    }

    private async void ExitRequested(object? sender, EventArgs eventArgs)
    {
        if (_exiting)
        {
            return;
        }
        _exiting = true;
        _monitorTimer.Stop();
        _trayStatusItem.Text = "Taking device offline...";
        try
        {
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(35));
            await _controller.StopForExitAsync(timeout.Token);
        }
        catch (Exception exception)
        {
            _logger.Error("Unable to stop the device during exit", exception);
            var result = MessageBox.Show(
                _mainForm.Visible ? _mainForm : null,
                $"The Supervisor could not be stopped cleanly.\n\n{AppLogger.Redact(exception.Message)}\n\nExit anyway?",
                ProductManifest.ProductName,
                MessageBoxButtons.YesNo,
                MessageBoxIcon.Warning);
            if (result != DialogResult.Yes)
            {
                _exiting = false;
                _monitorTimer.Start();
                return;
            }
        }

        _trayIcon.Visible = false;
        _mainForm.AllowCloseAndExit();
        ExitThread();
    }

    private void ShowFailure(string title, Exception exception)
    {
        ShowMainWindow();
        MessageBox.Show(_mainForm, AppLogger.Redact(exception.Message), title, MessageBoxButtons.OK, MessageBoxIcon.Error);
    }

    private void OpenRelayPortal()
    {
        Process.Start(new ProcessStartInfo { FileName = ProductManifest.RelayPortalUri.AbsoluteUri, UseShellExecute = true });
    }
}

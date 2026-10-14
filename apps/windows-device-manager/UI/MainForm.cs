using System.Diagnostics;
using System.Runtime.InteropServices;
using RemoteCodex.DeviceManager.Infrastructure;
using RemoteCodex.DeviceManager.Models;
using RemoteCodex.DeviceManager.Services;

namespace RemoteCodex.DeviceManager.UI;

internal sealed class MainForm : Form
{
    private static readonly Color OnlineColor = Color.FromArgb(60, 181, 128);
    private static readonly Color ErrorColor = Color.FromArgb(222, 101, 129);
    private readonly DeviceManagerController _controller;
    private readonly TextBox _pasteConfigurationTextBox, _relayUrlTextBox, _tokenTextBox, _workspaceTextBox;
    private readonly Label _pasteStateLabel, _tokenStateLabel, _statusTitle, _statusDetail, _stepLabel, _runtimeValue, _runtimeUpdateLabel;
    private readonly NumericUpDown _portInput;
    private readonly CheckBox _startWithWindowsCheckBox;
    private readonly Button _connectButton, _disconnectButton, _checkUpdateButton, _updateButton;
    private readonly Panel _statusDot;
    private readonly ThemedProgress _progressBar;
    private ParsedRelayConfiguration? _parsed;
    private bool _allowClose, _isApplyingPaste;
    private string _themeName;

    public MainForm(DeviceManagerController controller)
    {
        _controller = controller; _themeName = controller.Settings.Theme;
        Text = ProductManifest.ProductName;
        StartPosition = FormStartPosition.CenterScreen;
        AutoScaleMode = AutoScaleMode.Dpi;
        Font = new Font("Segoe UI", 10F);
        ClientSize = new Size(760, 710); MinimumSize = new Size(700, 700);
        Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath) ?? SystemIcons.Application;
        var root = Grid(1, DockStyle.Fill); root.Padding = new Padding(24, 18, 24, 18);
        foreach (var height in new[] { 64, 84, 0, 50, 48 }) root.RowStyles.Add(new RowStyle(height == 0 ? SizeType.Percent : SizeType.Absolute, height == 0 ? 100 : height));
        var header = Grid(2, DockStyle.Fill); header.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100)); header.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 170));
        var brand = new Panel { Dock = DockStyle.Fill };
        brand.Controls.Add(new Label { Text = "Pockymoe", Font = new Font("Segoe UI", 22F, FontStyle.Bold), AutoSize = true, Location = Point.Empty });
        brand.Controls.Add(new Label { Text = "Windows Device Manager  ·  0.2.0", Tag = "muted", AutoSize = true, Location = new Point(2, 40) });
        var themes = new ComboBox { DropDownStyle = ComboBoxStyle.DropDownList, Dock = DockStyle.Top, AccessibleName = "Color theme", Margin = new Padding(0, 10, 0, 0) };
        themes.Items.AddRange(["Plum · Dark", "Plum · Light", "Classic · Dark", "Classic · Light"]);
        var names = new[] { "plum-dark", "plum-light", "classic-dark", "classic-light" };
        themes.SelectedIndex = Math.Max(0, Array.IndexOf(names, _themeName));
        themes.SelectedIndexChanged += (_, _) => { _themeName = names[themes.SelectedIndex]; _controller.SetTheme(_themeName); ManagerTheme.Apply(this, ManagerTheme.Get(_themeName)); UpdateState(); };
        header.Controls.Add(brand, 0, 0); header.Controls.Add(themes, 1, 0); root.Controls.Add(header, 0, 0);
        var status = new ThemedPanel { Dock = DockStyle.Fill, Margin = new Padding(0, 4, 0, 10) };
        var statusGrid = Grid(2, DockStyle.Fill); statusGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 28)); statusGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
        statusGrid.RowStyles.Add(new RowStyle(SizeType.Absolute, 24)); statusGrid.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
        _statusDot = new Panel { Size = new Size(10, 10), Anchor = AnchorStyles.None };
        _statusTitle = Label("Checking device…", true); _statusDetail = Label("", muted: true);
        statusGrid.Controls.Add(_statusDot, 0, 0); statusGrid.SetRowSpan(_statusDot, 2);
        statusGrid.Controls.Add(_statusTitle, 1, 0); statusGrid.Controls.Add(_statusDetail, 1, 1); status.Controls.Add(statusGrid); root.Controls.Add(status, 0, 1);
        var body = Grid(1, DockStyle.Fill); body.AutoScroll = true;
        foreach (var height in new[] { 28, 76, 28, 40, 36, 30, 0, 92 }) body.RowStyles.Add(new RowStyle(height == 0 ? SizeType.AutoSize : SizeType.Absolute, height));
        body.Controls.Add(Label("Connect this computer", true), 0, 0);
        var pasteGrid = Grid(2, DockStyle.Fill); pasteGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100)); pasteGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 108));
        _pasteConfigurationTextBox = CreateTextBox(); _pasteConfigurationTextBox.Multiline = true; _pasteConfigurationTextBox.UseSystemPasswordChar = false;
        _pasteConfigurationTextBox.WordWrap = true; _pasteConfigurationTextBox.ScrollBars = ScrollBars.Vertical;
        _pasteConfigurationTextBox.PlaceholderText = "Paste a Windows setup command from the Devices page"; _pasteConfigurationTextBox.AccessibleName = "Device setup command";
        _pasteConfigurationTextBox.TextChanged += (_, _) => TryApplyPastedConfiguration(false);
        var paste = CreateNeutralButton("Paste", 100); paste.Dock = DockStyle.Fill; paste.Click += PasteConfigurationClicked;
        pasteGrid.Controls.Add(_pasteConfigurationTextBox, 0, 0); pasteGrid.Controls.Add(paste, 1, 0); body.Controls.Add(pasteGrid, 0, 1);
        _pasteStateLabel = Label("Parsed locally. Commands are never executed.", muted: true); body.Controls.Add(_pasteStateLabel, 0, 2);
        var workspaceGrid = Grid(3, DockStyle.Fill); workspaceGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 110)); workspaceGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100)); workspaceGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 108));
        _workspaceTextBox = CreateTextBox(); _workspaceTextBox.AccessibleName = "Workspace folder";
        var browse = CreateNeutralButton("Browse", 100); browse.Dock = DockStyle.Fill; browse.Click += BrowseWorkspace;
        workspaceGrid.Controls.Add(Label("Workspace"), 0, 0); workspaceGrid.Controls.Add(_workspaceTextBox, 1, 0); workspaceGrid.Controls.Add(browse, 2, 0); body.Controls.Add(workspaceGrid, 0, 3);
        _startWithWindowsCheckBox = new CheckBox { Text = "Keep this device online when I sign in to Windows", AutoSize = true, Dock = DockStyle.Fill };
        body.Controls.Add(_startWithWindowsCheckBox, 0, 4);
        var manual = Grid(3, DockStyle.Top); manual.AutoSize = true; manual.Visible = false;
        manual.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 110)); manual.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100)); manual.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 108));
        foreach (var h in new[] { 36, 36, 26, 36 }) manual.RowStyles.Add(new RowStyle(SizeType.Absolute, h));
        _relayUrlTextBox = CreateTextBox(); _tokenTextBox = CreateTextBox(); _tokenTextBox.UseSystemPasswordChar = true; _tokenTextBox.AccessibleName = "Device token";
        _tokenTextBox.TextChanged += (_, _) => { if (!_isApplyingPaste) _parsed = null; };
        _tokenStateLabel = Label("", muted: true);
        _portInput = new NumericUpDown { Dock = DockStyle.Fill, Minimum = 1024, Maximum = 65535, AccessibleName = "Local port" };
        AddField(manual, "Relay", _relayUrlTextBox, 0); AddField(manual, "Token", _tokenTextBox, 1, false);
        var show = new CheckBox { Text = "Show", AutoSize = true, Dock = DockStyle.Fill }; show.CheckedChanged += (_, _) => _tokenTextBox.UseSystemPasswordChar = !show.Checked; manual.Controls.Add(show, 2, 1);
        manual.Controls.Add(_tokenStateLabel, 1, 2); manual.SetColumnSpan(_tokenStateLabel, 2); AddField(manual, "Local port", _portInput, 3);
        var advanced = CreateNeutralButton("Connection details  ▾", 200); advanced.Height = 28; advanced.Anchor = AnchorStyles.Left; advanced.Click += (_, _) => { manual.Visible = !manual.Visible; advanced.Text = manual.Visible ? "Connection details  ▴" : "Connection details  ▾"; };
        body.Controls.Add(advanced, 0, 5); body.Controls.Add(manual, 0, 6);
        var runtime = new ThemedPanel { Dock = DockStyle.Fill, Margin = new Padding(0, 8, 0, 0) };
        var runtimeGrid = Grid(3, DockStyle.Fill); runtimeGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100)); runtimeGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 92)); runtimeGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 92));
        runtimeGrid.RowStyles.Add(new RowStyle(SizeType.Absolute, 32)); runtimeGrid.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
        _runtimeValue = Label("Native runtime", true); _runtimeUpdateLabel = Label("GitHub Releases", muted: true);
        _checkUpdateButton = CreateNeutralButton("Check", 84); _checkUpdateButton.Dock = DockStyle.Fill; _checkUpdateButton.Click += CheckUpdateClicked;
        _updateButton = CreateNeutralButton("Update", 84); _updateButton.Dock = DockStyle.Fill; _updateButton.Click += UpdateRuntimeClicked;
        runtimeGrid.Controls.Add(_runtimeValue, 0, 0); runtimeGrid.Controls.Add(_checkUpdateButton, 1, 0); runtimeGrid.Controls.Add(_updateButton, 2, 0); runtimeGrid.Controls.Add(_runtimeUpdateLabel, 0, 1); runtimeGrid.SetColumnSpan(_runtimeUpdateLabel, 3);
        runtime.Controls.Add(runtimeGrid); body.Controls.Add(runtime, 0, 7); root.Controls.Add(body, 0, 2);
        var progress = new Panel { Dock = DockStyle.Fill, Padding = new Padding(0, 10, 0, 0) };
        _stepLabel = Label("Ready", muted: true); _stepLabel.Dock = DockStyle.Top; _stepLabel.Height = 25;
        _progressBar = new ThemedProgress { Dock = DockStyle.Bottom, Height = 4 };
        progress.Controls.Add(_stepLabel); progress.Controls.Add(_progressBar); root.Controls.Add(progress, 0, 3);
        var actions = new FlowLayoutPanel { Dock = DockStyle.Fill, FlowDirection = FlowDirection.RightToLeft, WrapContents = false, Padding = new Padding(0, 8, 0, 0) };
        _connectButton = new ThemedButton { Text = "Connect", Primary = true, Size = new Size(110, 36) }; _connectButton.Click += ConnectClicked;
        _disconnectButton = CreateNeutralButton("Disconnect", 110); _disconnectButton.Click += DisconnectClicked;
        var web = CreateNeutralButton("Open Web", 110); web.Click += (_, _) => Process.Start(new ProcessStartInfo { FileName = ProductManifest.RelayPortalUri.AbsoluteUri, UseShellExecute = true });
        var logs = CreateNeutralButton("Logs", 80); logs.Click += (_, _) => OpenLogs();
        actions.Controls.AddRange([_connectButton, _disconnectButton, web, logs]); root.Controls.Add(actions, 0, 4); Controls.Add(root);
        _controller.Changed += ControllerChanged; LoadSettings(); ManagerTheme.Apply(this, ManagerTheme.Get(_themeName)); UpdateState();
        Resize += (_, _) => { if (WindowState == FormWindowState.Minimized) Hide(); };
    }
    private static TableLayoutPanel Grid(int columns, DockStyle dock) => new() { ColumnCount = columns, Dock = dock, Margin = Padding.Empty, Padding = Padding.Empty };
    private static Label Label(string text, bool bold = false, bool muted = false) => new() { Text = text, Dock = DockStyle.Fill, AutoEllipsis = true, TextAlign = ContentAlignment.MiddleLeft, Margin = Padding.Empty, Tag = muted ? "muted" : null, Font = new Font("Segoe UI", 10F, bold ? FontStyle.Bold : FontStyle.Regular) };
    public async Task ApplyTokenAsync(string token)
    {
        _tokenTextBox.Text = token;
        await ConnectCurrentConfigurationAsync();
    }

    public void FocusToken()
    {
        _tokenTextBox.Clear();
        _tokenTextBox.Focus();
    }

    public void AllowCloseAndExit()
    {
        _allowClose = true;
        Close();
    }

    public static void OpenLogs()
    {
        AppPaths.EnsureDirectories();
        var target = File.Exists(AppPaths.RelayLogPath) ? AppPaths.RelayLogPath : AppPaths.LogPath;
        if (!File.Exists(target))
        {
            File.WriteAllText(target, string.Empty);
        }
        Process.Start(new ProcessStartInfo { FileName = target, UseShellExecute = true });
    }

    protected override void OnFormClosing(FormClosingEventArgs eventArgs)
    {
        if (!_allowClose && eventArgs.CloseReason == CloseReason.UserClosing)
        {
            eventArgs.Cancel = true;
            Hide();
            return;
        }
        base.OnFormClosing(eventArgs);
    }

    private void LoadSettings()
    {
        _relayUrlTextBox.Text = _controller.Settings.RelayUrl;
        _workspaceTextBox.Text = _controller.Settings.WorkspaceRoot;
        _portInput.Value = Math.Clamp(_controller.Settings.SupervisorPort, (int)_portInput.Minimum, (int)_portInput.Maximum);
        _startWithWindowsCheckBox.Checked = _controller.Settings.StartWithWindows;
    }

    private async void ConnectClicked(object? sender, EventArgs eventArgs)
    {
        await ConnectCurrentConfigurationAsync();
    }

    private void PasteConfigurationClicked(object? sender, EventArgs eventArgs)
    {
        try
        {
            if (!Clipboard.ContainsText())
            {
                _pasteStateLabel.ForeColor = ErrorColor;
                _pasteStateLabel.Text = "The clipboard does not contain text.";
                return;
            }
            _pasteConfigurationTextBox.Text = Clipboard.GetText(TextDataFormat.Text);
            TryApplyPastedConfiguration(showErrors: true);
        }
        catch (Exception exception) when (exception is ExternalException or ThreadStateException)
        {
            _pasteStateLabel.ForeColor = ErrorColor;
            _pasteStateLabel.Text = "Clipboard access is unavailable. Paste into the box with Ctrl+V.";
        }
    }

    private void TryApplyPastedConfiguration(bool showErrors)
    {
        if (_isApplyingPaste || string.IsNullOrWhiteSpace(_pasteConfigurationTextBox.Text))
        {
            return;
        }
        if (!RelayConfigurationParser.TryParse(
                _pasteConfigurationTextBox.Text,
                out var configuration,
                out var error))
        {
            if (showErrors)
            {
                _pasteStateLabel.ForeColor = ErrorColor;
                _pasteStateLabel.Text = error;
            }
            return;
        }

        _isApplyingPaste = true;
        _parsed = configuration;
        _relayUrlTextBox.Text = configuration!.RelayUrl;
        _tokenTextBox.Text = configuration.DeviceToken;
        if (configuration.SupervisorPort is { } port)
        {
            _portInput.Value = Math.Clamp(port, (int)_portInput.Minimum, (int)_portInput.Maximum);
        }

        _isApplyingPaste = true;
        _pasteConfigurationTextBox.Clear();
        _isApplyingPaste = false;
        _pasteStateLabel.ForeColor = OnlineColor;
        _pasteStateLabel.Text = configuration.SupervisorPort is null
            ? "Configuration parsed: Relay URL and device token filled."
            : "Configuration parsed: Relay URL, device token and local port filled.";
    }

    private async void CheckUpdateClicked(object? sender, EventArgs eventArgs)
    {
        try
        {
            await _controller.CheckRemoteCodexUpdateAsync();
        }
        catch (Exception exception)
        {
            MessageBox.Show(this, AppLogger.Redact(exception.Message), "Update check failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
    }

    private async void UpdateRuntimeClicked(object? sender, EventArgs eventArgs)
    {
        try
        {
            await _controller.UpdateRemoteCodexAsync();
        }
        catch (Exception exception)
        {
            MessageBox.Show(this, AppLogger.Redact(exception.Message), "Update failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
    }

    private async Task ConnectCurrentConfigurationAsync()
    {
        try
        {
            var configuration = new DeviceConfiguration(
                _relayUrlTextBox.Text,
                _tokenTextBox.Text,
                _workspaceTextBox.Text,
                decimal.ToInt32(_portInput.Value),
                _parsed?.SetupCode, _parsed?.AccountKey, _parsed?.EnrollmentSecret, _parsed?.WebOrigin);
            await _controller.ConnectAsync(configuration, _startWithWindowsCheckBox.Checked);
            _tokenTextBox.Clear();
            _parsed = null;
        }
        catch (OperationCanceledException)
        {
            // The UI already reflects the cancelled setup state.
        }
        catch (Exception exception)
        {
            MessageBox.Show(this, AppLogger.Redact(exception.Message), "Connection failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
    }

    private async void DisconnectClicked(object? sender, EventArgs eventArgs)
    {
        try
        {
            await _controller.DisconnectAsync();
        }
        catch (Exception exception)
        {
            MessageBox.Show(this, AppLogger.Redact(exception.Message), "Disconnect failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
    }

    private void BrowseWorkspace(object? sender, EventArgs eventArgs)
    {
        using var dialog = new FolderBrowserDialog
        {
            Description = "Select the root folder Pockymoe can access",
            SelectedPath = Directory.Exists(_workspaceTextBox.Text)
                ? _workspaceTextBox.Text
                : Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
            ShowNewFolderButton = true,
        };
        if (dialog.ShowDialog(this) == DialogResult.OK)
        {
            _workspaceTextBox.Text = dialog.SelectedPath;
        }
    }

    private void ControllerChanged(object? sender, EventArgs eventArgs)
    {
        if (IsDisposed)
        {
            return;
        }
        if (InvokeRequired)
        {
            BeginInvoke(UpdateState);
        }
        else
        {
            UpdateState();
        }
    }

    private void UpdateState()
    {
        var (title, detail, color) = _controller.State switch
        {
            SupervisorState.Running => ("Supervisor running", "This computer is available through the Relay.", OnlineColor),
            SupervisorState.Starting => ("Connecting device", _controller.Progress.Message, ManagerTheme.Get(_themeName).Accent),
            SupervisorState.Stopped => ("Device offline", "Paste a setup command from the Devices page to connect.", Color.FromArgb(100, 116, 139)),
            SupervisorState.Error => ("Connection needs attention", _controller.LastError ?? _controller.Progress.Message, ErrorColor),
            _ => ("Checking status", "Waiting for the local supervisor.", Color.FromArgb(148, 163, 184)),
        };
        _statusTitle.Text = title;
        _statusDetail.Text = detail;
        _statusDot.BackColor = color;
        _stepLabel.Text = $"{_controller.Progress.Step}: {_controller.Progress.Message}";
        _progressBar.Value = Math.Clamp(_controller.Progress.Percent, 0, 100);
        _connectButton.Text = _controller.Runtime?.IsUsable == true ? "Connect" : "Install && connect";
        _connectButton.Enabled = !_controller.IsBusy;
        _disconnectButton.Enabled = !_controller.IsBusy && _controller.Runtime?.IsUsable == true && _controller.State != SupervisorState.Stopped;
        _relayUrlTextBox.Enabled = !_controller.IsBusy;
        _tokenTextBox.Enabled = !_controller.IsBusy;
        _workspaceTextBox.Enabled = !_controller.IsBusy;
        _portInput.Enabled = !_controller.IsBusy;
        _startWithWindowsCheckBox.Enabled = !_controller.IsBusy;
        _pasteConfigurationTextBox.Enabled = !_controller.IsBusy;
        _checkUpdateButton.Enabled = !_controller.IsBusy && _controller.Runtime?.IsNative == true && _controller.State == SupervisorState.Running;
        _updateButton.Enabled = !_controller.IsBusy
            && _controller.Runtime?.IsNative == true
            && _controller.State == SupervisorState.Running
            && !string.IsNullOrWhiteSpace(_controller.AvailableRemoteCodexVersion);
        _tokenStateLabel.Text = _controller.HasSavedToken
            ? "Leave blank to keep the saved device registration."
            : "Use the setup command from the Devices page.";
        _runtimeValue.Text = _controller.Runtime?.IsNative == true
            ? $"Pockymoe {RuntimeState.ResolveNative()?.Version ?? _controller.CurrentRemoteCodexVersion}  ·  Native"
            : _controller.Runtime?.IsUsable == true ? "Legacy npm installation" : "Native runtime  ·  Not installed";
        _runtimeUpdateLabel.Text = _controller.RuntimeUpdateMessage;
    }

    private static TextBox CreateTextBox() => new() { Dock = DockStyle.Fill, BorderStyle = BorderStyle.FixedSingle, Margin = new Padding(2, 4, 8, 4) };
    private static Button CreateNeutralButton(string text, int width) => new ThemedButton { Text = text, Size = new Size(width, 36), Margin = new Padding(4, 0, 0, 0) };
    private static void AddField(TableLayoutPanel grid, string name, Control control, int row, bool spanRemainingColumns = true)
    {
        grid.Controls.Add(Label(name), 0, row); grid.Controls.Add(control, 1, row);
        if (spanRemainingColumns) grid.SetColumnSpan(control, 2);
    }
    protected override void OnShown(EventArgs e)
    {
        base.OnShown(e);
        var available = Screen.FromControl(this).WorkingArea;
        MinimumSize = new Size(Math.Min(MinimumSize.Width, available.Width - 16), Math.Min(MinimumSize.Height, available.Height - 16));
        Size = new Size(Math.Min(Width, available.Width - 16), Math.Min(Height, available.Height - 16));
    }
    protected override void Dispose(bool disposing)
    {
        if (disposing) _controller.Changed -= ControllerChanged;
        base.Dispose(disposing);
    }
}

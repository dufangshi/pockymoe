namespace RemoteCodex.DeviceManager.UI;

internal sealed class TokenDialog : Form
{
    private readonly TextBox _tokenTextBox;

    public TokenDialog(string themeName)
    {
        Text = "Change device token";
        StartPosition = FormStartPosition.CenterParent;
        FormBorderStyle = FormBorderStyle.FixedDialog;
        MaximizeBox = false;
        MinimizeBox = false;
        ShowInTaskbar = false;
        ClientSize = new Size(500, 180);
        Font = new Font("Segoe UI", 10F);

        var title = new Label
        {
            AutoSize = true,
            Font = new Font(Font, FontStyle.Bold),
            Location = new Point(24, 22),
            Text = "Device token",
        };
        _tokenTextBox = new TextBox
        {
            Location = new Point(24, 55),
            Size = new Size(452, 30),
            UseSystemPasswordChar = true,
            AccessibleName = "Relay device token",
        };
        var showToken = new CheckBox
        {
            AutoSize = true,
            Location = new Point(24, 92),
            Text = "Show token",
        };
        showToken.CheckedChanged += (_, _) => _tokenTextBox.UseSystemPasswordChar = !showToken.Checked;

        var cancelButton = new Button
        {
            DialogResult = DialogResult.Cancel,
            Location = new Point(296, 130),
            Size = new Size(86, 32),
            Text = "Cancel",
        };
        var applyButton = new Button
        {
            DialogResult = DialogResult.OK,
            Location = new Point(390, 130),
            Size = new Size(86, 32),
            Text = "Apply",
        };
        applyButton.Click += (_, eventArgs) =>
        {
            if (string.IsNullOrWhiteSpace(DeviceToken))
            {
                MessageBox.Show(this, "Enter a device token.", Text, MessageBoxButtons.OK, MessageBoxIcon.Warning);
                DialogResult = DialogResult.None;
            }
        };

        AcceptButton = applyButton;
        CancelButton = cancelButton;
        Controls.AddRange([title, _tokenTextBox, showToken, cancelButton, applyButton]);
        ManagerTheme.Apply(this, ManagerTheme.Get(themeName));
    }

    public string DeviceToken => _tokenTextBox.Text.Trim();

    protected override void OnShown(EventArgs eventArgs)
    {
        base.OnShown(eventArgs);
        _tokenTextBox.Clear();
        _tokenTextBox.Focus();
    }
}

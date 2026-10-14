using System.Drawing.Drawing2D;

namespace RemoteCodex.DeviceManager.UI;

internal sealed record ManagerTheme(Color Background, Color Surface, Color Input, Color Text, Color Muted, Color Border, Color Accent, Color AccentText, bool Plum)
{
    private static Color Hex(string hex) => ColorTranslator.FromHtml(hex);
    public static ManagerTheme Get(string name) => name switch
    {
        "plum-light" => new(Hex("#fbf5e9"), Hex("#fffdf7"), Hex("#f4e9d1"), Hex("#402449"), Hex("#817087"), Hex("#ddbe7d"), Hex("#efb445"), Hex("#352038"), true),
        "classic-light" => new(Hex("#f7f9f8"), Color.White, Hex("#eef2f0"), Hex("#1e2924"), Hex("#67766e"), Hex("#cbd5cf"), Hex("#008b5c"), Color.White, false),
        "classic-dark" => new(Hex("#0c1012"), Hex("#151b20"), Hex("#202830"), Hex("#e8eeeb"), Hex("#a1ada8"), Hex("#33413c"), Hex("#00c985"), Hex("#08291d"), false),
        _ => new(Hex("#1a1022"), Hex("#291b33"), Hex("#362441"), Hex("#f7ebd5"), Hex("#bba7c5"), Hex("#604171"), Hex("#f0b54a"), Hex("#2a1530"), true),
    };
    public static void Apply(Control control, ManagerTheme theme)
    {
        control.ForeColor = theme.Text;
        control.BackColor = control.Parent?.BackColor ?? theme.Background;
        if (control is ThemedPanel panel) { panel.Theme = theme; panel.BackColor = theme.Surface; }
        if (control is ThemedButton button) { button.Theme = theme; button.BackColor = theme.Surface; }
        if (control is ThemedProgress progress) progress.Theme = theme;
        if (control is TextBox or NumericUpDown or ComboBox) control.BackColor = theme.Input;
        if (control is Label label && label.Tag as string == "muted") label.ForeColor = theme.Muted;
        foreach (Control child in control.Controls) Apply(child, theme);
        control.Invalidate();
    }
    internal static GraphicsPath Round(Rectangle bounds, int radius)
    {
        var path = new GraphicsPath(); var d = Math.Min(radius * 2, Math.Min(bounds.Width, bounds.Height));
        if (d <= 0) { path.AddRectangle(bounds); return path; }
        path.AddArc(bounds.X, bounds.Y, d, d, 180, 90);
        path.AddArc(bounds.Right - d, bounds.Y, d, d, 270, 90);
        path.AddArc(bounds.Right - d, bounds.Bottom - d, d, d, 0, 90);
        path.AddArc(bounds.X, bounds.Bottom - d, d, d, 90, 90); path.CloseFigure(); return path;
    }
}
internal sealed class ThemedPanel : Panel
{
    public ManagerTheme Theme { get; set; } = ManagerTheme.Get("plum-dark");
    public ThemedPanel() { DoubleBuffered = true; Padding = new Padding(16, 12, 16, 12); }
    protected override void OnPaint(PaintEventArgs e)
    {
        base.OnPaint(e); e.Graphics.SmoothingMode = SmoothingMode.AntiAlias;
        using var path = ManagerTheme.Round(new Rectangle(1, 1, Width - 3, Height - 3), Theme.Plum ? 16 : 10);
        using var pen = new Pen(Theme.Border, 1) { DashStyle = Theme.Plum ? DashStyle.Dash : DashStyle.Solid };
        e.Graphics.DrawPath(pen, path);
    }
}
internal sealed class ThemedButton : Button
{
    public ManagerTheme Theme { get; set; } = ManagerTheme.Get("plum-dark");
    public bool Primary { get; set; }
    public ThemedButton() { FlatStyle = FlatStyle.Flat; FlatAppearance.BorderSize = 0; DoubleBuffered = true; Cursor = Cursors.Hand; }
    protected override void OnPaint(PaintEventArgs e)
    {
        e.Graphics.Clear(Parent?.BackColor ?? Theme.Background);
        e.Graphics.SmoothingMode = SmoothingMode.AntiAlias;
        using var path = ManagerTheme.Round(new Rectangle(1, 1, Width - 3, Height - 3), Theme.Plum ? 12 : 8);
        using var fill = new SolidBrush(Primary && Enabled ? Theme.Accent : Theme.Input);
        e.Graphics.FillPath(fill, path);
        using var border = new Pen(Focused ? Theme.Accent : Theme.Border, Focused ? 2 : 1);
        e.Graphics.DrawPath(border, path);
        TextRenderer.DrawText(e.Graphics, Text, Font, ClientRectangle, !Enabled ? Theme.Muted : Primary ? Theme.AccentText : Theme.Text,
            TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter | TextFormatFlags.EndEllipsis);
    }
}

internal sealed class ThemedProgress : Control
{
    public ManagerTheme Theme { get; set; } = ManagerTheme.Get("plum-dark");
    private int _value;
    public int Value { get => _value; set { _value = Math.Clamp(value, 0, 100); Invalidate(); } }
    public ThemedProgress() { DoubleBuffered = true; }
    protected override void OnPaint(PaintEventArgs e)
    {
        e.Graphics.Clear(Theme.Input);
        using var fill = new SolidBrush(Theme.Accent);
        e.Graphics.FillRectangle(fill, 0, 0, Width * Value / 100, Height);
    }
}

internal sealed class ManagerMenuRenderer(ManagerTheme theme) : ToolStripRenderer
{
    protected override void OnRenderToolStripBackground(ToolStripRenderEventArgs e)
    {
        using var brush = new SolidBrush(theme.Surface);
        e.Graphics.FillRectangle(brush, e.AffectedBounds);
    }
    protected override void OnRenderMenuItemBackground(ToolStripItemRenderEventArgs e)
    {
        using var brush = new SolidBrush(e.Item.Selected && e.Item.Enabled ? theme.Input : theme.Surface);
        e.Graphics.FillRectangle(brush, new Rectangle(Point.Empty, e.Item.Size));
    }
    protected override void OnRenderImageMargin(ToolStripRenderEventArgs e)
    {
        using var brush = new SolidBrush(theme.Surface);
        e.Graphics.FillRectangle(brush, e.AffectedBounds);
    }
    protected override void OnRenderItemCheck(ToolStripItemImageRenderEventArgs e)
    {
        using var pen = new Pen(theme.Accent, 2);
        var r = e.ImageRectangle;
        e.Graphics.DrawLines(pen, new Point[] { new Point(r.Left + 2, r.Top + r.Height / 2), new Point(r.Left + r.Width / 3, r.Bottom - 3), new Point(r.Right - 2, r.Top + 3) });
    }
    protected override void OnRenderToolStripBorder(ToolStripRenderEventArgs e)
    {
        using var pen = new Pen(theme.Border);
        e.Graphics.DrawRectangle(pen, 0, 0, e.ToolStrip.Width - 1, e.ToolStrip.Height - 1);
    }
    protected override void OnRenderSeparator(ToolStripSeparatorRenderEventArgs e)
    {
        using var pen = new Pen(theme.Border);
        e.Graphics.DrawLine(pen, 8, e.Item.Height / 2, e.Item.Width - 8, e.Item.Height / 2);
    }
    protected override void OnRenderItemText(ToolStripItemTextRenderEventArgs e)
    {
        e.TextColor = e.Item.Enabled ? theme.Text : theme.Muted;
        base.OnRenderItemText(e);
    }
}

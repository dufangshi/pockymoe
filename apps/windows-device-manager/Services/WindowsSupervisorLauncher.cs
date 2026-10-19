using System.Diagnostics;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text.Json;
using RemoteCodex.DeviceManager.Infrastructure;
using RemoteCodex.DeviceManager.Models;

namespace RemoteCodex.DeviceManager.Services;

internal static class WindowsSupervisorLauncher
{
    // A normal Windows child outlives its parent. WMI is unnecessary for the
    // desktop Manager and can acknowledge Create without running the launcher.
    internal static void StartScript(string script)
    {
        var start = new ProcessStartInfo(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System),
            "WindowsPowerShell", "v1.0", "powershell.exe"))
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            WorkingDirectory = AppPaths.UserHome,
        };
        foreach (var arg in new[] { "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script })
            start.ArgumentList.Add(arg);
        start.Environment.Remove("POCKYMOE_ENROLL_SECRET");
        start.Environment.Remove("POCKYMOE_ENROLL");
        start.Environment.Remove("REMOTE_CODEX_ENROLL_SECRET");
        var command = new System.Text.StringBuilder($"\"{start.FileName}\" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{script}\"");
        var environment = string.Join('\0', start.Environment.OrderBy(e => e.Key, StringComparer.OrdinalIgnoreCase)
            .Select(e => $"{e.Key}={e.Value}")) + "\0\0";
        var block = Marshal.StringToHGlobalUni(environment);
        try
        {
            var info = new StartupInfo { Size = Marshal.SizeOf<StartupInfo>() };
            const uint flags = 0x08000000 | 0x01000000 | 0x00000400; // no console, break away from caller job, Unicode environment
            if (!CreateProcess(start.FileName, command, IntPtr.Zero, IntPtr.Zero, false, flags, block, start.WorkingDirectory, ref info, out var process))
            {
                var error = Marshal.GetLastWin32Error();
                // A restrictive parent job can prohibit breakaway. A desktop
                // Manager normally has no job; preserve launch on managed hosts.
                if (error != 5 || !CreateProcess(start.FileName, command, IntPtr.Zero, IntPtr.Zero, false,
                    flags & ~0x01000000u, block, start.WorkingDirectory, ref info, out process))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Unable to start the Windows Supervisor launcher.");
            }
            CloseHandle(process.Thread);
            CloseHandle(process.Process);
        }
        finally { Marshal.FreeHGlobal(block); }
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct StartupInfo
    {
        public int Size;
        public string? Reserved, Desktop, Title;
        public uint X, Y, Width, Height, XCount, YCount, Fill, Flags;
        public ushort Show, ReservedSize;
        public IntPtr ReservedBytes, Input, Output, Error;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessInfo { public IntPtr Process, Thread; public uint ProcessId, ThreadId; }
    [DllImport("kernel32.dll", EntryPoint = "CreateProcessW", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CreateProcess(string application, System.Text.StringBuilder command, IntPtr processAttributes,
        IntPtr threadAttributes, bool inheritHandles, uint flags, IntPtr environment, string directory, ref StartupInfo startup, out ProcessInfo process);
    [DllImport("kernel32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CloseHandle(IntPtr handle);

    internal static HashSet<string> PendingLaunchers() => Directory.Exists(Path.GetDirectoryName(AppPaths.RelayLogPath)!)
        ? Directory.GetFiles(Path.GetDirectoryName(AppPaths.RelayLogPath)!, "relay-supervisor.*.launch.json").ToHashSet(StringComparer.OrdinalIgnoreCase)
        : new(StringComparer.OrdinalIgnoreCase);

    internal static HashSet<string> PendingMaintenanceLaunchers()
    {
        var updates = Path.Combine(AppPaths.RelayDataRoot, "updates");
        return Directory.Exists(updates) ? Directory.GetFiles(updates, "update.*.launch.json", SearchOption.AllDirectories)
            .ToHashSet(StringComparer.OrdinalIgnoreCase) : new(StringComparer.OrdinalIgnoreCase);
    }

    private static void StartPayload(string payload, JsonElement root)
    {
        // Parse with System.Text.Json, rather than PowerShell 5.1's legacy JSON
        // parser. Environment values carried through a native restart can be
        // valid JSON that the legacy parser rejects before consuming the task.
        static string Quote(string value) => "'" + value.Replace("'", "''") + "'";
        var script = Path.ChangeExtension(payload, ".ps1");
        var source = new System.Text.StringBuilder("$ErrorActionPreference='Stop'\n");
        source.AppendLine($"Remove-Item -LiteralPath {Quote(payload)}, $PSCommandPath -Force");
        if (root.TryGetProperty("environment", out var environment))
            foreach (var entry in environment.EnumerateObject())
            {
                if (entry.Name is "POCKYMOE_ENROLL_SECRET" or "POCKYMOE_ENROLL" or "REMOTE_CODEX_ENROLL_SECRET") continue;
                source.AppendLine($"[Environment]::SetEnvironmentVariable({Quote(entry.Name)},{Quote(entry.Value.GetString()!)},'Process')");
            }
        source.AppendLine("$ErrorActionPreference='Continue'");
        source.AppendLine($"& {Quote(root.GetProperty("binary").GetString()!)} {string.Join(' ', root.GetProperty("args").EnumerateArray().Select(v => Quote(v.GetString()!)))} *> {Quote(root.GetProperty("log").GetString()!)}");
        source.AppendLine("exit $LASTEXITCODE");
        var temporary = script + ".manager.tmp";
        File.WriteAllText(temporary, source.ToString(), new System.Text.UTF8Encoding(true));
        File.Move(temporary, script, true);
        StartScript(script);
    }

    internal static void RecoverMaintenance(HashSet<string> previous, AppLogger logger)
    {
        var installed = RuntimeState.ResolveNative();
        if (installed is null) return;
        foreach (var payload in PendingMaintenanceLaunchers().Except(previous, StringComparer.OrdinalIgnoreCase))
        {
            if (DateTime.UtcNow - File.GetLastWriteTimeUtc(payload) < TimeSpan.FromSeconds(5)) continue;
            try
            {
                using var json = JsonDocument.Parse(File.ReadAllText(payload));
                var root = json.RootElement;
                var args = root.GetProperty("args").EnumerateArray().Select(v => v.GetString()).ToArray();
                var directory = Path.GetDirectoryName(payload)!;
                var planPath = Path.Combine(directory, "plan.json");
                var binary = Path.Combine(directory, "remote-codex.exe");
                static bool Same(string? a, string b) => a is not null && string.Equals(Path.GetFullPath(a), Path.GetFullPath(b), StringComparison.OrdinalIgnoreCase);
                if (args.Length != 3 || args[0] != "internal-update" || args[1] != "--plan" || !Same(args[2], planPath)
                    || !Same(root.GetProperty("binary").GetString(), binary)
                    || !Same(root.GetProperty("log").GetString(), Path.Combine(directory, "update.log"))) continue;
                using var plan = JsonDocument.Parse(File.ReadAllText(planPath));
                if (!Same(plan.RootElement.GetProperty("context").GetProperty("config").GetString(), AppPaths.RelayConfigPath)
                    || !Same(plan.RootElement.GetProperty("statusFile").GetString(), Path.Combine(AppPaths.RelayDataRoot, "updates", "supervisor-update.json"))) continue;
                using var expected = File.OpenRead(installed.Executable);
                using var actual = File.OpenRead(binary);
                if (!SHA256.HashData(expected).SequenceEqual(SHA256.HashData(actual))) continue;
                var script = Path.ChangeExtension(payload, ".ps1");
                if (!File.Exists(script)) continue;
                previous.Add(payload);
                logger.Warning("Native maintenance launcher was not consumed by WMI; starting the validated worker directly.");
                StartPayload(payload, root);
            }
            catch (Exception e) when (e is IOException or JsonException) { }
        }
    }

    // Released runtimes use WMI during setup. Recover only the one-use launcher
    // emitted by this setup, after install/activation and account binding. Never
    // rerun setup or redeem its registration code a second time.
    internal static bool TryRecoverSetup(string version, HashSet<string> previous, AppLogger logger)
    {
        var installed = RuntimeState.ResolveNative();
        if (installed?.Version != version) return false;
        foreach (var payload in PendingLaunchers().Except(previous, StringComparer.OrdinalIgnoreCase))
        {
            if (DateTime.UtcNow - File.GetLastWriteTimeUtc(payload) < TimeSpan.FromSeconds(5)) continue;
            try
            {
                using var json = JsonDocument.Parse(File.ReadAllText(payload));
                var root = json.RootElement;
                var args = root.GetProperty("args").EnumerateArray().Select(v => v.GetString()).ToArray();
                if (!string.Equals(Path.GetFullPath(root.GetProperty("binary").GetString()!), Path.GetFullPath(installed.Executable), StringComparison.OrdinalIgnoreCase)
                    || args.Length != 3 || args[0] != "device-run" || args[1] != "--config"
                    || !string.Equals(Path.GetFullPath(args[2]!), Path.GetFullPath(AppPaths.RelayConfigPath), StringComparison.OrdinalIgnoreCase)
                    || !string.Equals(Path.GetFullPath(root.GetProperty("log").GetString()!), Path.GetFullPath(AppPaths.RelayLogPath), StringComparison.OrdinalIgnoreCase)) continue;
                var script = Path.ChangeExtension(payload, ".ps1");
                if (!File.Exists(script)) continue;
                previous.Add(payload);
                logger.Warning("Native setup launcher was not consumed by WMI; starting the verified installation directly.");
                StartPayload(payload, root);
                return true;
            }
            catch (Exception e) when (e is IOException or JsonException)
            {
                // The native launcher may consume its payload between enumeration and reading.
            }
        }
        return false;
    }
}

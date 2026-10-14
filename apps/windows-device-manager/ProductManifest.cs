namespace RemoteCodex.DeviceManager;

internal static class ProductManifest
{
    public const string ProductName = "Pockymoe Device Manager";
    public const string DefaultRelayUrl = "https://remote.lnz-study.com";
    public const int DefaultSupervisorPort = 45680;
    public const string RemoteCodexVersion = "0.13.3";
    public const string MinimumNativeRuntimeVersion = "0.13.3";
    public const string ReleaseRepository = "https://github.com/dufangshi/pockymoe";
    public static readonly Uri RelayPortalUri = new("https://app.lnz-study.com/devices");
}

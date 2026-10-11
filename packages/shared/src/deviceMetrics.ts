export interface PowerReadingDto {
  watts: number | null;
  source: string | null;
  reason: string | null;
}
export interface TemperatureReadingDto {
  celsius: number | null;
  source: string | null;
  reason: string | null;
  sensors: Array<{ label: string; celsius: number }>;
}
export interface MemoryMetricsDto {
  totalBytes: number;
  usedBytes: number;
  availableBytes: number;
  usagePercent: number | null;
}
export interface GpuMetricsDto {
  id: string;
  name: string;
  usagePercent: number | null;
  usedMemoryBytes: number | null;
  totalMemoryBytes: number | null;
  power: PowerReadingDto;
  source: string;
}
export interface DeviceMetricsDto {
  sampledAt: string;
  sampleWindowMs: number;
  platform: string;
  environment: string;
  cpu: {
    model: string;
    usagePercent: number | null;
    logicalCoreCount: number;
    /** `celsius` is the physical core's temperature, when the platform reports one. */
    cores: Array<{ index: number; usagePercent: number | null; celsius?: number | null }>;
  };
  memory: MemoryMetricsDto;
  swap: MemoryMetricsDto;
  cpuPower: PowerReadingDto;
  cpuTemperature?: TemperatureReadingDto;
  gpus: GpuMetricsDto[];
  hardwareSampledAt: string;
  hardwareNotes: string[];
  limits: {
    cpuCores: number | null;
    memoryTotalBytes: number | null;
    memoryUsedBytes: number | null;
  } | null;
}

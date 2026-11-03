import { createContext, useContext, type ReactNode } from 'react';
import { categoryColor, type DeviceMetrics } from './model';
import { normalizeModelName } from './usage';

const Colors = createContext<{ devices: string[]; models: string[] }>({ devices: [], models: [] });
export function UsageColors({ deviceIds, metrics, children }: { deviceIds: string[]; metrics: Record<string, DeviceMetrics>; children: ReactNode }) {
  // All-time, unfiltered summaries are the model inventory. Include cached
  // snapshots while older/offline devices cannot supply an all-time response.
  const models = deviceIds.flatMap(id => Object.values(metrics[id]?.summaries ?? {}).flatMap(summary => summary?.byModel.map(row => normalizeModelName(row.model)) ?? []));
  return <Colors.Provider value={{ devices: deviceIds, models: [...models, 'unknown'] }}>{children}</Colors.Provider>;
}
export function useUsageColors(fallbackDevices: string[] = [], fallbackModels: string[] = []) {
  const inventory = useContext(Colors);
  return {
    deviceColor: (id: string) => categoryColor(id, inventory.devices.length ? inventory.devices : fallbackDevices),
    usageColor: (id: string) => categoryColor(id, inventory.models.length ? inventory.models : fallbackModels),
  };
}

import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { type ThemePreset, useThemePreset } from '../lib/themePreset';

const presets: { id: ThemePreset; label: 'files.themePresetClassic' | 'files.themePresetPlumPocket'; description: 'files.themePresetClassicDescription' | 'files.themePresetPlumPocketDescription'; swatches: string[] }[] = [
  { id: 'classic', label: 'files.themePresetClassic', description: 'files.themePresetClassicDescription', swatches: ['#e2e7e2', '#0f1317', '#008b53', '#a2afb9'] },
  { id: 'plum-pocket', label: 'files.themePresetPlumPocket', description: 'files.themePresetPlumPocketDescription', swatches: ['#2f1a3b', '#fbf4e8', '#e8a93a', '#f0e0f4'] },
];

/** Theme preset choice for this browser; the color mode applies to each preset. */
export function ThemePresetSettings() {
  useI18n();
  const [selected, select] = useThemePreset();
  return (
    <div className="mt-4">
      <p className="text-xs font-semibold text-[var(--theme-fg)]">{translate('files.themePreset')}</p>
      <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">{translate('files.themePresetDescription')}</p>
      <div role="radiogroup" aria-label={translate('files.themePreset')} className="mt-3 grid grid-cols-2 gap-2 sm:max-w-lg">
        {presets.map((preset) => (
          <label key={preset.id} className="theme-preset-option" data-preset={preset.id}>
            <input
              type="radio"
              name="settings-theme-preset"
              value={preset.id}
              checked={selected === preset.id}
              onChange={() => select(preset.id)}
              className="sr-only"
            />
            <span className="theme-preset-swatches" aria-hidden="true">
              {preset.swatches.map((color) => <i key={color} style={{ background: color }} />)}
            </span>
            <span className="theme-preset-name">{translate(preset.label)}</span>
            <span className="theme-preset-description">{translate(preset.description)}</span>
          </label>
        ))}
      </div>
    </div>
  );
}

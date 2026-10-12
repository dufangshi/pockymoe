import { translate, useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import { Download, Trash2, Upload } from 'lucide-react';
import { useEffect, useRef, useState, type ChangeEvent } from 'react';

import {
  CLASSIC_THEME_ID,
  ThemePackError,
  exportThemePack,
  importThemePack,
  localizedText,
  removeThemePack,
  selectTheme,
  themePreviewUrl,
  useThemeRegistry,
  type ThemeEntry,
  type ThemePackErrorCode,
} from '../lib/themePacks';

const ERROR_KEYS: Record<ThemePackErrorCode, TranslationKey> = {
  archive: 'files.themeErrorArchive',
  limits: 'files.themeErrorLimits',
  path: 'files.themeErrorPath',
  manifest: 'files.themeErrorManifest',
  css: 'files.themeErrorCss',
  asset: 'files.themeErrorAsset',
  reserved: 'files.themeErrorReserved',
  storage: 'files.themeErrorStorage',
};

function errorText(error: unknown) {
  if (error instanceof ThemePackError) return `${translate(ERROR_KEYS[error.code])} (${error.detail})`;
  return error instanceof Error ? error.message : String(error);
}

function themeName(entry: ThemeEntry, locale: string) {
  return entry.source === 'classic' ? translate('files.themePresetClassic') : localizedText(entry.manifest.name, locale);
}

function ThemeSwatches({ entry }: { entry: ThemeEntry }) {
  const [preview, setPreview] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    if (entry.manifest.preview) void themePreviewUrl(entry.id).then((url) => live && setPreview(url)).catch(() => undefined);
    return () => { live = false; };
  }, [entry]);
  if (preview) return <img className="theme-preset-preview" src={preview} alt="" aria-hidden="true" />;
  return (
    <span className="theme-preset-swatches" aria-hidden="true">
      {entry.manifest.swatches.map((color, index) => <i key={`${color}-${index}`} style={{ background: color }} />)}
    </span>
  );
}

/** Theme packs for this browser: Classic, the built-in packs and imported ones. The
 *  color mode applies to each. */
export function ThemePresetSettings() {
  const { locale } = useI18n();
  const { entries, selected } = useThemeRegistry();
  const [status, setStatus] = useState<{ error: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const checked = entries.some((entry) => entry.id === selected) ? selected : CLASSIC_THEME_ID;

  const onImport = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file) return;
    setBusy(true);
    try {
      const manifest = await importThemePack(file);
      setStatus({ error: false, text: translate('files.themeImported', { value1: localizedText(manifest.name, locale) }) });
    } catch (error) {
      setStatus({ error: true, text: translate('files.themeImportFailed', { value1: errorText(error) }) });
    } finally {
      setBusy(false);
    }
  };
  const onExport = async (entry: ThemeEntry) => {
    try {
      const { fileName, blob } = await exportThemePack(entry.id);
      const url = URL.createObjectURL(blob);
      const link = Object.assign(document.createElement('a'), { href: url, download: fileName });
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 30_000);
    } catch (error) {
      setStatus({ error: true, text: translate('files.themeExportFailed', { value1: errorText(error) }) });
    }
  };
  const onRemove = async (entry: ThemeEntry) => {
    setStatus(null);
    await removeThemePack(entry.id).catch((error: unknown) => setStatus({ error: true, text: errorText(error) }));
  };

  return (
    <div className="mt-4">
      <div className="theme-preset-heading">
        <p className="text-xs font-semibold text-[var(--theme-fg)]">{translate('files.themePreset')}</p>
        <button type="button" className="theme-preset-import" disabled={busy} onClick={() => input.current?.click()}>
          <Upload aria-hidden="true" />
          <span>{translate('files.themeImport')}</span>
        </button>
        <input ref={input} type="file" accept=".pockytheme,.zip,application/zip" hidden data-testid="theme-pack-file" onChange={(event) => void onImport(event)} />
      </div>
      <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">
        {translate('files.themePresetDescription')} {translate('files.themeImportHint')}
      </p>
      {status ? (
        <p role={status.error ? 'alert' : 'status'} className={`theme-preset-status ${status.error ? 'is-error' : ''}`} title={status.text}>
          {status.text}
        </p>
      ) : null}
      <div role="radiogroup" aria-label={translate('files.themePreset')} className="mt-3 grid grid-cols-2 gap-2 sm:max-w-lg">
        {entries.map((entry) => {
          const name = themeName(entry, locale);
          const description = entry.source === 'classic' ? translate('files.themePresetClassicDescription') : localizedText(entry.manifest.description, locale);
          const meta = entry.source === 'classic' ? translate('files.themeBuiltIn') : [entry.manifest.author, entry.manifest.version].filter(Boolean).join(' · ');
          const title = [description, entry.manifest.engine ? `Pockymoe ${entry.manifest.engine}` : ''].filter(Boolean).join('\n');
          return (
            <div key={entry.id} className="theme-preset-card" data-source={entry.source}>
              <label className="theme-preset-option" data-preset={entry.id} title={title || undefined}>
                <input
                  type="radio"
                  name="settings-theme-preset"
                  value={entry.id}
                  checked={checked === entry.id}
                  onChange={() => { setStatus(null); void selectTheme(entry.id); }}
                  className="sr-only"
                />
                <ThemeSwatches entry={entry} />
                <span className="theme-preset-name">{name}</span>
                <span className="theme-preset-meta">{meta}</span>
              </label>
              {entry.source !== 'classic' ? (
                <div className="theme-preset-actions">
                  <button type="button" aria-label={translate('files.themeExport', { value1: name })} title={translate('files.themeExport', { value1: name })} onClick={() => void onExport(entry)}>
                    <Download aria-hidden="true" />
                  </button>
                  {entry.source === 'imported' ? (
                    <button type="button" aria-label={translate('files.themeRemove', { value1: name })} title={translate('files.themeRemove', { value1: name })} onClick={() => void onRemove(entry)}>
                      <Trash2 aria-hidden="true" />
                    </button>
                  ) : null}
                </div>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}

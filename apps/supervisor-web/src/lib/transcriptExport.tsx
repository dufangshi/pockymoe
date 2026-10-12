import { relayUrl, relayFetchInit } from './relayOrigin';
import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import {
  PublicTranscript,
  transcriptSnapshot,
  type PublicTranscriptSnapshot,
} from '@pockymoe/thread-ui';
import type { ExportThreadTranscriptInput, ThreadTurnDto } from '@pockymoe/shared';
import { downloadThreadImage, fetchThreadDetail, fetchThreadTurnDetail } from './api';
import { standaloneImageViewer } from './standaloneImageViewer';

const nextFrame = () =>
  new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const escape = (text: string) =>
  text
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
const dataUrl = (blob: Blob) =>
  new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });

export async function loadExportSnapshot(
  id: string,
  input: ExportThreadTranscriptInput,
  all = false,
): Promise<PublicTranscriptSnapshot> {
  const selected = input.mode === 'selected' ? new Set(input.turnIds) : null;
  const limit = Math.min(100, Math.max(1, input.limit ?? 10));
  if (selected && (!selected.size || selected.size > 10_000))
    throw new Error(translate("sharing.selectBetween1And10000Turns"));
  let turns: ThreadTurnDto[] = [];
  let cursor: string | undefined;
  let title = translate("sharing.thread");
  const seen = new Set<string>();
  do {
    const page = await fetchThreadDetail(id, {
      limit: selected || all ? 100 : limit,
      ...(cursor ? { beforeTurnId: cursor } : {}),
    });
    title = page.thread.title;
    const fresh = page.turns.filter((turn) => !seen.has(turn.id));
    if (!fresh.length) break;
    fresh.forEach((turn) => seen.add(turn.id));
    turns = [
      ...fresh.filter((turn) => !selected || selected.has(turn.id)),
      ...turns,
    ];
    cursor = fresh[0]?.id;
    if (turns.length > 10000) throw new Error(translate("sharing.theTranscriptIsTooLargeToShare"));
    if (
      (!selected && !all) ||
      (selected && turns.length === selected.size) ||
      seen.size >= (page.totalTurnCount ?? seen.size)
    )
      break;
  } while (cursor);
  if (selected && turns.length !== selected.size)
    throw new Error(translate("sharing.oneOrMoreSelectedTurnsAreNo"));
  const theme =
    document.documentElement.getAttribute('data-theme-effective') === 'dark'
      ? 'dark'
      : 'light';
  turns = await Promise.all(turns.map(turn => turn.hasDeferredItems ? fetchThreadTurnDetail(id, turn.id) : turn));
  const snapshot = transcriptSnapshot(title, turns, theme);
  if (input.options?.includeTokenAndPrice === false)
    snapshot.turns.forEach((turn) => {
      delete turn.tokenUsage;
      delete turn.priceEstimate;
    });
  const paths = new Set(
    snapshot.turns.flatMap((turn) =>
      // Only user messages render PHOTO markers as attachments. Assistant
      // replies can discuss the syntax (e.g. [PHOTO …]); those remain text.
      turn.messages.filter(message => message.role === 'user').flatMap((message) =>
        Array.from(
          message.text.matchAll(/\[PHOTO\s+([^\]]+)\]/g),
          (match) => match[1]!.trim(),
        ).filter(Boolean),
      ),
    ),
  );
  snapshot.images = {};
  let imageBytes = 0;
  for (const path of paths) {
    const blob = await downloadThreadImage(id, path);
    imageBytes += blob.size;
    if (imageBytes > 10 * 1024 * 1024) throw new Error(translate("sharing.attachmentsExceedThe10MBPublicSnapshot"));
    snapshot.images[path] = await dataUrl(blob);
  }
  return snapshot;
}

async function standaloneCss() {
  const resources = new Map<string, Promise<string>>();
  const embed = (url: string) => {
    if (!resources.has(url))
      resources.set(
        url,
        fetch(relayUrl(url), relayFetchInit(url)).then(async (response) => {
          if (!response.ok)
            throw new Error(
              translate("sharing.anExportFontOrStyleResourceCould"),
            );
          return dataUrl(await response.blob());
        }),
      );
    return resources.get(url)!;
  };
  async function rules(sheet: CSSStyleSheet): Promise<string> {
    const result: string[] = [];
    for (const rule of Array.from(sheet.cssRules)) {
      if (rule instanceof CSSImportRule && rule.styleSheet) {
        result.push(await rules(rule.styleSheet));
        continue;
      }
      let css = rule.cssText;
      const urls = Array.from(
        css.matchAll(/url\(\s*['"]?([^'"\)]+)['"]?\s*\)/g),
      );
      for (const match of urls) {
        const path = match[1]!.trim();
        if (path.startsWith('data:') || path.startsWith('#')) continue;
        const url = new URL(path, sheet.href ?? document.baseURI).href;
        css = css.replace(match[0], `url("${await embed(url)}")`);
      }
      result.push(css);
    }
    return result.join('\n');
  }
  return (await Promise.all(Array.from(document.styleSheets).map(rules))).join(
    '\n',
  );
}

export async function renderStandaloneTranscript(
  snapshot: PublicTranscriptSnapshot,
) {
  const host = document.createElement('div');
  host.style.cssText =
    'position:fixed;left:-100000px;top:0;width:896px;pointer-events:none';
  document.body.append(host);
  const root = createRoot(host);
  try {
    flushSync(() => root.render(<PublicTranscript snapshot={snapshot} />));
    await document.fonts.ready;
    // The same asynchronous syntax highlighter used by the live thread.
    const deadline = performance.now() + 10_000;
    while (
      host.querySelector('[data-markdown-ready="false"]') &&
      performance.now() < deadline
    )
      await new Promise((resolve) => setTimeout(resolve, 30));
    await nextFrame();
    await nextFrame();
    const css = await standaloneCss();
    const clone = host.firstElementChild!.cloneNode(true) as HTMLElement;
    clone.querySelectorAll('button').forEach((button) => {
      const span = document.createElement('span');
      span.className = button.className;
      span.append(...Array.from(button.childNodes));
      button.replaceWith(span);
    });
    await Promise.all(Array.from(clone.querySelectorAll('img')).map(async (img) => {
      img.setAttribute('loading', 'eager');
      if (img.src.startsWith('data:')) return;
      // Embed Markdown images when their host allows browser access. Otherwise
      // preserve the original external image, as the live thread does.
      try {
        const response = await fetch(relayUrl(img.src), relayFetchInit(img.src));
        if (response.ok) img.src = await dataUrl(await response.blob());
      } catch { /* External images can still load when viewing the HTML online. */ }
    }));
    clone.querySelectorAll('img').forEach(img => {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'transcript-image-button';
      button.setAttribute('aria-label', `Enlarge image${img.alt ? `: ${img.alt}` : ''}`);
      const anchor = img.closest('a');
      (anchor ?? img).replaceWith(button);
      button.append(img);
    });
    const theme = snapshot.theme ?? 'dark';
    // The applied theme pack is the only one in the document's stylesheets (its
    // assets are inlined above); the export keeps its look and flags.
    const { themePreset, themeFeatures } = document.documentElement.dataset;
    const pack = themePreset ? ` data-theme-preset="${escape(themePreset)}" data-theme-features="${escape(themeFeatures ?? '')}"` : '';
    return `<!doctype html><html lang="${getLocale()}" class="${theme}" data-theme-effective="${theme}"${pack}><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="${theme}"><title>${escape(snapshot.title)}</title><style>${css.replaceAll('</style', '<\\/style')}</style><style>html,body{margin:0;background:${theme === 'dark' ? '#11110d' : '#f5f6f5'}}a{overflow-wrap:anywhere}</style></head><body>${clone.outerHTML}${standaloneImageViewer()}</body></html>`;
  } finally {
    root.unmount();
    host.remove();
  }
}

// The desktop client takes one file through its save dialog in a single
// message of at most 64 MB. A larger one goes through its download manager,
// which streams to disk and asks where to save it as well.
export const NATIVE_SAVE_LIMIT = 64 * 1024 * 1024;

export async function nativeSave(blob: Blob, name: string): Promise<boolean> {
  const host = window.pockymoeHost;
  if (!host || typeof host.files?.saveAs !== 'function' || blob.size > NATIVE_SAVE_LIMIT) return false;
  const capabilities = await host.capabilities();
  if (capabilities.surface === 'desktop' && !capabilities.features.nativeSave) return false;
  await host.files.saveAs(name, await blob.arrayBuffer());
  return true; // A cancelled native dialog must not start a second, browser download.
}

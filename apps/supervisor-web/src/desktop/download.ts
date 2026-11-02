export async function nativeSave(blob: Blob, name: string): Promise<boolean> {
  const host = window.pockymoeHost;
  if (!host || typeof host.files?.saveAs !== 'function') return false;
  const capabilities = await host.capabilities();
  if (capabilities.surface === 'desktop' && !capabilities.features.nativeSave) return false;
  await host.files.saveAs(name, await blob.arrayBuffer());
  return true; // A cancelled native dialog must not start a second, browser download.
}

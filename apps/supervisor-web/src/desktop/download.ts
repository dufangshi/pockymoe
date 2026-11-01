/** Imported on demand by existing browser download flows. */
export async function nativeSave(blob: Blob, name: string): Promise<boolean> {
  const host = window.remoteCodexHost;
  if (!host || !(await host.capabilities()).features.nativeSave) return false;
  await host.files.saveAs(name, await blob.arrayBuffer());
  // Cancellation is handled by the native dialog; do not fall through to a web download.
  return true;
}

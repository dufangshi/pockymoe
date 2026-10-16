export interface PreviewLease { leaseId: string; expiresAt?: number | undefined }

// Older CLI links have no expiry metadata. The device remains authoritative
// about lease validity; this only decides whether the browser must renew it.
export function reusablePreviewLease(lease: PreviewLease | undefined): boolean {
  return !!lease?.leaseId && (lease.expiresAt === undefined ||
    (Number.isSafeInteger(lease.expiresAt) && lease.expiresAt > Date.now()));
}

export function previewLeaseFromQuery(query: URLSearchParams): PreviewLease | undefined {
  const leaseId = query.get('lease');
  if (!leaseId) return undefined;
  const expiry = query.get('leaseExpiresAt');
  const expiresAt = expiry && /^\d+$/.test(expiry) && Number.isSafeInteger(Number(expiry)) ? Number(expiry) : undefined;
  return { leaseId, ...(expiresAt === undefined ? {} : { expiresAt }) };
}

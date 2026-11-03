import { useEffect } from 'react';

/** Metadata updates retain the array. Only releasing its owner releases blobs. */
export function useOptimisticAttachmentPreviews(previews: readonly { url: string }[] | undefined) {
  useEffect(() => () => {
    for (const preview of previews ?? []) URL.revokeObjectURL(preview.url);
  }, [previews]);
}

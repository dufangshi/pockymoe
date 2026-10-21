interface Anchor {
  left: number;
  right: number;
  top: number;
  bottom: number;
}
interface Size { width: number; height: number }
export function popoverPosition(
  anchor: Anchor,
  popover: Size,
  viewport: Size & { left?: number; top?: number },
  margin = 12,
  gap = 8,
  align: 'start' | 'end' = 'end',
) {
  const minX = (viewport.left ?? 0) + margin;
  const minY = (viewport.top ?? 0) + margin;
  const maxWidth = Math.max(0, viewport.width - 2 * margin);
  const maxHeight = Math.max(0, viewport.height - 2 * margin);
  const width = Math.min(popover.width, maxWidth);
  const height = Math.min(popover.height, maxHeight);
  const maxX = minX + maxWidth - width;
  const maxY = minY + maxHeight - height;
  // Preserve the existing desktop end alignment; flip to start alignment if
  // the trigger is on the left, then clamp even when neither alignment fits.
  const end = anchor.right - width;
  const left = Math.min(maxX, Math.max(minX, align === 'start' || end < minX ? anchor.left : end));
  const below = anchor.bottom + gap;
  const above = anchor.top - gap - height;
  const top = Math.min(maxY, Math.max(minY, below > maxY ? above : below));
  return { left, top, maxWidth, maxHeight };
}

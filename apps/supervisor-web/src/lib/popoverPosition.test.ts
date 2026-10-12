// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { popoverPosition } from './popoverPosition';
describe('device trust popover placement', () => {
  it('preserves end-aligned placement below the trigger on desktop', () => {
    expect(popoverPosition({ left: 1200, right: 1232, top: 100, bottom: 132 },
      { width: 360, height: 260 }, { width: 1440, height: 900 })).toMatchObject({ left: 872, top: 140 });
  });
  it.each([393, 320])('flips alignment at the left edge and stays inside a %ipx phone viewport', (width) => {
    const placement = popoverPosition({ left: 24, right: 56, top: 570, bottom: 602 },
      { width: 360, height: 300 }, { width, height: 727 });
    expect(placement.left).toBeGreaterThanOrEqual(12);
    expect(placement.left + Math.min(360, placement.maxWidth)).toBeLessThanOrEqual(width - 12);
    expect(placement.top).toBe(262);
    expect(placement.top + 300).toBeLessThanOrEqual(715);
  });
  it('clamps both axes and makes oversized content scrollable', () => {
    expect(popoverPosition({ left: -40, right: -8, top: 5, bottom: 37 },
      { width: 600, height: 900 }, { width: 320, height: 200 })).toEqual({
      left: 12, top: 12, maxWidth: 296, maxHeight: 176,
    });
  });
  it('keeps placement inside the visual viewport when zoomed or the keyboard is open', () => {
    const result = popoverPosition({ left: 50, right: 82, top: 390, bottom: 422 },
      { width: 360, height: 240 }, { left: 30, top: 120, width: 320, height: 300 });
    expect(result).toEqual({ left: 42, top: 142, maxWidth: 296, maxHeight: 276 });
  });
});

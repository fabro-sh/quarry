import { describe, expect, it } from 'vitest';
import { cursorColorWithAlpha } from './RemoteCursorOverlay';

describe('cursorColorWithAlpha', () => {
  it('adds clamped alpha to hex cursor colors', () => {
    expect(cursorColorWithAlpha('#336699', 0.35)).toBe('#33669959');
    expect(cursorColorWithAlpha('#336699', 2)).toBe('#336699FF');
    expect(cursorColorWithAlpha('#336699', -1)).toBe('#33669900');
  });

  it('preserves functional color formats with alpha', () => {
    expect(cursorColorWithAlpha('rgb(10, 20, 30)', 0.5)).toBe('rgba(10, 20, 30, 0.5)');
    expect(cursorColorWithAlpha('hsl(200 90% 40%)', 0.25)).toBe('hsla(200 90% 40%, 0.25)');
  });
});

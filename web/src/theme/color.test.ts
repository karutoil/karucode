import { describe, expect, it } from 'vitest';
import {
  contrastRatio,
  hexToRgb,
  isDarkColor,
  mix,
  normalizeHex,
  readableTextOn,
  relativeLuminance,
  rgbToHex,
  withAlpha,
} from './color';

describe('normalizeHex', () => {
  it('accepts short, long, bare and upper-case hex values', () => {
    expect(normalizeHex('#abc')).toBe('#aabbcc');
    expect(normalizeHex('ABC')).toBe('#aabbcc');
    expect(normalizeHex('  #A1B2C3  ')).toBe('#a1b2c3');
  });

  it('rejects everything else', () => {
    for (const value of ['', '#12', '#12345', 'red', 'rgb(0,0,0)', null, 42, undefined]) {
      expect(normalizeHex(value)).toBeNull();
    }
  });
});

describe('mix', () => {
  it('returns the endpoints and a channel-wise midpoint', () => {
    expect(mix('#000000', '#ffffff', 0)).toBe('#000000');
    expect(mix('#000000', '#ffffff', 1)).toBe('#ffffff');
    expect(mix('#000000', '#ffffff', 0.5)).toBe('#808080');
  });

  it('clamps amounts outside 0–1', () => {
    expect(mix('#000000', '#ffffff', -4)).toBe('#000000');
    expect(mix('#000000', '#ffffff', 4)).toBe('#ffffff');
  });

  it('throws for a non-colour so palette bugs fail loudly', () => {
    expect(() => mix('nope', '#ffffff', 0.5)).toThrow();
  });
});

describe('contrast', () => {
  it('matches the WCAG extremes', () => {
    expect(contrastRatio('#000000', '#ffffff')).toBeCloseTo(21, 5);
    expect(contrastRatio('#1c1d1f', '#1c1d1f')).toBeCloseTo(1, 5);
  });

  it('computes relative luminance in sRGB', () => {
    expect(relativeLuminance('#ffffff')).toBeCloseTo(1, 5);
    expect(relativeLuminance('#000000')).toBeCloseTo(0, 5);
  });

  it('classifies dark and light surfaces', () => {
    expect(isDarkColor('#111213')).toBe(true);
    expect(isDarkColor('#f7f8fb')).toBe(false);
  });

  it('picks readable button text', () => {
    expect(readableTextOn('#dfb680')).toBe('#0b0d0f');
    expect(readableTextOn('#0c1221')).toBe('#ffffff');
  });
});

describe('rgb helpers', () => {
  it('round-trips through rgb', () => {
    const rgb = hexToRgb('#dfb680');
    expect(rgb).toEqual([223, 182, 128]);
    expect(rgbToHex(rgb)).toBe('#dfb680');
  });

  it('renders rgba for overlays', () => {
    expect(withAlpha('#111213', 0.5)).toBe('rgba(17, 18, 19, 0.50)');
    expect(withAlpha('#111213', 4)).toBe('rgba(17, 18, 19, 1.00)');
  });
});
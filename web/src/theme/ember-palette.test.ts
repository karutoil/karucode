// @vitest-environment node
/* Ember palette fidelity test.
 *
 * Verifies that the Ember theme in builtins.ts matches the exact color palette
 * from the karucode-v3-themes.html mock. The mock defines root CSS variables
 * that Ember must reproduce byte-for-byte.
 */

import { describe, expect, it } from 'vitest';
import { DEFAULT_THEME, findBuiltin } from './builtins';

/**
 * Expected Ember palette from karucode-v3-themes.html :root block.
 * These are the exact values from the mock's CSS variables.
 */
const MOCK_EMBER_PALETTE = {
  // Backgrounds (from mock :root)
  bg: '#111213',        // --bg
  rail: '#141516',      // --rail
  sidebar: '#18191a',   // --sidebar
  inspector: '#171819', // --inspector
  surface: '#1c1d1f',   // --surface
  // Surface variants (from --surface2, --surface3)
  raised: '#222326',    // --surface2
  hover: '#27292b',     // --surface3
  // Lines (from --line, --softline, --linehi)
  border: '#343638',    // --linehi
  // Text (from --text, --text2, --muted, --dim)
  text: '#eae9e5',      // --text
  muted: '#8b8d8c',     // --muted
  // Accent (from --gold, --goldhi)
  accent: '#dfb680',    // --gold
  // State colors (from --green, --red, --blue)
  success: '#88baa0',   // --green
  danger: '#e49e91',    // --red
  info: '#92b3d8',      // --blue
  // Additional (from --goldhi for warn)
  warn: '#f1c691',      // --goldhi
  // Syntax highlighting (matching the palette)
  syntaxKeyword: '#dfb680',  // --gold
  syntaxString: '#88baa0',   // --green
  syntaxComment: '#8b8d8c',  // --muted
} as const;

describe('Ember palette matches the v3 mock exactly', () => {
  it('is the default theme', () => {
    expect(DEFAULT_THEME.id).toBe('ember');
  });

  it('matches every background color from the mock', () => {
    const ember = DEFAULT_THEME;
    expect(ember.colors.bg).toBe(MOCK_EMBER_PALETTE.bg);
    expect(ember.colors.rail).toBe(MOCK_EMBER_PALETTE.rail);
    expect(ember.colors.sidebar).toBe(MOCK_EMBER_PALETTE.sidebar);
    expect(ember.colors.inspector).toBe(MOCK_EMBER_PALETTE.inspector);
    expect(ember.colors.surface).toBe(MOCK_EMBER_PALETTE.surface);
    expect(ember.colors.raised).toBe(MOCK_EMBER_PALETTE.raised);
    expect(ember.colors.hover).toBe(MOCK_EMBER_PALETTE.hover);
    expect(ember.colors.border).toBe(MOCK_EMBER_PALETTE.border);
  });

  it('matches every text color from the mock', () => {
    const ember = DEFAULT_THEME;
    expect(ember.colors.text).toBe(MOCK_EMBER_PALETTE.text);
    expect(ember.colors.muted).toBe(MOCK_EMBER_PALETTE.muted);
  });

  it('matches the accent color from the mock', () => {
    const ember = DEFAULT_THEME;
    expect(ember.colors.accent).toBe(MOCK_EMBER_PALETTE.accent);
  });

  it('matches state colors from the mock', () => {
    const ember = DEFAULT_THEME;
    expect(ember.colors.success).toBe(MOCK_EMBER_PALETTE.success);
    expect(ember.colors.danger).toBe(MOCK_EMBER_PALETTE.danger);
    expect(ember.colors.info).toBe(MOCK_EMBER_PALETTE.info);
    expect(ember.colors.warn).toBe(MOCK_EMBER_PALETTE.warn);
  });

  it('matches syntax highlighting colors', () => {
    const ember = DEFAULT_THEME;
    expect(ember.colors.syntaxKeyword).toBe(MOCK_EMBER_PALETTE.syntaxKeyword);
    expect(ember.colors.syntaxString).toBe(MOCK_EMBER_PALETTE.syntaxString);
    expect(ember.colors.syntaxComment).toBe(MOCK_EMBER_PALETTE.syntaxComment);
  });

  it('is byte-for-byte identical to the mock palette', () => {
    const ember = DEFAULT_THEME;
    const colors = ember.colors;
    
    // Every color must match exactly
    for (const [key, expectedValue] of Object.entries(MOCK_EMBER_PALETTE)) {
      const actualValue = colors[key as keyof typeof colors];
      expect(
        actualValue,
        `colors.${key} must match mock value ${expectedValue}`
      ).toBe(expectedValue);
    }
  });

  it('can be found by id', () => {
    const ember = findBuiltin('ember');
    expect(ember).toBeDefined();
    expect(ember?.colors.bg).toBe(MOCK_EMBER_PALETTE.bg);
  });

  it('maintains the mock color relationships', () => {
    const ember = DEFAULT_THEME;
    
    // Verify the progression from bg through rail/sidebar/inspector
    const bgLum = parseInt(ember.colors.bg.slice(1), 16);
    const railLum = parseInt(ember.colors.rail.slice(1), 16);
    const sidebarLum = parseInt(ember.colors.sidebar.slice(1), 16);
    
    expect(railLum).toBeGreaterThan(bgLum);
    expect(sidebarLum).toBeGreaterThan(railLum);
    
    // Verify surface progression
    const surfaceLum = parseInt(ember.colors.surface.slice(1), 16);
    const raisedLum = parseInt(ember.colors.raised.slice(1), 16);
    const hoverLum = parseInt(ember.colors.hover.slice(1), 16);
    
    expect(raisedLum).toBeGreaterThan(surfaceLum);
    expect(hoverLum).toBeGreaterThan(raisedLum);
  });
});

describe('Mock palette derived values', () => {
  it('documents the mock secondary palette values for reference', () => {
    // These are from the mock :root but not directly in the ThemeColors type.
    // They are used to derive the role layer or are implicit in the design.
    const mockSecondaryValues = {
      // Additional surface levels from mock (not in ThemeColors)
      surface2: '#222326',  // same as raised
      surface3: '#27292b',  // same as hover
      // Line variants from mock
      line: '#2a2c2f',      // base line color
      softline: '#242628',  // softer line
      linehi: '#343638',    // same as border
      // Text variants from mock
      text2: '#b4b3ae',     // secondary text (between text and muted)
      dim: '#676b6b',       // dimmest text
      // Gold variants from mock
      goldhi: '#f1c691',    // same as warn
      goldbg: '#32291e',    // gold background tint
      goldline: '#58452e',  // gold border
      // Green variant from mock
      greenbg: '#1b3029',   // success background tint
    };
    
    // This test just documents the values for reference
    expect(mockSecondaryValues.surface2).toBe('#222326');
  });
});

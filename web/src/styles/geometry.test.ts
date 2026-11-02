/**
 * Computed geometry tests: verify that the CSS files match karucode-v3-themes.html
 * dimensions exactly.
 *
 * Reference: karucode-v3-themes.html (Theme Studio mock, lines 9-19):
 *   --railw:64px; --sidew:258px; --rightw:342px; --radius:10px;
 *   .brand{height:70px}
 *   .project-btn{width:42px;height:42px}
 *   .main-head{height:66px;min-height:66px;padding:0 26px;gap:18px}
 *   .workspace-tabs{height:44px;min-height:44px;padding:0 26px;gap:26px}
 *   .main-title{font-size:14px}
 *   .msg{gap:14px;margin:0 0 33px}
 *   .msg-avatar{height:29px;width:29px;border-radius:10px}
 *   .thread-inner{max-width:820px}
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

describe('Layout geometry matches karucode-v3-themes.html', () => {
  describe('CSS Custom Properties in tokens.css', () => {
    const tokensCSS = readFileSync(resolve(__dirname, 'tokens.css'), 'utf-8');

    it('should have --rail-w: 64px (mock: --railw:64px)', () => {
      expect(tokensCSS).toMatch(/--rail-w:\s*64px/);
    });

    it('should have --sidebar-w: 258px (mock: --sidew:258px)', () => {
      expect(tokensCSS).toMatch(/--sidebar-w:\s*258px/);
    });

    it('should have --inspector-w: 342px (mock: --rightw:342px)', () => {
      expect(tokensCSS).toMatch(/--inspector-w:\s*342px/);
    });

    it('should have --radius: 10px', () => {
      expect(tokensCSS).toMatch(/--radius:\s*10px/);
    });

    it('should have --content-max: 820px (mock: .thread-inner{max-width:820px})', () => {
      expect(tokensCSS).toMatch(/--content-max:\s*820px/);
    });

    it('should have --header-h: 66px (mock: .main-head{height:66px})', () => {
      expect(tokensCSS).toMatch(/--header-h:\s*66px/);
    });

    it('should have --tabs-h: 44px (mock: .workspace-tabs{height:44px})', () => {
      expect(tokensCSS).toMatch(/--tabs-h:\s*44px/);
    });

    it('should have --lh-message: 1.72 (mock: .msg-body{line-height:1.72})', () => {
      expect(tokensCSS).toMatch(/--lh-message:\s*1\.72/);
    });
  });

  describe('Project Rail dimensions in rail.css', () => {
    const railCSS = readFileSync(resolve(__dirname, 'rail.css'), 'utf-8');

    it('should have --project-rail-item: 42px (mock: .project-btn{width:42px;height:42px})', () => {
      expect(railCSS).toMatch(/--project-rail-item:\s*42px/);
    });

    it('should have --project-rail-glyph: 35px (mock: .project-glyph{width:35px;height:35px})', () => {
      expect(railCSS).toMatch(/--project-rail-glyph:\s*35px/);
    });

    it('.project-rail-brand should have height: 70px (mock: .brand{height:70px})', () => {
      expect(railCSS).toMatch(/\.project-rail-brand\s*\{[^}]*height:\s*70px/s);
    });

    it('.brand-mark should be 34x34 with border-radius: 11px', () => {
      const brandMarkRule = railCSS.match(/\.brand-mark\s*\{[^}]+\}/s)?.[0] || '';
      expect(brandMarkRule).toMatch(/width:\s*34px/);
      expect(brandMarkRule).toMatch(/height:\s*34px/);
      expect(brandMarkRule).toMatch(/border-radius:\s*11px/);
    });

    it('.project-rail-item should have border-radius: 13px (mock: .project-btn)', () => {
      const itemMatch = railCSS.match(/^\.project-rail-item\s*\{([^}]+)\}/m);
      expect(itemMatch).toBeTruthy();
      if (itemMatch) {
        expect(itemMatch[1]).toContain('border-radius: 13px');
      }
    });

    it('.project-rail-avatar should have border-radius: 11px (mock: .project-glyph)', () => {
      const avatarMatch = railCSS.match(/^\.project-rail-avatar\s*\{([^}]+)\}/m);
      expect(avatarMatch).toBeTruthy();
      if (avatarMatch) {
        expect(avatarMatch[1]).toContain('border-radius: 11px');
      }
    });
  });

  describe('Main Header dimensions in layout.css', () => {
    const layoutCSS = readFileSync(resolve(__dirname, 'layout.css'), 'utf-8');

    it('.main-head should use var(--header-h) for height and min-height', () => {
      // Extract the CSS rule by finding balanced braces
      const start = layoutCSS.indexOf('.main-head {');
      expect(start).toBeGreaterThan(-1);
      let braceCount = 0;
      let end = start;
      for (let i = start; i < layoutCSS.length; i++) {
        if (layoutCSS[i] === '{') braceCount++;
        if (layoutCSS[i] === '}') {
          braceCount--;
          if (braceCount === 0) {
            end = i + 1;
            break;
          }
        }
      }
      const mainHeadSection = layoutCSS.substring(start, end);
      expect(mainHeadSection).toContain('height: var(--header-h)');
      expect(mainHeadSection).toContain('min-height: var(--header-h)');
    });

    it('.main-head should have padding: 0 26px (mock)', () => {
      const mainHeadMatch = layoutCSS.match(/^\.main-head\s*\{([^}]+)\}/m);
      expect(mainHeadMatch).toBeTruthy();
      if (mainHeadMatch) {
        expect(mainHeadMatch[1]).toMatch(/padding:\s*0\s+26px/);
      }
    });

    it('.main-head should have gap: 18px (mock)', () => {
      const mainHeadMatch = layoutCSS.match(/^\.main-head\s*\{([^}]+)\}/m);
      expect(mainHeadMatch).toBeTruthy();
      if (mainHeadMatch) {
        expect(mainHeadMatch[1]).toContain('gap: 18px');
      }
    });

    it('.main-title should have font-size: 14px', () => {
      const mainTitleMatch = layoutCSS.match(/^\.main-title\s*\{([^}]+)\}/m);
      expect(mainTitleMatch).toBeTruthy();
      if (mainTitleMatch) {
        expect(mainTitleMatch[1]).toContain('font-size: 14px');
      }
    });
  });

  describe('Workspace Tabs dimensions in layout.css', () => {
    const layoutCSS = readFileSync(resolve(__dirname, 'layout.css'), 'utf-8');

    it('.workspace-tabbar should use var(--tabs-h) for height and min-height', () => {
      const tabbarRule = layoutCSS.match(/\.workspace-tabbar\s*\{[^}]+\}/s)?.[0] || '';
      expect(tabbarRule).toMatch(/height:\s*var\(--tabs-h\)/);
      expect(tabbarRule).toMatch(/min-height:\s*var\(--tabs-h\)/);
    });

    it('.workspace-tabbar should have padding: 0 26px (mock)', () => {
      expect(layoutCSS).toMatch(/\.workspace-tabbar\s*\{[^}]*padding:\s*0\s+26px/s);
    });

    it('.workspace-tabbar should have gap: 26px (mock)', () => {
      expect(layoutCSS).toMatch(/\.workspace-tabbar\s*\{[^}]*gap:\s*26px/s);
    });

    it('.workspace-tabs should have gap: 26px', () => {
      expect(layoutCSS).toMatch(/\.workspace-tabs\s*\{[^}]*gap:\s*26px/s);
    });
  });

  describe('Chat Message dimensions in chat.css', () => {
    const chatCSS = readFileSync(resolve(__dirname, 'chat.css'), 'utf-8');

    it('.msg should have gap: 14px (mock)', () => {
      // Extract just the .msg rule
      const msgMatch = chatCSS.match(/^\.msg\s*\{([^}]+)\}/m);
      expect(msgMatch).toBeTruthy();
      if (msgMatch) {
        expect(msgMatch[1]).toContain('gap: 14px');
      }
    });

    it('.msg should have margin: 0 0 33px (mock)', () => {
      // Extract just the .msg rule
      const msgMatch = chatCSS.match(/^\.msg\s*\{([^}]+)\}/m);
      expect(msgMatch).toBeTruthy();
      if (msgMatch) {
        expect(msgMatch[1]).toMatch(/margin:\s*0\s+0\s+33px/);
      }
    });

    it('.msg-avatar should be 29x29 (mock)', () => {
      const avatarMatch = chatCSS.match(/^\.msg-avatar\s*\{([^}]+)\}/m);
      expect(avatarMatch).toBeTruthy();
      if (avatarMatch) {
        expect(avatarMatch[1]).toContain('width: 29px');
        expect(avatarMatch[1]).toContain('height: 29px');
        expect(avatarMatch[1]).toMatch(/flex:\s*0\s+0\s+29px/);
      }
    });

    it('.msg-avatar should have border-radius: 10px (mock)', () => {
      const avatarMatch = chatCSS.match(/^\.msg-avatar\s*\{([^}]+)\}/m);
      expect(avatarMatch).toBeTruthy();
      if (avatarMatch) {
        expect(avatarMatch[1]).toContain('border-radius: 10px');
      }
    });
  });

  describe('Content Column dimensions', () => {
    const layoutCSS = readFileSync(resolve(__dirname, 'layout.css'), 'utf-8');
    const chatCSS = readFileSync(resolve(__dirname, 'chat.css'), 'utf-8');

    it('.chat-inner should have padding: 30px 26px 18px (mock: .transcript)', () => {
      expect(layoutCSS).toMatch(/\.chat-inner\s*\{[^}]*padding:\s*30px\s+26px\s+18px/s);
    });

    it('.chat-inner should use calc with --content-max for max-width', () => {
      const chatInnerRule = chatCSS.match(/\.chat-inner\s*\{[^}]+\}/s)?.[0] || '';
      expect(chatInnerRule).toMatch(/max-width:\s*calc\(var\(--content-max\)/);
    });
  });

  describe('Responsive Breakpoints', () => {
    const tokensCSS = readFileSync(resolve(__dirname, 'tokens.css'), 'utf-8');

    it('should adjust --sidebar-w to 234px at 1350px breakpoint', () => {
      const mediaQuery1350 = tokensCSS.match(/@media\s*\(max-width:\s*1350px\)\s*\{[^}]+\}/s)?.[0] || '';
      expect(mediaQuery1350).toMatch(/--sidebar-w:\s*234px/);
    });

    it('should adjust --inspector-w to 305px at 1350px breakpoint', () => {
      const mediaQuery1350 = tokensCSS.match(/@media\s*\(max-width:\s*1350px\)\s*\{[^}]+\}/s)?.[0] || '';
      expect(mediaQuery1350).toMatch(/--inspector-w:\s*305px/);
    });

    it('should adjust --sidebar-w to 222px at 1100px breakpoint', () => {
      const match = tokensCSS.match(/@media\s*\(max-width:\s*1100px\)\s*\{[^}]+\}/s);
      expect(match).toBeTruthy();
      const mediaQuery1100 = match?.[0] || '';
      expect(mediaQuery1100).toMatch(/--sidebar-w:\s*222px/);
    });

    it('should adjust --inspector-w to 300px at 1100px breakpoint', () => {
      const match = tokensCSS.match(/@media\s*\(max-width:\s*1100px\)\s*\{[^}]+\}/s);
      expect(match).toBeTruthy();
      const mediaQuery1100 = match?.[0] || '';
      expect(mediaQuery1100).toMatch(/--inspector-w:\s*300px/);
    });
  });

  describe('Mock reference documentation', () => {
    const tokensCSS = readFileSync(resolve(__dirname, 'tokens.css'), 'utf-8');
    const layoutCSS = readFileSync(resolve(__dirname, 'layout.css'), 'utf-8');
    const railCSS = readFileSync(resolve(__dirname, 'rail.css'), 'utf-8');

    it('tokens.css should reference karucode-v3-themes.html in comments', () => {
      expect(tokensCSS).toMatch(/karucode-v3-themes\.html/);
    });

    it('should document mock values in comments near the CSS properties', () => {
      expect(tokensCSS).toMatch(/Mock reference/);
      expect(layoutCSS).toMatch(/Mock reference/);
      expect(railCSS).toMatch(/Mock reference/);
    });
  });
});

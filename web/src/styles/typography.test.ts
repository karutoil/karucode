/**
 * Typography tests — verify base.css and tokens.css match karucode-v3-themes.html exactly.
 *
 * The v3 mock specifies 13px/1.5 base, Inter font stack, and specific font-sizes
 * and weights for different roles (eyebrow 10px 720 weight; breadcrumb 11px;
 * main-title 14px 660 weight; etc). This test suite verifies every typographic
 * primitive matches those specs.
 */
import { readFileSync } from 'fs';
import { describe, expect, it } from 'vitest';

const tokensCSS = readFileSync('src/styles/tokens.css', 'utf-8');
const baseCSS = readFileSync('src/styles/base.css', 'utf-8');
const layoutCSS = readFileSync('src/styles/layout.css', 'utf-8');

describe('Typography tokens in tokens.css', () => {
  it('defines 13px base font size', () => {
    expect(tokensCSS).toMatch(/--fs-body:\s*13px/);
  });

  it('defines 1.5 base line height', () => {
    expect(tokensCSS).toMatch(/--lh-body:\s*1\.5/);
  });

  it('defines 11.5px small font size', () => {
    expect(tokensCSS).toMatch(/--fs-small:\s*11\.5px/);
  });

  it('defines 10px micro font size (for eyebrows, badges)', () => {
    expect(tokensCSS).toMatch(/--fs-micro:\s*10px/);
  });

  it('defines 12.5px message font size', () => {
    expect(tokensCSS).toMatch(/--fs-message:\s*12\.5px/);
  });

  it('defines 1.72 message line height', () => {
    expect(tokensCSS).toMatch(/--lh-message:\s*1\.72/);
  });

  it('defines Inter font stack for UI', () => {
    expect(tokensCSS).toMatch(/--font-ui:/);
    expect(tokensCSS).toMatch(/Inter/);
    expect(tokensCSS).toMatch(/ui-sans-serif/);
  });

  it('defines monospace font stack with SFMono-Regular and Consolas', () => {
    expect(tokensCSS).toMatch(/--font-mono:/);
    expect(tokensCSS).toMatch(/SFMono-Regular/);
    expect(tokensCSS).toMatch(/Consolas/);
  });
});

describe('body element typography in base.css', () => {
  it('uses --fs-body for font-size', () => {
    const bodyRule = baseCSS.match(/body\s*{[^}]+}/s);
    expect(bodyRule).toBeTruthy();
    expect(bodyRule![0]).toMatch(/font-size:\s*var\(--fs-body\)/);
  });

  it('uses --lh-body for line-height', () => {
    const bodyRule = baseCSS.match(/body\s*{[^}]+}/s);
    expect(bodyRule).toBeTruthy();
    expect(bodyRule![0]).toMatch(/line-height:\s*var\(--lh-body\)/);
  });

  it('uses --font-ui for font-family', () => {
    const bodyRule = baseCSS.match(/body\s*{[^}]+}/s);
    expect(bodyRule).toBeTruthy();
    expect(bodyRule![0]).toMatch(/font-family:\s*var\(--font-ui\)/);
  });
});

describe('Eyebrow typography in base.css', () => {
  it('uses --fs-micro (10px) for font-size', () => {
    const eyebrowRule = baseCSS.match(/\.eyebrow\s*{[^}]+}/s);
    expect(eyebrowRule).toBeTruthy();
    expect(eyebrowRule![0]).toMatch(/font-size:\s*var\(--fs-micro\)/);
  });

  it('uses 720 font-weight', () => {
    const eyebrowRule = baseCSS.match(/\.eyebrow\s*{[^}]+}/s);
    expect(eyebrowRule).toBeTruthy();
    expect(eyebrowRule![0]).toMatch(/font-weight:\s*720/);
  });

  it('uses uppercase transform', () => {
    const eyebrowRule = baseCSS.match(/\.eyebrow\s*{[^}]+}/s);
    expect(eyebrowRule).toBeTruthy();
    expect(eyebrowRule![0]).toMatch(/text-transform:\s*uppercase/);
  });

  it('uses 0.125em letter-spacing', () => {
    const eyebrowRule = baseCSS.match(/\.eyebrow\s*{[^}]+}/s);
    expect(eyebrowRule).toBeTruthy();
    expect(eyebrowRule![0]).toMatch(/letter-spacing:\s*0\.125em/);
  });
});

describe('Breadcrumb typography in layout.css', () => {
  it('uses 12px font-size', () => {
    const breadcrumbRule = layoutCSS.match(/\.breadcrumb\s*{[^}]+}/s);
    expect(breadcrumbRule).toBeTruthy();
    expect(breadcrumbRule![0]).toMatch(/font-size:\s*12px/);
  });
});

describe('Main title typography in layout.css', () => {
  it('uses 14px font-size', () => {
    const mainTitleRule = layoutCSS.match(/\.main-title\s*{[^}]+}/s);
    expect(mainTitleRule).toBeTruthy();
    expect(mainTitleRule![0]).toMatch(/font-size:\s*14px/);
  });

  it('uses 500 font-weight', () => {
    const mainTitleRule = layoutCSS.match(/\.main-title\s*{[^}]+}/s);
    expect(mainTitleRule).toBeTruthy();
    expect(mainTitleRule![0]).toMatch(/font-weight:\s*500/);
  });

  it('uses 1.5 line-height', () => {
    const mainTitleRule = layoutCSS.match(/\.main-title\s*{[^}]+}/s);
    expect(mainTitleRule).toBeTruthy();
    expect(mainTitleRule![0]).toMatch(/line-height:\s*1\.5/);
  });
});

describe('Button typography in base.css', () => {
  it('uses --fs-small (11.5px)', () => {
    const buttonRule = baseCSS.match(/\.button\s*{[^}]+}/s);
    expect(buttonRule).toBeTruthy();
    expect(buttonRule![0]).toMatch(/font-size:\s*var\(--fs-small\)/);
  });

  it('primary buttons use 660 font-weight', () => {
    const primaryRule = baseCSS.match(/\.button-primary\s*{[^}]+}/s);
    expect(primaryRule).toBeTruthy();
    expect(primaryRule![0]).toMatch(/font-weight:\s*660/);
  });

  it('danger buttons use 660 font-weight', () => {
    const dangerRule = baseCSS.match(/\.button-danger\s*{[^}]+}/s);
    expect(dangerRule).toBeTruthy();
    expect(dangerRule![0]).toMatch(/font-weight:\s*660/);
  });

  it('small buttons use 11.5px directly', () => {
    const smallRule = baseCSS.match(/\.button-small\s*{[^}]+}/s);
    expect(smallRule).toBeTruthy();
    expect(smallRule![0]).toMatch(/font-size:\s*11\.5px/);
  });
});

describe('Input typography in base.css', () => {
  it('uses --fs-small (11.5px)', () => {
    const inputRule = baseCSS.match(/\.input\s*{[^}]+}/s);
    expect(inputRule).toBeTruthy();
    expect(inputRule![0]).toMatch(/font-size:\s*var\(--fs-small\)/);
  });

  it('mono inputs use 12.5px and --font-mono', () => {
    const monoRule = baseCSS.match(/\.input\.mono,\s*input\.mono\s*{[^}]+}/s);
    expect(monoRule).toBeTruthy();
    expect(monoRule![0]).toMatch(/font-family:\s*var\(--font-mono\)/);
    expect(monoRule![0]).toMatch(/font-size:\s*12\.5px/);
  });
});

describe('Code and monospace elements in base.css', () => {
  it('code, kbd, pre, samp use --font-mono', () => {
    const monoRule = baseCSS.match(/code,\s*kbd,\s*pre,\s*samp\s*{[^}]+}/s);
    expect(monoRule).toBeTruthy();
    expect(monoRule![0]).toMatch(/font-family:\s*var\(--font-mono\)/);
  });

  it('code uses 0.92em font-size', () => {
    const codeRule = baseCSS.match(/\ncode\s*{[^}]+}/s);
    expect(codeRule).toBeTruthy();
    expect(codeRule![0]).toMatch(/font-size:\s*0\.92em/);
  });

  it('kbd uses --fs-micro (10px) and --font-mono', () => {
    const kbdRule = baseCSS.match(/kbd,\s*\.keycap\s*{[^}]+}/s);
    expect(kbdRule).toBeTruthy();
    expect(kbdRule![0]).toMatch(/font-family:\s*var\(--font-mono\)/);
    expect(kbdRule![0]).toMatch(/font-size:\s*var\(--fs-micro\)/);
    expect(kbdRule![0]).toMatch(/line-height:\s*1\.1/);
  });
});

describe('Headings typography in base.css', () => {
  it('h1-h4 use 660 font-weight', () => {
    const headingRule = baseCSS.match(/h1,\s*h2,\s*h3,\s*h4\s*{[^}]+}/s);
    expect(headingRule).toBeTruthy();
    expect(headingRule![0]).toMatch(/font-weight:\s*660/);
  });

  it('h1-h4 use 1.3 line-height', () => {
    const headingRule = baseCSS.match(/h1,\s*h2,\s*h3,\s*h4\s*{[^}]+}/s);
    expect(headingRule).toBeTruthy();
    expect(headingRule![0]).toMatch(/line-height:\s*1\.3/);
  });

  it('h1-h4 use -0.02em letter-spacing', () => {
    const headingRule = baseCSS.match(/h1,\s*h2,\s*h3,\s*h4\s*{[^}]+}/s);
    expect(headingRule).toBeTruthy();
    expect(headingRule![0]).toMatch(/letter-spacing:\s*-0\.02em/);
  });
});

describe('Badge typography in base.css', () => {
  it('uses --fs-micro (10px)', () => {
    const badgeRule = baseCSS.match(/\.badge\s*{[^}]+}/s);
    expect(badgeRule).toBeTruthy();
    expect(badgeRule![0]).toMatch(/font-size:\s*var\(--fs-micro\)/);
  });

  it('uses --font-mono', () => {
    const badgeRule = baseCSS.match(/\.badge\s*{[^}]+}/s);
    expect(badgeRule).toBeTruthy();
    expect(badgeRule![0]).toMatch(/font-family:\s*var\(--font-mono\)/);
  });

  it('uses 0.02em letter-spacing', () => {
    const badgeRule = baseCSS.match(/\.badge\s*{[^}]+}/s);
    expect(badgeRule).toBeTruthy();
    expect(badgeRule![0]).toMatch(/letter-spacing:\s*0\.02em/);
  });
});

describe('Chip typography in base.css', () => {
  it('uses 10.5px font-size', () => {
    const chipRule = baseCSS.match(/\.chip\s*{[^}]+}/s);
    expect(chipRule).toBeTruthy();
    expect(chipRule![0]).toMatch(/font-size:\s*10\.5px/);
  });

  it('chip code uses 10.5px', () => {
    const chipCodeRule = baseCSS.match(/\.chip\s+code\s*{[^}]+}/s);
    expect(chipCodeRule).toBeTruthy();
    expect(chipCodeRule![0]).toMatch(/font-size:\s*10\.5px/);
  });
});

describe('Crash card typography in base.css', () => {
  it('crash-kicker uses --fs-micro and 720 weight', () => {
    const kickerRule = baseCSS.match(/\.crash-kicker\s*{[^}]+}/s);
    expect(kickerRule).toBeTruthy();
    expect(kickerRule![0]).toMatch(/font-size:\s*var\(--fs-micro\)/);
    expect(kickerRule![0]).toMatch(/font-weight:\s*720/);
  });

  it('crash-title uses --fs-body and 660 weight', () => {
    const titleRule = baseCSS.match(/\.crash-title\s*{[^}]+}/s);
    expect(titleRule).toBeTruthy();
    expect(titleRule![0]).toMatch(/font-size:\s*var\(--fs-body\)/);
    expect(titleRule![0]).toMatch(/font-weight:\s*660/);
  });

  it('crash-message uses --font-mono and --fs-small', () => {
    const messageRule = baseCSS.match(/\.crash-message\s*{[^}]+}/s);
    expect(messageRule).toBeTruthy();
    expect(messageRule![0]).toMatch(/font-family:\s*var\(--font-mono\)/);
    expect(messageRule![0]).toMatch(/font-size:\s*var\(--fs-small\)/);
  });
});

describe('Utility classes in base.css', () => {
  it('.small uses --fs-small', () => {
    const smallRule = baseCSS.match(/\.small\s*{[^}]+}/s);
    expect(smallRule).toBeTruthy();
    expect(smallRule![0]).toMatch(/font-size:\s*var\(--fs-small\)/);
  });

  it('.mono uses --font-mono', () => {
    const monoRule = baseCSS.match(/\.mono\s*{[^}]+}/s);
    expect(monoRule).toBeTruthy();
    expect(monoRule![0]).toMatch(/font-family:\s*var\(--font-mono\)/);
  });
});

describe('Link buttons in base.css', () => {
  it('uses 11.5px font-size', () => {
    const linkButtonRule = baseCSS.match(/\.link-button\s*{[^}]+}/s);
    expect(linkButtonRule).toBeTruthy();
    expect(linkButtonRule![0]).toMatch(/font-size:\s*11\.5px/);
  });
});


import type { ArtifactThemeName } from './artifactPreview';

/**
 * Mermaid, loaded only when a diagram is actually on screen.
 *
 * Mermaid is the one dependency in this product that is bigger than everything
 * else put together (it carries d3, a layout engine and eleven diagram types), so
 * it is behind a dynamic `import()`: the chat that contains no diagram never
 * downloads it, and the one that does pays for it once, on first paint of the
 * diagram. Vite splits it into its own chunks; the main bundle's size is
 * unchanged.
 *
 * Three settings are load-bearing:
 *
 * * `securityLevel: 'strict'` makes mermaid run its own output through DOMPurify
 *   and refuse `click … call` bindings, which are script. `ArtifactCard`
 *   sanitises again on the way in.
 * * `htmlLabels: false` keeps every label a real `<text>` element. The default
 *   (`true`) wraps labels in `foreignObject`, which is HTML inside SVG - and the
 *   sanitiser that guards the transcript (correctly) drops `foreignObject` and
 *   everything in it, so a diagram rendered with HTML labels came out as empty
 *   boxes with no text at all. Verified in Chromium before the change: the
 *   diagram measured 190px wide and `textContent` held only mermaid's stylesheet.
 * * `useMaxWidth: true` scales a wide flowchart down to the card instead of
 *   making it a horizontal scrollbar.
 */

/** The slice of mermaid's API this product uses. */
export interface MermaidApi {
  initialize(config: Record<string, unknown>): void;
  render(id: string, text: string): Promise<{ svg: string }>;
}

let loader: Promise<MermaidApi> | null = null;
let renderCounter = 0;

/** The app's own UI font, so a diagram matches the prose around it. */
function uiFont(): string {
  if (typeof document === 'undefined') return 'system-ui, sans-serif';
  const value = getComputedStyle(document.documentElement).getPropertyValue('--font-ui').trim();
  return value === '' ? 'system-ui, sans-serif' : value;
}

/** One theme token, with a fallback for a document that has not been themed yet. */
function token(name: string, fallback: string): string {
  if (typeof document === 'undefined') return fallback;
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value === '' ? fallback : value;
}

/**
 * Mermaid's `base` theme, painted with the app's own palette.
 *
 * The built-in `default`/`dark` themes are their own design: lavender nodes with
 * violet borders, which look pasted-in beside a warm transcript and clash
 * outright in the light theme (measured in Chromium - `#ECECFF` boxes in the
 * middle of a `daybreak` card). `base` derives every diagram colour from these
 * variables, so a diagram is drawn out of the same surfaces, lines and accent as
 * the card around it.
 */
function themeVariables(theme: ArtifactThemeName): Record<string, string> {
  const dark = theme === 'dark';
  const surface = token('--surface', dark ? '#232120' : '#f6f4f1');
  const raised = token('--surface-raised', dark ? '#2c2a28' : '#ffffff');
  const inset = token('--surface-inset', dark ? '#2b2926' : '#efece8');
  const text = token('--text', dark ? '#f2f0ee' : '#1b1917');
  const secondary = token('--text-secondary', dark ? '#c9c5c0' : '#4a4540');
  const muted = token('--muted', dark ? '#9a948d' : '#6b655e');
  const line = token('--line', dark ? '#3a3733' : '#e0dcd6');
  const strong = token('--line-strong', dark ? '#4a4641' : '#c9c4bd');
  const accent = token('--accent', dark ? '#d7a45f' : '#a4552a');
  const font = uiFont();
  return {
    darkMode: dark ? 'true' : 'false',
    background: surface,
    fontFamily: font,
    fontSize: '13px',
    primaryColor: raised,
    primaryTextColor: text,
    primaryBorderColor: accent,
    secondaryColor: inset,
    secondaryTextColor: secondary,
    secondaryBorderColor: line,
    tertiaryColor: surface,
    tertiaryTextColor: secondary,
    tertiaryBorderColor: line,
    mainBkg: raised,
    nodeBorder: accent,
    nodeTextColor: text,
    textColor: text,
    lineColor: muted,
    edgeLabelBackground: surface,
    clusterBkg: inset,
    clusterBorder: line,
    titleColor: text,
    actorBkg: raised,
    actorBorder: strong,
    actorTextColor: text,
    actorLineColor: muted,
    signalColor: secondary,
    signalTextColor: text,
    labelBoxBkgColor: raised,
    labelBoxBorderColor: line,
    labelTextColor: text,
    loopTextColor: text,
    noteBkgColor: inset,
    noteBorderColor: line,
    noteTextColor: text,
    activationBkgColor: inset,
    activationBorderColor: strong,
    sequenceNumberColor: surface,
    pie1: accent,
    pie2: token('--success', accent),
    pie3: muted,
    pie4: strong,
  };
}

function config(theme: ArtifactThemeName): Record<string, unknown> {
  return {
    startOnLoad: false,
    securityLevel: 'strict',
    theme: 'base',
    themeVariables: themeVariables(theme),
    fontFamily: uiFont(),
    htmlLabels: false,
    // Mermaid always draws into a temporary element appended to `document.body`.
    // On a parse error the default is to draw a giant "Syntax error in text"
    // bomb into it *and* throw, leaving that SVG in the page for ever (each
    // render uses a fresh id, so mermaid's own cleanup never comes back for it).
    // The card has its own error state, so the bomb is noise - this makes mermaid
    // remove the element and throw, which is all we want from it.
    suppressErrorRendering: true,
    flowchart: { htmlLabels: false, useMaxWidth: true, curve: 'basis' },
    sequence: { useMaxWidth: true },
    gantt: { useMaxWidth: true },
    er: { useMaxWidth: true },
  };
}

export function loadMermaid(): Promise<MermaidApi> {
  if (loader) return loader;
  const started = import('mermaid').then((module) => {
    const api = ((module as { default?: MermaidApi }).default ?? module) as MermaidApi;
    api.initialize(config('light'));
    return api;
  });
  // A failed download must not be cached as the answer for ever: the next
  // diagram retries rather than showing a permanent error.
  loader = started.catch((error: unknown) => {
    loader = null;
    throw error;
  });
  return loader;
}

/**
 * Render one Mermaid definition to SVG markup.
 *
 * The id has to be unique per render: mermaid puts a measuring element into the
 * document under it and reuses it if it is still there. It is also the only thing
 * that keeps two identical diagrams from sharing one measuring node.
 */
export async function renderMermaid(
  source: string,
  theme: ArtifactThemeName,
): Promise<string> {
  const api = await loadMermaid();
  renderCounter += 1;
  const id = `karucode-mermaid-${renderCounter}`;
  // The theme is per render, so it is set again here rather than only once.
  api.initialize(config(theme));
  try {
    const { svg } = await api.render(id, source);
    return svg;
  } catch (error) {
    // Belt and braces for a mermaid that did not honour `suppressErrorRendering`:
    // never leave its measuring element (or an error diagram) in our document.
    document.getElementById(`d${id}`)?.remove();
    document.getElementById(id)?.remove();
    throw error;
  }
}

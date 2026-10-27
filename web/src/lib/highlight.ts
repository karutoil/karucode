import hljs from 'highlight.js/lib/core';
import rust from 'highlight.js/lib/languages/rust';
import typescript from 'highlight.js/lib/languages/typescript';
import javascript from 'highlight.js/lib/languages/javascript';
import python from 'highlight.js/lib/languages/python';
import bash from 'highlight.js/lib/languages/bash';
import json from 'highlight.js/lib/languages/json';
import ini from 'highlight.js/lib/languages/ini';
import yaml from 'highlight.js/lib/languages/yaml';
import css from 'highlight.js/lib/languages/css';
import xml from 'highlight.js/lib/languages/xml';

// Only the languages this product needs are registered; see package.json.
hljs.registerLanguage('rust', rust);
hljs.registerLanguage('typescript', typescript);
hljs.registerLanguage('javascript', javascript);
hljs.registerLanguage('python', python);
hljs.registerLanguage('bash', bash);
hljs.registerLanguage('json', json);
hljs.registerLanguage('ini', ini);
hljs.registerLanguage('toml', ini);
hljs.registerLanguage('yaml', yaml);
hljs.registerLanguage('css', css);
hljs.registerLanguage('xml', xml);

/** Long blocks skip highlighting so streaming stays responsive. */
const MAX_HIGHLIGHT_CHARS = 60_000;
/** File viewer gets a larger budget, but still bounded. */
export const MAX_FILE_HIGHLIGHT_CHARS = 160_000;

const ALIASES: Record<string, string> = {
  rs: 'rust',
  ts: 'typescript',
  tsx: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  py: 'python',
  python3: 'python',
  sh: 'bash',
  shell: 'bash',
  zsh: 'bash',
  console: 'bash',
  jsonc: 'json',
  json5: 'json',
  yml: 'yaml',
  toml: 'toml',
  html: 'xml',
  svg: 'xml',
  htm: 'xml',
  md: 'markdown',
};

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

export function normaliseLanguage(raw: string | null | undefined): string {
  if (!raw) return '';
  const first = raw.trim().split(/\s+/)[0] ?? '';
  const lower = first.toLowerCase();
  return ALIASES[lower] ?? lower;
}

export function languageLabel(raw: string | null | undefined): string {
  const normalised = normaliseLanguage(raw);
  return normalised === '' ? 'text' : normalised;
}

export function canHighlight(language: string): boolean {
  return language !== '' && hljs.getLanguage(language) !== undefined;
}

/**
 * Returns safe HTML for a code block. Highlighted output comes from
 * highlight.js, which escapes the source; unknown/oversized input is escaped
 * here so the result is always safe to insert under DOMPurify.
 */
export function highlightCode(
  code: string,
  language: string | null | undefined,
  maxChars = MAX_HIGHLIGHT_CHARS,
): string {
  const normalised = normaliseLanguage(language);
  if (code.length > maxChars || !canHighlight(normalised)) return escapeHtml(code);
  try {
    return hljs.highlight(code, { language: normalised, ignoreIllegals: true }).value;
  } catch {
    return escapeHtml(code);
  }
}
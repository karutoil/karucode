import MarkdownIt from 'markdown-it';
import DOMPurify from 'dompurify';
import { escapeHtml, highlightCode, languageLabel, normaliseLanguage } from './highlight';

const md = new MarkdownIt({
  html: false, // raw HTML from model output is never parsed
  linkify: true,
  breaks: false,
  typographer: false,
});

const defaultLinkOpen = md.renderer.rules.link_open;
md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
  const token = tokens[idx];
  if (token) {
    token.attrSet('target', '_blank');
    token.attrSet('rel', 'noopener noreferrer');
  }
  return defaultLinkOpen
    ? defaultLinkOpen(tokens, idx, options, env, self)
    : self.renderToken(tokens, idx, options);
};

// Fenced code gets a language label and a copy button; the handler in the
// Markdown component copies the raw textContent of the <code> element.
md.renderer.rules.fence = (tokens, idx) => {
  const token = tokens[idx];
  if (!token) return '';
  const info = token.info.trim();
  const rawLanguage = info.split(/\s+/)[0] ?? '';
  const language = normaliseLanguage(rawLanguage);
  const label = languageLabel(rawLanguage);
  const highlighted = highlightCode(token.content, language);
  const languageClass = language === '' ? '' : ` language-${escapeHtml(language)}`;
  return (
    `<div class="code-block" data-lang="${escapeHtml(label)}">` +
    `<div class="code-block-head"><span class="code-block-lang">${escapeHtml(label)}</span>` +
    `<button class="code-copy" type="button" data-copy aria-label="Copy ${escapeHtml(label)} code">Copy</button></div>` +
    `<pre class="code-block-pre"><code class="hljs${languageClass}">${highlighted}</code></pre>` +
    `</div>`
  );
};

const SANITIZE_CONFIG = {
  USE_PROFILES: { html: true },
  ADD_ATTR: ['target', 'rel', 'data-lang', 'data-copy'],
  ADD_TAGS: ['button'],
  FORBID_TAGS: [
    'style',
    'form',
    'input',
    'textarea',
    'select',
    'option',
    'iframe',
    'object',
    'embed',
    'link',
    'meta',
    'base',
  ],
  FORBID_ATTR: ['style'],
};

export function renderMarkdown(source: string): string {
  const rendered = md.render(source);
  return DOMPurify.sanitize(rendered, SANITIZE_CONFIG);
}
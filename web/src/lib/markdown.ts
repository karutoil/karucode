import MarkdownIt from 'markdown-it';
import DOMPurify from 'dompurify';
import footnote from 'markdown-it-footnote';
import taskLists from 'markdown-it-task-lists';
import { parseFence, type ArtifactSpec } from './artifacts';
import { escapeHtml, highlightCode, languageLabel, normaliseLanguage } from './highlight';

const md = new MarkdownIt({
  html: false, // raw HTML from model output is never parsed
  linkify: true,
  breaks: false,
  typographer: false,
});

/*
 * GitHub's Markdown, less the parts that need raw HTML.
 *
 * markdown-it's default preset already covers tables, `~~strikethrough~~`,
 * images, fenced code and (with `linkify`) bare-URL autolinks. What it does not
 * implement are the two block extensions GitHub added on top: task lists
 * (`- [ ] thing`) and footnotes (`text[^1]`). Without them an agent writing GFM -
 * which it has, product-wide, in its own docs and in almost every repository it
 * reads - got literal `[x]` and a stray `[^1]` in the answer.
 *
 * Both run before sanitising, so their output goes through the same DOMPurify
 * pass as everything else. `enabled: false` keeps the checkboxes read-only: this
 * is a rendered document, not a form, and a clickable box would promise a state
 * change that nothing stores.
 */
md.use(taskLists, { enabled: false, label: false });
md.use(footnote);

const defaultLinkOpen = md.renderer.rules.link_open;
md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
  const token = tokens[idx];
  const href = token?.attrGet('href') ?? '';
  // A fragment ("#fn1") is a place in *this* document - a footnote reference or
  // its way back. Opening it in a new tab would show the reader a second copy of
  // the page instead of taking them to the note.
  if (token && !href.startsWith('#')) {
    token.attrSet('target', '_blank');
    token.attrSet('rel', 'noopener noreferrer');
  }
  return defaultLinkOpen
    ? defaultLinkOpen(tokens, idx, options, env, self)
    : self.renderToken(tokens, idx, options);
};

/**
 * The per-render state the fence rule reads.
 *
 * `undefined` for a plain `renderMarkdown` call, which is what keeps a file
 * preview, a plugin panel and everything else that renders Markdown from
 * producing artifact placeholders: only a caller that has somewhere to put a card
 * passes this.
 */
interface RenderEnv {
  artifacts: ArtifactSpec[];
  /** Off while a message is streaming: a half-written fence is not content yet. */
  allowArtifacts: boolean;
  /** Off for the block still being written: it is re-rendered on every delta. */
  allowHighlight: boolean;
}

/** The attribute a card is mounted into, and the index that finds its spec. */
export const ARTIFACT_SLOT_ATTRIBUTE = 'data-artifact-index';

/** Local, already-escaped code block. `head` is escaped markup for extra controls. */
function codeBlockHtml(
  source: string,
  language: string,
  label: string,
  options: {
    head?: string;
    languageClass?: boolean;
    copy?: boolean;
    chrome?: boolean;
    /**
     * Run the syntax highlighter. Off for the block being written: tokenising a
     * half-written fence again on every delta is one of the costs the streaming
     * split exists to avoid, and the finished fence is highlighted once.
     */
    highlight?: boolean;
  } = {},
): string {
  const highlighted =
    options.highlight === false ? escapeHtml(source) : highlightCode(source, language);
  const languageClass =
    options.languageClass === false || language === '' ? '' : ` language-${escapeHtml(language)}`;
  const safeLabel = escapeHtml(label);
  const copy =
    options.copy === false
      ? ''
      : `<button class="code-copy" type="button" data-copy aria-label="Copy ${safeLabel} code">Copy</button>`;
  const head =
    options.chrome === false
      ? ''
      : `<div class="code-block-head"><span class="code-block-lang">${safeLabel}</span>${options.head ?? ''}${copy}</div>`;
  return (
    `<div class="code-block" data-lang="${safeLabel}">` +
    head +
    `<pre class="code-block-pre"><code class="hljs${languageClass}">${highlighted}</code></pre>` +
    `</div>`
  );
}

/**
 * The Markdown the chat already rendered, for a `<pre>`-shaped caller.
 *
 * Exported so the artifact card's Source half looks exactly like the fence it
 * replaces - same label, same highlighting - instead of a second, subtly
 * different code block. `chrome: false` drops the head entirely, which is what
 * the card uses: its own header already names the artifact and owns the copy
 * button, so a second head inside the body is a duplicate.
 */
export function renderCodeBlock(
  source: string,
  language: string,
  label?: string,
  options: { copy?: boolean; chrome?: boolean } = {},
): string {
  return codeBlockHtml(source, language, label ?? languageLabel(language), options);
}

// Fenced code gets a language label and a copy button; the handler in the
// Markdown component copies the raw textContent of the <code> element. A fence
// the artifact parser recognises becomes a placeholder instead, which the
// component mounts a card into (see `ARTIFACT_SLOT_ATTRIBUTE`).
md.renderer.rules.fence = (tokens, idx, _options, env) => {
  const token = tokens[idx];
  if (!token) return '';
  const info = token.info.trim();
  const rawLanguage = info.split(/\s+/)[0] ?? '';
  const renderEnv = env as Partial<RenderEnv> | undefined;

  if (renderEnv?.allowArtifacts && Array.isArray(renderEnv.artifacts)) {
    const spec = parseFence(info, token.content);
    if (spec) {
      const index = renderEnv.artifacts.length;
      renderEnv.artifacts.push(spec);
      return `<div class="artifact-slot" ${ARTIFACT_SLOT_ATTRIBUTE}="${index}"></div>`;
    }
  }

  const language = normaliseLanguage(rawLanguage);
  return codeBlockHtml(token.content, language, languageLabel(rawLanguage), {
    highlight: renderEnv?.allowHighlight !== false,
  });
};

const SANITIZE_CONFIG = {
  USE_PROFILES: { html: true },
  // `class` is what gives a task item its checkbox and a footnote its list; the
  // checkbox itself is the only form control the renderer can produce, and the
  // hook below refuses every other `<input>`. Raw HTML is off (`html: false`), so
  // nothing else can introduce one.
  ADD_ATTR: ['target', 'rel', 'data-lang', 'data-copy', ARTIFACT_SLOT_ATTRIBUTE, 'type', 'checked', 'disabled'],
  ADD_TAGS: ['button', 'input'],
  FORBID_TAGS: [
    'style',
    'form',
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

/**
 * A task list is the only reason an `<input>` exists in rendered Markdown, so
 * anything that is not a (disabled) checkbox is dropped.
 *
 * The sanitiser's tag allow-list is per-page, and this one is shared by the chat,
 * the file preview and the artifact cards. Narrowing the *element* here keeps
 * the allow-list usable by the task-list renderer without letting a text field,
 * a submit button or an interactive checkbox through with it.
 */
if (typeof DOMPurify.addHook === 'function') {
  DOMPurify.addHook('afterSanitizeAttributes', (node) => {
    if (node.nodeName !== 'INPUT') return;
    const type = node.getAttribute('type');
    if (type !== 'checkbox') {
      node.parentNode?.removeChild(node);
      return;
    }
    // Read-only regardless of what the renderer asked for.
    node.setAttribute('disabled', '');
    node.removeAttribute('name');
    node.removeAttribute('value');
  });
}

export function renderMarkdown(source: string): string {
  const rendered = md.render(source);
  return DOMPurify.sanitize(rendered, SANITIZE_CONFIG);
}

/** One rendered message: its HTML, and the artifacts it wants drawn. */
export interface MarkdownDocument {
  html: string;
  artifacts: ArtifactSpec[];
}

export interface MarkdownRenderOptions {
  /**
   * Render recognised fences as artifact cards. Default `true`.
   *
   * Turned off for the message that is still streaming: markdown-it closes a
   * fence at the end of the input, so a half-written mock would otherwise render,
   * reload and relayout on every token. It comes back the moment the message
   * settles, which is one render later.
   */
  artifacts?: boolean;
  /**
   * Run the syntax highlighter over fenced code. Default `true`.
   *
   * Turned off for the block that is still being written, which is re-rendered on
   * every delta: tokenising half-written code again each time is one of the costs
   * the streaming split exists to remove, and a fence that has closed is in the
   * settled prefix, where it is highlighted normally.
   */
  highlight?: boolean;
}

/**
 * Render Markdown for the chat, leaving a placeholder for every artifact.
 *
 * The placeholders are empty `<div>`s carrying an index into `artifacts`; the
 * caller mounts a component into each one. They are in the HTML string rather
 * than in the component tree because the content around them is Markdown, and
 * splitting that into tokens to interleave components would mean rendering the
 * prose twice (once to find the fences, once to draw them).
 */
export function renderMarkdownDocument(
  source: string,
  options: MarkdownRenderOptions = {},
): MarkdownDocument {
  const artifacts: ArtifactSpec[] = [];
  const env: RenderEnv = {
    artifacts,
    allowArtifacts: options.artifacts !== false,
    allowHighlight: options.highlight !== false,
  };
  const rendered = md.render(source, env);
  return { html: DOMPurify.sanitize(rendered, SANITIZE_CONFIG), artifacts };
}

import { describe, expect, it } from 'vitest';
import {
  buildTable,
  describeArtifact,
  MAX_HEIGHT,
  MAX_TABLE_CHARS,
  MAX_TABLE_COLUMNS,
  MAX_TABLE_ROWS,
  MIN_HEIGHT,
  parseFence,
  parseHeaderBlock,
  parseHeight,
  parseInfoString,
  parseTable,
  sniffKind,
  tableDelimiter,
} from './artifacts';

const HTML_DOC = `<!doctype html>
<html><body><button>Buy</button></body></html>`;

describe('parseInfoString', () => {
  it('splits the umbrella word, a bare language and metadata', () => {
    expect(parseInfoString('artifact html title="Signup form" height=420')).toEqual({
      umbrella: true,
      language: 'html',
      meta: { title: 'Signup form', height: '420' },
    });
  });

  it('accepts commas between metadata, single quotes and no language', () => {
    expect(parseInfoString("artifact, title='Two words', height=300")).toEqual({
      umbrella: true,
      language: '',
      meta: { title: 'Two words', height: '300' },
    });
  });

  it('keeps a quoted value containing spaces and an equals sign', () => {
    expect(parseInfoString('svg title="a = b"').meta['title']).toBe('a = b');
  });

  it('ignores unknown metadata keys instead of treating them as the language', () => {
    const parsed = parseInfoString('html colour=red');
    expect(parsed.language).toBe('html');
    expect(parsed.meta['colour']).toBeUndefined();
  });

  it('is empty for an empty info string', () => {
    expect(parseInfoString('')).toEqual({ umbrella: false, language: '', meta: {} });
  });
});

describe('parseHeaderBlock', () => {
  it('reads keys until --- and returns the rest as the body', () => {
    const parsed = parseHeaderBlock('title: Signup form\ntype: html\n---\n<p>hi</p>');
    expect(parsed.meta).toEqual({ title: 'Signup form', type: 'html' });
    expect(parsed.body).toBe('<p>hi</p>');
  });

  it('refuses a header with no terminator, so a document is never eaten', () => {
    const content = 'Note: this is prose\n---\nand this is under a rule';
    expect(parseHeaderBlock(content)).toEqual({ meta: {}, body: content });
    // The first line is not `key: value`, so this is not a header at all.
    const prose = 'Note: this is prose\n<h1>hi</h1>';
    expect(parseHeaderBlock(prose)).toEqual({ meta: {}, body: prose });
  });

  it('does not strip a document whose first line is the rule', () => {
    const content = '---\ntitle: nope';
    expect(parseHeaderBlock(content)).toEqual({ meta: {}, body: content });
  });

  it('gives up past the line budget', () => {
    const filler = Array.from({ length: 16 }, (_, i) => `title: ${i}`).join('\n');
    const content = `${filler}\n---\nbody`;
    expect(parseHeaderBlock(content).body).toBe(content);
  });
});

describe('parseHeight', () => {
  it('clamps to the allowed range and ignores nonsense', () => {
    expect(parseHeight(undefined)).toBeNull();
    expect(parseHeight('420')).toBe(420);
    expect(parseHeight('10')).toBe(MIN_HEIGHT);
    expect(parseHeight('99999')).toBe(MAX_HEIGHT);
    expect(parseHeight('tall')).toBeNull();
  });
});

describe('sniffKind', () => {
  it('recognises the kinds an artifact fence is likely to carry', () => {
    expect(sniffKind(HTML_DOC)).toBe('html');
    expect(sniffKind('<svg viewBox="0 0 1 1"></svg>')).toBe('svg');
    expect(sniffKind('<div class="card">hi</div>')).toBe('html');
    expect(sniffKind('graph TD\n  a-->b')).toBe('mermaid');
    expect(sniffKind('sequenceDiagram\n  a->>b: hi')).toBe('mermaid');
    expect(sniffKind('{"a":1}')).toBe('json');
    expect(sniffKind('name,qty\nbolt,4')).toBe('csv');
    expect(sniffKind('# Title\n\nbody')).toBe('markdown');
    expect(sniffKind('data:image/png;base64,AAA')).toBe('image');
    expect(sniffKind('   ')).toBe('unknown');
  });
});

describe('parseFence', () => {
  it('refuses a language that is not renderable', () => {
    expect(parseFence('rust', 'fn main() {}')).toBeNull();
    expect(parseFence('', 'plain')).toBeNull();
  });

  it('leaves a small snippet as code', () => {
    expect(parseFence('html', '<div>a</div>')).toBeNull();
    expect(parseFence('json', '{"a":1}')).toBeNull();
    expect(parseFence('markdown', '# hi')).toBeNull();
    expect(parseFence('csv', 'one,two')).toBeNull();
  });

  it('turns a substantial HTML block into a card that opens on the preview', () => {
    const spec = parseFence('html', HTML_DOC);
    expect(spec).not.toBeNull();
    expect(spec?.kind).toBe('html');
    expect(spec?.language).toBe('xml');
    expect(spec?.initial).toBe('preview');
    expect(spec?.explicit).toBe(false);
    expect(spec?.label).toBe('HTML');
  });

  it('opens a document-ish HTML fragment on the preview even when short', () => {
    expect(parseFence('html', '<style>.a{}</style><div class="a">x</div>')?.kind).toBe('html');
  });

  it('keeps a markdown or JSON block on the source half', () => {
    const md = parseFence('markdown', `# Heading\n\n${'x'.repeat(300)}`);
    expect(md?.initial).toBe('source');
    const json = parseFence('json', JSON.stringify({ a: 'x'.repeat(300) }, null, 2));
    expect(json?.initial).toBe('source');
  });

  it('renders any mermaid fence, however small', () => {
    const spec = parseFence('mermaid', 'graph TD\n  a-->b');
    expect(spec?.kind).toBe('mermaid');
    expect(spec?.initial).toBe('preview');
  });

  it('only renders a full SVG element as an SVG', () => {
    expect(parseFence('svg', '<svg viewBox="0 0 10 10"><circle r="4"/></svg>')?.kind).toBe('svg');
    expect(parseFence('svg', '<circle r="4"/>')).toBeNull();
  });

  it('turns a delimited block into a table', () => {
    const spec = parseFence('csv', 'name,qty\nbolt,4\nnut,7');
    expect(spec?.kind).toBe('csv');
    expect(spec?.initial).toBe('preview');
    expect(parseFence('tsv', 'name\tqty\nbolt\t4')?.kind).toBe('tsv');
  });

  it('reads an explicit artifact fence with info-string metadata', () => {
    const spec = parseFence('artifact html title="Signup form" height=420', HTML_DOC);
    expect(spec).toEqual({
      kind: 'html',
      language: 'xml',
      label: 'HTML',
      title: 'Signup form',
      source: HTML_DOC,
      height: 420,
      explicit: true,
      initial: 'preview',
    });
  });

  it('strips a header block from an explicit artifact fence', () => {
    const spec = parseFence(
      'artifact',
      'title: Signup form\ntype: html\nheight: 300\n---\n<!doctype html><p>hi</p>',
    );
    expect(spec?.kind).toBe('html');
    expect(spec?.title).toBe('Signup form');
    expect(spec?.height).toBe(300);
    expect(spec?.source).toBe('<!doctype html><p>hi</p>');
  });

  it('lets the info string override the header block', () => {
    const spec = parseFence(
      'artifact title="From info"',
      'title: From header\ntype: svg\n---\n<svg></svg>',
    );
    expect(spec?.title).toBe('From info');
    expect(spec?.kind).toBe('svg');
  });

  it('sniffs the kind of an artifact fence that declares nothing', () => {
    expect(parseFence('artifact', 'graph TD\n  a-->b')?.kind).toBe('mermaid');
    expect(parseFence('artifact', '<svg></svg>')?.kind).toBe('svg');
    expect(parseFence('artifact', 'name,qty\nbolt,4')?.kind).toBe('csv');
  });

  it('renders even a tiny artifact fence, because the author asked for it', () => {
    const spec = parseFence('artifact', '<p>hi</p>');
    expect(spec?.explicit).toBe(true);
    expect(spec?.kind).toBe('html');
  });

  it('refuses an empty artifact fence rather than rendering an empty frame', () => {
    expect(parseFence('artifact', '   ')?.kind).toBe('unknown');
    expect(parseFence('artifact html', '')).not.toBeNull();
  });

  it('accepts the preview alias', () => {
    expect(parseFence('preview html', HTML_DOC)?.explicit).toBe(true);
  });
});

describe('parseTable', () => {
  it('reads simple rows', () => {
    expect(parseTable('a,b\n1,2')).toEqual([
      ['a', 'b'],
      ['1', '2'],
    ]);
  });

  it('handles quotes, doubled quotes and embedded delimiters', () => {
    expect(parseTable('name,note\n"Smith, J","said ""hi"""')).toEqual([
      ['name', 'note'],
      ['Smith, J', 'said "hi"'],
    ]);
  });

  it('handles a quoted field spanning several lines and CRLF', () => {
    expect(parseTable('a,b\r\n"one\r\ntwo",2\r\n')).toEqual([
      ['a', 'b'],
      ['one\r\ntwo', '2'],
    ]);
  });

  it('honours an explicit tab delimiter', () => {
    expect(parseTable('a\tb\n1\t2', '\t')).toEqual([
      ['a', 'b'],
      ['1', '2'],
    ]);
  });

  it('returns nothing for empty input', () => {
    expect(parseTable('')).toEqual([]);
  });
});

describe('tableDelimiter', () => {
  it('uses a tab for a TSV fence and sniffs the rest', () => {
    expect(tableDelimiter('tsv', 'a,b')).toBe('\t');
    expect(tableDelimiter('csv', 'a;b\n1;2')).toBe(';');
    expect(tableDelimiter('csv', 'a|b')).toBe('|');
    expect(tableDelimiter('csv', 'a,b')).toBe(',');
  });
});

describe('buildTable', () => {
  it('takes the first row as the header', () => {
    const spec = parseFence('csv', 'name,qty\nbolt,4')!;
    expect(buildTable(spec)).toEqual({
      header: ['name', 'qty'],
      rows: [['bolt', '4']],
      hiddenRows: 0,
      hiddenColumns: 0,
    });
  });

  it('caps rows and columns and reports what it hid', () => {
    const wide = Array.from({ length: MAX_TABLE_COLUMNS + 3 }, () => 'x').join(',');
    const lines = [wide];
    for (let index = 0; index < MAX_TABLE_ROWS + 5; index += 1) {
      lines.push(wide);
    }
    const spec = parseFence('csv', lines.join('\n'))!;
    const table = buildTable(spec);
    expect(table.rows).toHaveLength(MAX_TABLE_ROWS);
    expect(table.hiddenRows).toBe(5);
    expect(table.header).toHaveLength(MAX_TABLE_COLUMNS);
    expect(table.hiddenColumns).toBe(3);
    expect(MAX_TABLE_CHARS).toBeGreaterThan(0);
  });

  it('counts narrow rows iteratively just below the source-size cap', () => {
    const source = 'x\n'.repeat(127_000);
    expect(source).toHaveLength(254_000);
    expect(source.length).toBeLessThan(MAX_TABLE_CHARS);
    const spec = parseFence('artifact csv', source)!;

    const table = buildTable(spec);

    expect(table.header).toEqual(['x']);
    expect(table.rows).toHaveLength(MAX_TABLE_ROWS);
    expect(table.hiddenRows).toBe(126_799);
    expect(table.hiddenColumns).toBe(0);
  });
});

describe('describeArtifact', () => {
  it('names the title, kind and the isolation of a page', () => {
    expect(describeArtifact(parseFence('artifact html title="Signup"', HTML_DOC)!)).toBe(
      'Signup (HTML) artifact, rendered as an isolated page',
    );
    expect(describeArtifact(parseFence('mermaid', 'graph TD\n a-->b')!)).toBe('Mermaid artifact');
  });
});

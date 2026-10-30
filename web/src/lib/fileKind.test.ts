import { describe, expect, it } from 'vitest';
import {
  describeFile,
  fileExtension,
  fileName,
  hasRichPreview,
  hasSourceView,
  resolveRelativePath,
} from './fileKind';

describe('fileExtension', () => {
  it('reads the extension of the last path segment', () => {
    expect(fileExtension('docs/guide.md')).toBe('md');
    expect(fileExtension('logo.PNG')).toBe('png');
    expect(fileExtension('archive.tar.gz')).toBe('gz');
    expect(fileExtension('src/main.rs')).toBe('rs');
  });

  it('treats a dotfile as having no extension', () => {
    expect(fileExtension('.env')).toBe('');
    expect(fileExtension('config/.gitignore')).toBe('');
    expect(fileExtension('Makefile')).toBe('');
    expect(fileExtension('a/b/.eslintrc.json')).toBe('json');
  });

  it('does not read a dot in a directory as an extension', () => {
    expect(fileExtension('v1.2/README')).toBe('');
  });
});

describe('fileName', () => {
  it('returns the last segment', () => {
    expect(fileName('docs/guide.md')).toBe('guide.md');
    expect(fileName('README.md')).toBe('README.md');
  });
});

describe('describeFile', () => {
  it('renders markdown with a preview and allows editing it', () => {
    const capability = describeFile({ path: 'docs/guide.md', language: 'markdown', binary: false });
    expect(capability.render).toBe('markdown');
    expect(capability.preferPreview).toBe(true);
    expect(capability.canEdit).toBe(true);
    expect(capability.editBlockedReason).toBeNull();
  });

  it('renders html with a preview and allows editing it', () => {
    const capability = describeFile({ path: 'site/index.html', language: 'html', binary: false });
    expect(capability.render).toBe('html');
    expect(capability.preferPreview).toBe(true);
    expect(capability.canEdit).toBe(true);
  });

  it('renders images and pdfs from raw bytes but never edits them', () => {
    const image = describeFile({ path: 'assets/logo.png', language: null, binary: true });
    expect(image.render).toBe('image');
    expect(image.preferPreview).toBe(true);
    expect(image.canEdit).toBe(false);
    expect(image.editBlockedReason).toContain('no text to edit');

    const pdf = describeFile({ path: 'docs/spec.pdf', language: null, binary: true });
    expect(pdf.render).toBe('pdf');
    expect(pdf.canEdit).toBe(false);
  });

  it('renders a pdf the server called text, because a PDF is ASCII', () => {
    // `%PDF-1.4` contains no NUL byte, so the server's binary sniff reports
    // `binary: false`. The extension still decides how it is shown.
    const pdf = describeFile({ path: 'docs/spec.pdf', language: null, binary: false });
    expect(pdf.render).toBe('pdf');
    expect(pdf.canEdit).toBe(false);

    // The same applies to an SVG, which is XML text.
    const svg = describeFile({ path: 'assets/icon.svg', language: 'xml', binary: false });
    expect(svg.render).toBe('image');
    expect(svg.canEdit).toBe(false);
  });

  it('shows other code as source and allows editing it', () => {
    const capability = describeFile({ path: 'src/main.rs', language: 'rust', binary: false });
    expect(capability.render).toBe('code');
    expect(capability.preferPreview).toBe(false);
    expect(capability.canEdit).toBe(true);
  });

  it('refuses a binary file, even when the extension is editable', () => {
    const capability = describeFile({ path: 'weird.md', language: 'markdown', binary: true });
    expect(capability.render).toBe('binary');
    expect(capability.canEdit).toBe(false);
    expect(capability.editBlockedReason).toContain('binary file');
  });

  it('refuses to edit a truncated file, because saving it would lose the rest', () => {
    const capability = describeFile({
      path: 'huge.log',
      language: 'text',
      binary: false,
      truncated: true,
    });
    expect(capability.canEdit).toBe(false);
    expect(capability.editBlockedReason).toContain('limits.max_file_read_bytes');
  });

  it('falls back to the language when the extension is unknown', () => {
    expect(describeFile({ path: 'docs/NOTES', language: 'markdown' }).render).toBe('markdown');
    expect(describeFile({ path: 'page', language: 'html' }).render).toBe('html');
  });
});

describe('hasRichPreview', () => {
  it('is true only for a real rendered view', () => {
    expect(hasRichPreview('markdown')).toBe(true);
    expect(hasRichPreview('html')).toBe(true);
    expect(hasRichPreview('image')).toBe(true);
    expect(hasRichPreview('pdf')).toBe(true);
    expect(hasRichPreview('code')).toBe(false);
    expect(hasRichPreview('binary')).toBe(false);
  });
});

describe('hasSourceView', () => {
  it('is true only when there is text to show as source', () => {
    expect(hasSourceView('code')).toBe(true);
    expect(hasSourceView('markdown')).toBe(true);
    expect(hasSourceView('html')).toBe(true);
    // An image or PDF is its own preview: there is no source tab for it.
    expect(hasSourceView('image')).toBe(false);
    expect(hasSourceView('pdf')).toBe(false);
    expect(hasSourceView('binary')).toBe(false);
  });
});

describe('resolveRelativePath', () => {
  it('resolves against the file’s own directory', () => {
    expect(resolveRelativePath('docs/guide.md', 'logo.png')).toBe('docs/logo.png');
    expect(resolveRelativePath('docs/guide.md', './logo.png')).toBe('docs/logo.png');
    expect(resolveRelativePath('docs/guide.md', '../assets/logo.png')).toBe('assets/logo.png');
    expect(resolveRelativePath('README.md', 'docs/guide.md')).toBe('docs/guide.md');
  });

  it('treats a leading slash as workspace-relative', () => {
    expect(resolveRelativePath('docs/guide.md', '/assets/logo.png')).toBe('assets/logo.png');
  });

  it('drops query strings and fragments', () => {
    expect(resolveRelativePath('docs/guide.md', 'logo.png?raw=1')).toBe('docs/logo.png');
    expect(resolveRelativePath('docs/guide.md', 'other.md#section')).toBe('docs/other.md');
  });

  it('refuses to climb above the workspace root', () => {
    expect(resolveRelativePath('README.md', '../secrets.txt')).toBeNull();
    expect(resolveRelativePath('docs/guide.md', '../../etc/passwd')).toBeNull();
  });

  it('leaves absolute URLs and fragments alone', () => {
    expect(resolveRelativePath('docs/guide.md', 'https://example.com/x.png')).toBeNull();
    expect(resolveRelativePath('docs/guide.md', 'mailto:me@example.com')).toBeNull();
    expect(resolveRelativePath('docs/guide.md', '//cdn.example.com/x.png')).toBeNull();
    expect(resolveRelativePath('docs/guide.md', '#section')).toBeNull();
    expect(resolveRelativePath('docs/guide.md', 'data:image/png;base64,AAAA')).toBeNull();
    expect(resolveRelativePath('docs/guide.md', '   ')).toBeNull();
  });

  it('decodes percent-escapes and rejects NUL bytes', () => {
    expect(resolveRelativePath('docs/guide.md', 'my%20file.png')).toBe('docs/my file.png');
    expect(resolveRelativePath('docs/guide.md', 'bad%00.png')).toBeNull();
  });

  it('returns null for a target that resolves to nothing', () => {
    expect(resolveRelativePath('README.md', '.')).toBeNull();
    expect(resolveRelativePath('README.md', './')).toBeNull();
  });
});

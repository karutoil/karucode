import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { FileViewer } from './FileViewer';
import { errorResponse, jsonResponse, resetAppState } from '../test/fixtures';
import * as store from '../state/store';

interface MockFile {
  content: string;
  size: number;
  truncated?: boolean;
  binary?: boolean;
  language?: string | null;
  path?: string;
}

function installFetch(file: MockFile, onSave?: (body: unknown, url: string) => Response) {
  const read = {
    path: file.path ?? 'docs/guide.md',
    content: file.content,
    size: file.size,
    truncated: file.truncated ?? false,
    binary: file.binary ?? false,
    language: file.language ?? null,
  };
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    const method = (init?.method ?? 'GET').toUpperCase();
    if (method === 'PUT' && url.startsWith('/api/workspace/file')) {
      const body = init?.body ? JSON.parse(String(init.body)) : {};
      if (onSave) return onSave(body, url);
      return jsonResponse({
        path: read.path,
        size: new TextEncoder().encode(body.content ?? '').length,
        created: false,
        language: read.language,
      });
    }
    if (method === 'GET' && url.startsWith('/api/workspace/file')) return jsonResponse(read);
    return errorResponse('not_found', `unmocked ${method} ${url}`, 404);
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

describe('FileViewer', () => {
  beforeEach(() => {
    resetAppState();
    store.currentProjectId.value = null;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    resetAppState();
  });

  it('renders markdown as a preview with a source toggle and no raw HTML', async () => {
    installFetch({ content: '# Title\n\n<script>alert(1)</script>\n\nhello', size: 41, language: 'markdown' });
    store.fileViewerPath.value = 'docs/guide.md';
    const { container } = render(<FileViewer />);

    await waitFor(() => expect(screen.getByTestId('file-preview-markdown')).toBeTruthy());
    expect(screen.getByRole('heading', { name: 'Title' })).toBeTruthy();
    expect(screen.getByTestId('file-preview-markdown').textContent).toContain('<script>');
    expect(container.querySelector('script')).toBeNull();
    // Both view tabs exist, and the preview is the selected one.
    const previewTab = screen.getByRole('tab', { name: /preview/i });
    expect(previewTab.getAttribute('aria-selected')).toBe('true');

    fireEvent.click(screen.getByRole('tab', { name: /source/i }));
    await waitFor(() => expect(container.querySelector('.file-code')).not.toBeNull());
    expect(container.querySelector('.file-code')?.textContent).toContain('# Title');
  });

  it('renders html in a sandboxed iframe and never inlines it', async () => {
    installFetch({ content: '<h1>hi</h1><script>alert(1)</script>', size: 39, language: 'html', path: 'site/index.html' });
    store.fileViewerPath.value = 'site/index.html';
    const { container } = render(<FileViewer />);

    const frame = await screen.findByTestId('file-preview-html');
    // `sandbox=""` is the whole safety story: no scripts, no same-origin access.
    expect(frame.getAttribute('sandbox')).toBe('');
    expect(frame.getAttribute('srcdoc')).toContain('<h1>hi</h1>');
    expect(frame.getAttribute('srcdoc')).toContain('Content-Security-Policy');
    // The markup is inside the frame's attribute, not the app's DOM.
    expect(container.querySelector('h1')).toBeNull();
  });

  it('shows images from the raw endpoint rather than as binary', async () => {
    installFetch({ content: '', size: 1024, binary: true, path: 'assets/logo.png' });
    store.fileViewerPath.value = 'assets/logo.png';
    const { container } = render(<FileViewer />);

    const image = await screen.findByTestId('file-preview-image');
    expect(image.getAttribute('src')).toBe('/api/workspace/raw?path=assets%2Flogo.png');
    expect(image.getAttribute('alt')).toBe('assets/logo.png');
    expect(screen.queryByText(/binary file/i)).toBeNull();
    // No editor: there is no text.
    const edit = screen.getByRole('button', { name: /edit/i }) as HTMLButtonElement;
    expect(edit.disabled).toBe(true);
    expect(container.querySelector('textarea')).toBeNull();
    // An image *is* its preview, so there is no source view to switch to.
    expect(screen.queryByRole('tablist', { name: /file view/i })).toBeNull();
    expect(screen.queryByRole('tab', { name: /source/i })).toBeNull();
  });

  it('falls back to a refusal for a binary file with no preview', async () => {
    installFetch({ content: '', size: 512, binary: true, path: 'data/blob.bin' });
    store.fileViewerPath.value = 'data/blob.bin';
    const { container } = render(<FileViewer />);
    await waitFor(() => expect(container.querySelector('.file-notice')?.textContent).toContain('binary file'));
    const edit = screen.getByRole('button', { name: /edit/i }) as HTMLButtonElement;
    expect(edit.disabled).toBe(true);
    expect(edit.getAttribute('title')).toContain('no text to edit');
  });

  it('previews a pdf even though the server calls it text', async () => {
    // A real PDF is ASCII, so `binary` is false; the extension must still win.
    installFetch({
      content: '%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n',
      size: 48,
      binary: false,
      language: null,
      path: 'docs/spec.pdf',
    });
    store.fileViewerPath.value = 'docs/spec.pdf';
    const { container } = render(<FileViewer />);

    const frame = await screen.findByTestId('file-preview-pdf');
    expect(frame.getAttribute('src')).toBe('/api/workspace/raw?path=docs%2Fspec.pdf');
    // Not a highlighted dump of the PDF source, and not editable.
    expect(container.querySelector('.file-code')).toBeNull();
    expect(screen.queryByRole('tab', { name: /source/i })).toBeNull();
    const edit = screen.getByRole('button', { name: /edit/i }) as HTMLButtonElement;
    expect(edit.disabled).toBe(true);
    expect(container.querySelector('.file-statusbar')?.textContent).toContain('PDF');
  });

  it('edits and saves with the loaded size as the staleness check', async () => {
    const fetchMock = installFetch({ content: 'old', size: 3, language: 'markdown' });
    store.fileViewerPath.value = 'docs/guide.md';
    const { container } = render(<FileViewer />);

    await waitFor(() => expect(screen.getByTestId('file-preview-markdown')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: /edit/i }));

    const textarea = (await waitFor(() => {
      const element = container.querySelector('textarea');
      expect(element).not.toBeNull();
      return element as HTMLTextAreaElement;
    })) as HTMLTextAreaElement;
    expect(textarea.value).toBe('old');

    fireEvent.input(textarea, { target: { value: '# new' } });
    await waitFor(() => expect(textarea.value).toBe('# new'));
    fireEvent.click(screen.getByRole('button', { name: /save/i }));

    await waitFor(() => {
      const put = fetchMock.mock.calls.find(([, init]) => (init?.method ?? 'GET') === 'PUT');
      expect(put).toBeTruthy();
      const body = JSON.parse(String(put?.[1]?.body));
      expect(body).toEqual({ content: '# new', expected_size: 3 });
    });
    // Editing ends after a successful save, and the preview shows the new text.
    await waitFor(() => expect(container.querySelector('textarea')).toBeNull());
    await waitFor(() => expect(screen.getByText('new')).toBeTruthy());
  });

  it('offers an explicit overwrite when the file changed on disk', async () => {
    let attempts = 0;
    const fetchMock = installFetch(
      { content: 'old', size: 3, language: 'markdown' },
      (body, url) => {
        attempts += 1;
        if (!url.includes('force=true')) {
          return errorResponse(
            'bad_request',
            '`docs/guide.md` changed on disk (it was 3 bytes when you opened it, now 9); reload it or save with force=true to overwrite',
            409,
          );
        }
        return jsonResponse({
          path: 'docs/guide.md',
          size: new TextEncoder().encode(String((body as { content: string }).content)).length,
          created: false,
          language: 'markdown',
        });
      },
    );
    store.fileViewerPath.value = 'docs/guide.md';
    const { container } = render(<FileViewer />);

    await waitFor(() => expect(screen.getByTestId('file-preview-markdown')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: /edit/i }));
    const textarea = (await waitFor(() => container.querySelector('textarea'))) as HTMLTextAreaElement;
    fireEvent.input(textarea, { target: { value: 'mine' } });
    fireEvent.click(screen.getByRole('button', { name: /save/i }));

    const overwrite = await screen.findByRole('button', { name: /overwrite anyway/i });
    expect(screen.getByRole('alert').textContent).toContain('changed on disk');
    // The draft survives the refusal: nothing was lost.
    expect((container.querySelector('textarea') as HTMLTextAreaElement).value).toBe('mine');

    fireEvent.click(overwrite);
    await waitFor(() => expect(attempts).toBe(2));
    await waitFor(() => expect(container.querySelector('textarea')).toBeNull());
    const forced = fetchMock.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'PUT');
    expect(String(forced[forced.length - 1]?.[0])).toContain('force=true');
  });

  it('asks before discarding unsaved edits when the dialog is closed', async () => {
    installFetch({ content: 'old', size: 3, language: 'markdown' });
    store.fileViewerPath.value = 'docs/guide.md';
    const { container } = render(<FileViewer />);

    await waitFor(() => expect(screen.getByTestId('file-preview-markdown')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: /edit/i }));
    const textarea = (await waitFor(() => container.querySelector('textarea'))) as HTMLTextAreaElement;
    fireEvent.input(textarea, { target: { value: 'changed' } });

    fireEvent.click(screen.getByRole('button', { name: /close dialog/i }));
    // The viewer is still open, asking.
    expect(store.fileViewerPath.value).toBe('docs/guide.md');
    expect(await screen.findByText(/discard your unsaved changes/i)).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: /keep editing/i }));
    expect(store.fileViewerPath.value).toBe('docs/guide.md');
    expect(container.querySelector('textarea')).not.toBeNull();
  });

  it('refuses to edit a truncated file and says why', async () => {
    installFetch({ content: 'partial', size: 200_000, truncated: true, language: 'text', path: 'logs/huge.log' });
    store.fileViewerPath.value = 'logs/huge.log';
    render(<FileViewer />);

    await waitFor(() => expect(screen.getByText(/truncated by server/i)).toBeTruthy());
    const edit = screen.getByRole('button', { name: /edit/i }) as HTMLButtonElement;
    expect(edit.disabled).toBe(true);
    expect(edit.getAttribute('title')).toContain('limits.max_file_read_bytes');
  });
});

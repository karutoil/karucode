import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { FileViewer } from './FileViewer';
import { errorResponse, jsonResponse, resetAppState } from '../test/fixtures';
import * as store from '../state/store';

interface MockFile {
  content: string;
  size: number;
  digest?: string | null;
  truncated?: boolean;
  binary?: boolean;
  language?: string | null;
  path?: string;
}

const LOADED_DIGEST = `sha256:${'a'.repeat(64)}`;
const WRITTEN_DIGEST = `sha256:${'b'.repeat(64)}`;

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (cause: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((onResolve, onReject) => {
    resolve = onResolve;
    reject = onReject;
  });
  return { promise, resolve, reject };
}

function installFetch(
  file: MockFile,
  onSave?: (body: unknown, url: string) => Response | Promise<Response>,
) {
  const read = {
    path: file.path ?? 'docs/guide.md',
    content: file.content,
    size: file.size,
    digest: file.digest === null ? undefined : (file.digest ?? LOADED_DIGEST),
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
        digest: file.digest === null ? undefined : WRITTEN_DIGEST,
        created: false,
        language: read.language,
      });
    }
    if (method === 'GET' && url.startsWith('/api/workspace/file')) return jsonResponse(read);
    if (method === 'GET' && url.startsWith('/api/workspace/raw')) {
      return new Response(file.content || 'binary bytes', {
        status: 200,
        headers: { 'Content-Type': read.path.endsWith('.pdf') ? 'application/pdf' : 'image/png' },
      });
    }
    return errorResponse('not_found', `unmocked ${method} ${url}`, 404);
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

describe('FileViewer', () => {
  let createDescriptor: PropertyDescriptor | undefined;
  let revokeDescriptor: PropertyDescriptor | undefined;

  beforeEach(() => {
    resetAppState();
    store.currentProjectId.value = null;
    createDescriptor = Object.getOwnPropertyDescriptor(URL, 'createObjectURL');
    revokeDescriptor = Object.getOwnPropertyDescriptor(URL, 'revokeObjectURL');
    Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: () => 'blob:file-preview' });
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: vi.fn() });
  });

  afterEach(() => {
    // Tear down portals while the object-URL spies are still installed. The
    // shared teardown runs another idempotent cleanup later.
    cleanup();
    vi.unstubAllGlobals();
    resetAppState();
    for (const [name, descriptor] of [
      ['createObjectURL', createDescriptor],
      ['revokeObjectURL', revokeDescriptor],
    ] as const) {
      if (descriptor) Object.defineProperty(URL, name, descriptor);
      else delete (URL as unknown as Record<string, unknown>)[name];
    }
  });

  it('renders markdown as a preview with a source toggle and no raw HTML', async () => {
    installFetch({ content: '# Title\n\n<script>alert(1)</script>\n\nhello', size: 41, language: 'markdown' });
    store.fileViewerPath.value = 'docs/guide.md';
    render(<FileViewer />);
    // The viewer is a Dialog, and a Dialog renders through a portal into
    // `document.body` so no ancestor can clip it (see Dialog.tsx).
    const container = document.body;

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

  it('renders html as a real sandboxed page from its own URL, never inlined', async () => {
    installFetch({ content: '<h1>hi</h1><script>alert(1)</script>', size: 39, language: 'html', path: 'site/index.html' });
    store.fileViewerPath.value = 'site/index.html';
    render(<FileViewer />);
    // The viewer is a Dialog, and a Dialog renders through a portal into
    // `document.body` so no ancestor can clip it (see Dialog.tsx).
    const container = document.body;

    const frame = await screen.findByTestId('file-preview-html');
    // The page is fetched as a document from its own workspace path, so its
    // relative assets resolve and it gets the server's preview policy. A
    // `srcdoc` frame could do neither, and inherited the app's CSP on top.
    expect(frame.getAttribute('src')).toBe('/api/workspace/preview/site/index.html');
    expect(frame.getAttribute('srcdoc')).toBeNull();
    // `allow-scripts` makes the page work; `allow-same-origin` would give it the
    // app's origin, where it could read the stored bearer token and call `/api`.
    expect(frame.getAttribute('sandbox')).toBe('allow-scripts');
    // Nothing from the file is ever in the app's own DOM.
    expect(container.querySelector('h1')).toBeNull();
  });

  it('shows images from authenticated bytes rather than as binary', async () => {
    installFetch({ content: '', size: 1024, binary: true, path: 'assets/logo.png' });
    store.fileViewerPath.value = 'assets/logo.png';
    render(<FileViewer />);
    // The viewer is a Dialog, and a Dialog renders through a portal into
    // `document.body` so no ancestor can clip it (see Dialog.tsx).
    const container = document.body;

    const image = await screen.findByTestId('file-preview-image');
    expect(image.getAttribute('src')).toBe('blob:file-preview');
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
    render(<FileViewer />);
    // The viewer is a Dialog, and a Dialog renders through a portal into
    // `document.body` so no ancestor can clip it (see Dialog.tsx).
    const container = document.body;
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
    render(<FileViewer />);
    // The viewer is a Dialog, and a Dialog renders through a portal into
    // `document.body` so no ancestor can clip it (see Dialog.tsx).
    const container = document.body;

    const frame = await screen.findByTestId('file-preview-pdf');
    expect(frame.getAttribute('src')).toBe('blob:file-preview');
    // Not a highlighted dump of the PDF source, and not editable.
    expect(container.querySelector('.file-code')).toBeNull();
    expect(screen.queryByRole('tab', { name: /source/i })).toBeNull();
    const edit = screen.getByRole('button', { name: /edit/i }) as HTMLButtonElement;
    expect(edit.disabled).toBe(true);
    expect(container.querySelector('.file-statusbar')?.textContent).toContain('PDF');
  });

  it('sends the loaded digest for an equal-size rewrite, with size compatibility', async () => {
    const fetchMock = installFetch({ content: 'old', size: 3, language: 'markdown' });
    store.fileViewerPath.value = 'docs/guide.md';
    render(<FileViewer />);
    const container = document.body;

    await waitFor(() => expect(screen.getByTestId('file-preview-markdown')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: /edit/i }));
    const textarea = (await waitFor(() => container.querySelector('textarea'))) as HTMLTextAreaElement;
    expect(textarea.value).toBe('old');

    // Both values are three bytes: only the loaded digest can detect this rewrite.
    fireEvent.input(textarea, { target: { value: 'new' } });
    await waitFor(() => expect(textarea.value).toBe('new'));
    fireEvent.click(screen.getByRole('button', { name: /save/i }));

    await waitFor(() => {
      const put = fetchMock.mock.calls.find(([, init]) => (init?.method ?? 'GET') === 'PUT');
      expect(put).toBeTruthy();
      const body = JSON.parse(String(put?.[1]?.body));
      expect(body).toEqual({
        content: 'new',
        expected_digest: LOADED_DIGEST,
        expected_size: 3,
      });
    });
    await waitFor(() => expect(container.querySelector('textarea')).toBeNull());
    await waitFor(() => expect(screen.getByText('new')).toBeTruthy());

    // A second save must use the write response's fresh digest, not the GET's.
    fireEvent.click(screen.getByRole('button', { name: /edit/i }));
    const next = (await waitFor(() => container.querySelector('textarea'))) as HTMLTextAreaElement;
    fireEvent.input(next, { target: { value: 'two' } });
    fireEvent.click(screen.getByRole('button', { name: /save/i }));
    await waitFor(() => {
      const puts = fetchMock.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'PUT');
      expect(puts).toHaveLength(2);
      expect(JSON.parse(String(puts[1]?.[1]?.body)).expected_digest).toBe(WRITTEN_DIGEST);
    });
  });

  it('keeps newer keystrokes after a pending save and bases their successor on the saved digest', async () => {
    const writes: Array<{
      body: Record<string, unknown>;
      response: ReturnType<typeof deferred<Response>>;
    }> = [];
    installFetch({ content: 'old', size: 3, language: 'markdown' }, (body) => {
      const response = deferred<Response>();
      writes.push({ body: body as Record<string, unknown>, response });
      return response.promise;
    });
    store.fileViewerPath.value = 'docs/guide.md';
    render(<FileViewer />);
    await waitFor(() => expect(screen.getByTestId('file-preview-markdown')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: /edit/i }));
    const editor = (await waitFor(() => document.querySelector('textarea'))) as HTMLTextAreaElement;
    const saved = '# saved snapshot';
    const newer = `${saved}\n\nnewer unsaved keystrokes`;
    fireEvent.input(editor, { target: { value: saved } });
    fireEvent.click(screen.getByRole('button', { name: /^save$/i }));
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0]?.body).toEqual({
      content: saved,
      expected_digest: LOADED_DIGEST,
      expected_size: 3,
    });

    // The editor deliberately remains usable while the original PUT is pending.
    expect(editor.disabled).toBe(false);
    fireEvent.input(editor, { target: { value: newer } });
    await act(async () => {
      writes[0]?.response.resolve(jsonResponse({
        path: 'docs/guide.md',
        size: new TextEncoder().encode(saved).length,
        digest: WRITTEN_DIGEST,
        created: false,
        language: 'markdown',
      }));
      await writes[0]?.response.promise;
    });

    await waitFor(() => expect(screen.queryByRole('button', { name: /saving/i })).toBeNull());
    expect((document.querySelector('textarea') as HTMLTextAreaElement).value).toBe(newer);
    expect(screen.getByText('unsaved', { exact: true })).toBeTruthy();
    expect(document.querySelector('.file-size')?.textContent).toBe(
      `${new TextEncoder().encode(saved).length} B`,
    );
    expect(document.querySelector('.file-statusbar')?.textContent).toContain('editing');
    expect(document.querySelector('.file-statusbar')?.textContent).toContain('unsaved changes');
    expect(document.querySelector('.file-statusbar')?.textContent).not.toContain('Saved just now');
    expect(store.toasts.value.at(-1)?.message).toBe('Saved docs/guide.md; newer edits remain unsaved.');

    // The first response is the saved base, not a reason to discard or overwrite
    // the newer draft. Its digest is exactly what the second optimistic PUT needs.
    fireEvent.click(screen.getByRole('button', { name: /^save$/i }));
    await waitFor(() => expect(writes).toHaveLength(2));
    expect(writes[1]?.body).toEqual({
      content: newer,
      expected_digest: WRITTEN_DIGEST,
      expected_size: new TextEncoder().encode(saved).length,
    });
    await act(async () => {
      writes[1]?.response.resolve(jsonResponse({
        path: 'docs/guide.md',
        size: new TextEncoder().encode(newer).length,
        digest: `sha256:${'c'.repeat(64)}`,
        created: false,
        language: 'markdown',
      }));
      await writes[1]?.response.promise;
    });
    await waitFor(() => expect(document.querySelector('textarea')).toBeNull());
  });

  it('keeps size-only compatibility when an older server has no digest', async () => {
    const fetchMock = installFetch({ content: 'old', size: 3, digest: null, language: 'markdown' });
    store.fileViewerPath.value = 'docs/guide.md';
    render(<FileViewer />);
    await waitFor(() => expect(screen.getByTestId('file-preview-markdown')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: /edit/i }));
    const textarea = (await waitFor(() => document.querySelector('textarea'))) as HTMLTextAreaElement;
    fireEvent.input(textarea, { target: { value: 'new' } });
    fireEvent.click(screen.getByRole('button', { name: /save/i }));

    await waitFor(() => {
      const put = fetchMock.mock.calls.find(([, init]) => (init?.method ?? 'GET') === 'PUT');
      expect(JSON.parse(String(put?.[1]?.body))).toEqual({ content: 'new', expected_size: 3 });
    });
  });

  it.each(['success', 'conflict'] as const)(
    'ignores a delayed project A save %s after B opens and starts saving',
    async (outcome) => {
      const writes: {
        url: URL;
        body: Record<string, unknown>;
        response: ReturnType<typeof deferred<Response>>;
      }[] = [];
      const files = {
        p_a: { path: 'a.md', content: '# Alpha', digest: `sha256:${'1'.repeat(64)}` },
        p_b: { path: 'b.md', content: '# Beta', digest: `sha256:${'2'.repeat(64)}` },
      };
      vi.stubGlobal(
        'fetch',
        vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
          const url = new URL(String(input), 'http://localhost');
          const method = (init?.method ?? 'GET').toUpperCase();
          if (method === 'GET' && url.pathname === '/api/workspace/file') {
            const project = url.searchParams.get('project') === 'p_b' ? 'p_b' : 'p_a';
            const item = files[project];
            return Promise.resolve(
              jsonResponse({
                ...item,
                size: new TextEncoder().encode(item.content).length,
                truncated: false,
                binary: false,
                language: 'markdown',
              }),
            );
          }
          if (method === 'PUT' && url.pathname === '/api/workspace/file') {
            const response = deferred<Response>();
            writes.push({ url, body: JSON.parse(String(init?.body)), response });
            return response.promise;
          }
          return Promise.resolve(errorResponse('not_found', `unmocked ${method} ${url.pathname}`, 404));
        }),
      );
      store.currentProjectId.value = 'p_a';
      store.fileViewerPath.value = 'a.md';
      render(<FileViewer />);
      expect(await screen.findByRole('heading', { name: 'Alpha' })).toBeTruthy();
      fireEvent.click(screen.getByRole('button', { name: /edit/i }));
      const editorA = (await waitFor(() => document.querySelector('textarea'))) as HTMLTextAreaElement;
      fireEvent.input(editorA, { target: { value: '# A saved' } });
      fireEvent.click(screen.getByRole('button', { name: /save/i }));
      await waitFor(() => expect(writes).toHaveLength(1));
      expect(writes[0]?.url.searchParams.get('project')).toBe('p_a');
      expect(writes[0]?.url.searchParams.get('path')).toBe('a.md');
      expect(writes[0]?.body.expected_digest).toBe(files.p_a.digest);

      act(() => {
        store.currentProjectId.value = 'p_b';
        store.fileViewerPath.value = 'b.md';
      });
      expect(await screen.findByRole('heading', { name: 'Beta' })).toBeTruthy();
      fireEvent.click(screen.getByRole('button', { name: /edit/i }));
      const editorB = (await waitFor(() => document.querySelector('textarea'))) as HTMLTextAreaElement;
      fireEvent.input(editorB, { target: { value: '# B saved' } });
      fireEvent.click(screen.getByRole('button', { name: /save/i }));
      await waitFor(() => expect(writes).toHaveLength(2));
      expect(writes[1]?.body.expected_digest).toBe(files.p_b.digest);

      await act(async () => {
        writes[0]?.response.resolve(
          outcome === 'success'
            ? jsonResponse({
                path: 'a.md',
                size: 8,
                digest: `sha256:${'3'.repeat(64)}`,
                created: false,
                language: 'markdown',
              })
            : errorResponse('bad_request', 'A changed on disk', 409),
        );
        await writes[0]?.response.promise;
      });

      // A's success cannot replace B's draft or close its editor; its conflict
      // cannot become B's alert; its finally cannot clear B's pending save.
      expect((document.querySelector('textarea') as HTMLTextAreaElement).value).toBe('# B saved');
      expect(screen.getByRole('button', { name: /saving/i })).toBeTruthy();
      expect(screen.queryByRole('button', { name: /overwrite anyway/i })).toBeNull();
      expect(document.querySelector('.file-save-error')).toBeNull();
      expect(store.toasts.value.some((toast) => toast.message === 'Saved a.md')).toBe(false);

      await act(async () => {
        writes[1]?.response.resolve(
          jsonResponse({
            path: 'b.md',
            size: 8,
            digest: `sha256:${'4'.repeat(64)}`,
            created: false,
            language: 'markdown',
          }),
        );
        await writes[1]?.response.promise;
      });
      await waitFor(() => expect(document.querySelector('textarea')).toBeNull());
      expect(await screen.findByRole('heading', { name: 'B saved' })).toBeTruthy();
    },
  );

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
    render(<FileViewer />);
    // The viewer is a Dialog, and a Dialog renders through a portal into
    // `document.body` so no ancestor can clip it (see Dialog.tsx).
    const container = document.body;

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
    render(<FileViewer />);
    // The viewer is a Dialog, and a Dialog renders through a portal into
    // `document.body` so no ancestor can clip it (see Dialog.tsx).
    const container = document.body;

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

  it('offers no dead Preview tab for a plain code file, and keeps Source keyboard-operable', async () => {
    // `hasSourceView('code')` is true, which is why every `.rs`/`.ts`/`.toml` file
    // used to show a Preview tab that rendered nothing: clicking it left
    // `.file-code` as the pane with no preview element and no way to tell. A code
    // file has no preview renderer, so the strip should not offer one.
    installFetch({ content: 'fn main() {}\n', size: 14, language: 'rust', path: 'src/main.rs' });
    store.fileViewerPath.value = 'src/main.rs';
    render(<FileViewer />);
    // The viewer is a Dialog, and a Dialog renders through a portal into
    // `document.body` so no ancestor can clip it (see Dialog.tsx).
    const container = document.body;

    await waitFor(() => expect(container.querySelector('.file-code')).not.toBeNull());
    expect(screen.queryByRole('tab', { name: /preview/i })).toBeNull();
    // With nothing to switch between there is no tablist at all: the pane is the
    // file, and a one-item tablist is noise in the accessibility tree.
    expect(screen.queryByRole('tablist', { name: /file view/i })).toBeNull();
    expect(screen.queryByRole('tab', { name: /source/i })).toBeNull();
    // The status bar still says what is on screen.
    expect(container.querySelector('.file-statusbar')?.textContent).toContain('2 lines');
  });

  it('makes the file-view tabs keyboard-operable, with one tab-in-tablist', async () => {
    // The strip used to be two plain buttons with `role="tab"`: no `tabIndex={-1}`
    // on the inactive one, no arrow-key model, no ids for `aria-controls`. `Tabs`
    // implements all of it for the inspector and the workspace tabs, so the viewer
    // now shares it instead of reimplementing a worse copy.
    installFetch({ content: '# Title\n\nbody', size: 15, language: 'markdown' });
    store.fileViewerPath.value = 'docs/guide.md';
    render(<FileViewer />);
    // The viewer is a Dialog, and a Dialog renders through a portal into
    // `document.body` so no ancestor can clip it (see Dialog.tsx).
    const container = document.body;

    await waitFor(() => expect(screen.getByTestId('file-preview-markdown')).toBeTruthy());
    const preview = screen.getByRole('tab', { name: /preview/i });
    const source = screen.getByRole('tab', { name: /source/i });

    // Only the selected tab is in the tab order.
    expect(preview.getAttribute('tabindex')).toBe('0');
    expect(source.getAttribute('tabindex')).toBe('-1');
    // The selected tab names the panel and the panel names the tab back.
    expect(preview.getAttribute('aria-controls')).toBe('file-view-panel-preview');
    expect(preview.id).toBe('file-view-tab-preview');
    // The referenced panel exists, is a tabpanel, and points back at the tab: a
    // tab whose `aria-controls` names nothing is announced as controlling nothing.
    const previewPanel = document.getElementById('file-view-panel-preview');
    expect(previewPanel).not.toBeNull();
    expect(previewPanel?.getAttribute('role')).toBe('tabpanel');
    expect(previewPanel?.getAttribute('aria-labelledby')).toBe('file-view-tab-preview');

    // ArrowRight moves selection (and focus) to Source without a pointer.
    fireEvent.keyDown(preview, { key: 'ArrowRight' });
    await waitFor(() => expect(container.querySelector('.file-code')).not.toBeNull());
    expect(screen.getByRole('tab', { name: /source/i }).getAttribute('aria-selected')).toBe('true');
    expect(screen.getByRole('tab', { name: /source/i }).getAttribute('tabindex')).toBe('0');
    expect(document.activeElement).toBe(screen.getByRole('tab', { name: /source/i }));

    // Home goes back to the first tab, which wraps the other way round.
    fireEvent.keyDown(screen.getByRole('tab', { name: /source/i }), { key: 'Home' });
    await waitFor(() => expect(screen.getByTestId('file-preview-markdown')).toBeTruthy());
  });
});

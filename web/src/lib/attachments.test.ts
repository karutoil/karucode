import { describe, expect, it, vi } from 'vitest';
import { uploadAttachment } from '../api/endpoints';
import {
  attachmentSummary,
  canSendAttachments,
  classifyByName,
  classifyFile,
  composeMessage,
  extensionOf,
  INLINE_MAX_BYTES,
  looksBinary,
  newAttachmentId,
  readBytes,
  readText,
  removeAttachment,
  UPLOAD_MAX_BYTES,
  type Attachment,
} from './attachments';

function file(name: string, content: string | Uint8Array, type = 'text/plain'): File {
  const body = typeof content === 'string' ? [content] : [content as unknown as BlobPart];
  return new File(body, name, { type });
}

function attachment(overrides: Partial<Attachment> = {}): Attachment {
  return {
    id: 'a1',
    name: 'notes.md',
    size: 12,
    mode: 'inline',
    path: 'notes.md',
    status: 'ready',
    ...overrides,
  };
}

describe('extensionOf', () => {
  it('reads the last extension, case-insensitively', () => {
    expect(extensionOf('src/Main.TS')).toBe('ts');
    expect(extensionOf('archive.tar.gz')).toBe('gz');
    expect(extensionOf('Makefile')).toBe('');
    expect(extensionOf('Dockerfile')).toBe('dockerfile');
  });
});

describe('classifyByName', () => {
  it('inlines small text files', () => {
    expect(classifyByName('notes.md', 900).mode).toBe('inline');
    expect(classifyByName('src/main.ts', 2048, 'text/plain').mode).toBe('inline');
    expect(classifyByName('data.json', 100, 'application/json').mode).toBe('inline');
  });

  it('uploads binary files however small', () => {
    const image = classifyByName('shot.png', 900, 'image/png');
    expect(image.mode).toBe('upload');
    expect(image.reason).toBe('Binary file');
  });

  it('uploads text that is too big to inline', () => {
    const result = classifyByName('dump.log', INLINE_MAX_BYTES + 1);
    expect(result.mode).toBe('upload');
    expect(result.reason).toContain('Too large to inline');
  });

  it('uploads anything past the hard ceiling', () => {
    const result = classifyByName('video.mp4', UPLOAD_MAX_BYTES + 1, 'video/mp4');
    expect(result.mode).toBe('upload');
    expect(result.reason).toContain('Larger than');
  });

  it('treats an unknown extension without a MIME type as text', () => {
    expect(classifyByName('NOTES', 10, '').mode).toBe('inline');
  });
});

describe('looksBinary', () => {
  it('detects a NUL byte in the first chunk', () => {
    expect(looksBinary(new Uint8Array([104, 105, 0, 104]))).toBe(true);
    expect(looksBinary(new TextEncoder().encode('plain text'))).toBe(false);
  });
});

describe('classifyFile', () => {
  it('inlines a small text file', async () => {
    const decision = await classifyFile(file('notes.md', '# Title\n'));
    expect(decision.mode).toBe('inline');
  });

  it('uploads a text-looking file whose bytes are binary', async () => {
    const decision = await classifyFile(file('trap.txt', new Uint8Array([0, 1, 2, 3])));
    expect(decision.mode).toBe('upload');
    expect(decision.reason).toBe('Binary file');
  });

  it('uploads by name when the type is obviously binary', async () => {
    const decision = await classifyFile(file('shot.png', new Uint8Array([1, 2, 3]), 'image/png'));
    expect(decision.mode).toBe('upload');
  });
});

describe('read helpers', () => {
  it('reads text and byte ranges', async () => {
    const sample = file('a.txt', 'hello world');
    expect(await readText(sample)).toBe('hello world');
    expect(Array.from(await readBytes(sample, 0, 5))).toEqual([104, 101, 108, 108, 111]);
  });
});

describe('newAttachmentId', () => {
  it('is unique per file', () => {
    expect(newAttachmentId('a.ts')).not.toBe(newAttachmentId('a.ts'));
  });
});

describe('attachmentSummary', () => {
  it('describes every state', () => {
    expect(attachmentSummary(attachment({ status: 'reading' }))).toBe('Reading…');
    expect(attachmentSummary(attachment({ status: 'uploading', size: 2048 }))).toBe('Uploading · 2.0 KB…');
    expect(attachmentSummary(attachment({ status: 'error', error: 'nope' }))).toBe('nope');
    expect(attachmentSummary(attachment({ mode: 'inline', size: 120 }))).toBe('Inlined · 120 B');
    expect(attachmentSummary(attachment({ mode: 'upload', size: 4096 }))).toBe('Uploaded · 4.0 KB');
  });

  it('falls back to the failure text', () => {
    expect(attachmentSummary(attachment({ status: 'error' }))).toBe('Failed');
  });
});

describe('canSendAttachments', () => {
  it('is true only when every chip is ready', () => {
    expect(canSendAttachments([])).toBe(true);
    expect(canSendAttachments([attachment()])).toBe(true);
    expect(canSendAttachments([attachment({ status: 'uploading' })])).toBe(false);
    expect(canSendAttachments([attachment({ status: 'error' })])).toBe(false);
  });
});

describe('removeAttachment', () => {
  it('drops one chip and keeps the rest', () => {
    const first = attachment({ id: 'a' });
    const second = attachment({ id: 'b' });
    expect(removeAttachment([first, second], 'a')).toEqual([second]);
  });
});

describe('composeMessage', () => {
  it('returns the draft untouched without attachments', () => {
    expect(composeMessage('  keep me  ', [])).toBe('  keep me  ');
  });

  it('ignores attachments that are not ready', () => {
    expect(composeMessage('hi', [attachment({ status: 'uploading' })])).toBe('hi');
  });

  it('inlines a small text file with a language-tagged fence', () => {
    const message = composeMessage('Explain this', [
      attachment({ path: 'src/main.ts', content: 'const a = 1;\n', size: 13 }),
    ]);
    expect(message).toBe(
      ['Explain this', '', 'Attached file:', '', '### src/main.ts (inlined, 13 B)', '', '```ts', 'const a = 1;', '```'].join(
        '\n',
      ),
    );
  });

  it('references an uploaded file by workspace path', () => {
    const message = composeMessage('What is this?', [
      attachment({
        mode: 'upload',
        name: 'shot.png',
        path: '.karucode/attachments/s_1/shot.png',
        size: 4096,
      }),
    ]);
    expect(message).toContain('### .karucode/attachments/s_1/shot.png (uploaded, 4.0 KB)');
    expect(message).toContain('Read this file from the workspace when you need it:');
  });

  it('handles several attachments and an empty draft', () => {
    const message = composeMessage('', [
      attachment({ id: 'a', path: 'a.md', content: 'A' }),
      attachment({ id: 'b', mode: 'upload', path: 'b.png', size: 10 }),
    ]);
    expect(message.startsWith('Attached files:')).toBe(true);
    expect(message).toContain('### a.md (inlined, 12 B)');
    expect(message).toContain('### b.png (uploaded, 10 B)');
  });

  it('lengthens the fence when the content contains backticks', () => {
    const message = composeMessage('look', [
      attachment({ path: 'notes.md', content: '```js\nnested\n```\n' }),
    ]);
    expect(message).toContain('````md\n```js\nnested\n```\n````');
  });

  it('trims trailing blank lines off the draft', () => {
    const message = composeMessage('question\n\n  ', [attachment({ content: 'x' })]);
    expect(message.startsWith('question\n\nAttached file:')).toBe(true);
  });

  it('says nothing about the size when it is unknown', () => {
    const message = composeMessage('hi', [attachment({ size: 0 })]);
    expect(message).toContain('(inlined, unknown size)');
  });
});
describe('uploadAttachment helper', () => {
  // The locked contract guarantees only these three fields.
  const response = {
    path: '.karucode/attachments/s_1/shot-1.png',
    name: 'shot-1.png',
    size: 3,
  };

  function stubFetch() {
    return vi.fn(
      async () =>
        new Response(JSON.stringify(response), {
          status: 201,
          headers: { 'Content-Type': 'application/json' },
        }),
    );
  }

  it('posts the raw bytes with the documented query, headers and no JSON wrapper', async () => {
    const fetchImpl = stubFetch();
    const result = await uploadAttachment(
      { name: 'shot.png', data: new Uint8Array([1, 2, 3]) },
      { session: 's_1', project: 'p_1' },
      { fetchImpl: fetchImpl as unknown as typeof fetch },
    );

    // The response, not the local file name, is what callers reference.
    expect(result.path).toBe('.karucode/attachments/s_1/shot-1.png');
    expect(result.name).toBe('shot-1.png');
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/api/workspace/upload?session=s_1&name=shot.png&project=p_1');
    expect(init.method).toBe('POST');
    const headers = init.headers as Record<string, string>;
    // The contract fixes the type, so no per-file MIME sniffing on the wire.
    expect(headers['Content-Type']).toBe('application/octet-stream');
    // CSRF header still comes from the shared request path.
    expect(headers['X-Karucode-Client']).toBe('web');
    expect(headers.Accept).toBe('application/json');
    expect(init.body).toBeInstanceOf(Blob);
    expect((init.body as Blob).size).toBe(3);
  });

  it('omits project when the session decides it, and accepts an explicit contentType', async () => {
    const fetchImpl = stubFetch();
    await uploadAttachment(
      { name: 'shot.png', data: new Uint8Array([9]), contentType: 'image/png' },
      { session: 's_1' },
      { fetchImpl: fetchImpl as unknown as typeof fetch },
    );
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/api/workspace/upload?session=s_1&name=shot.png');
    expect((init.headers as Record<string, string>)['Content-Type']).toBe('image/png');
  });

  it('sends exactly the bytes it was given', async () => {
    const fetchImpl = stubFetch();
    const bytes = new Uint8Array([137, 80, 78, 71, 0, 255]);
    await uploadAttachment({ name: 'x.png', data: bytes }, { session: 's_1' }, {
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    const [, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    const body = init.body as Blob;
    // jsdom's Blob has no arrayBuffer() in this environment; FileReader does.
    const sent = new Uint8Array(
      await new Promise<ArrayBuffer>((resolve, reject) => {
        const reader = new FileReader();
        reader.onerror = () => reject(reader.error ?? new Error('read failed'));
        reader.onload = () => resolve(reader.result as ArrayBuffer);
        reader.readAsArrayBuffer(body);
      }),
    );
    expect(body.size).toBe(bytes.length);
    expect(Array.from(sent)).toEqual(Array.from(bytes));
  });

  it('turns a server rejection into an ApiError the chip can display', async () => {
    const fetchImpl = vi.fn(
      async () =>
        new Response(JSON.stringify({ error: { code: 'bad_request', message: 'request body is empty' } }), {
          status: 400,
        }),
    );
    await expect(
      uploadAttachment(
        { name: 'empty.txt', data: new Uint8Array([]) },
        { session: 's_1' },
        { fetchImpl: fetchImpl as unknown as typeof fetch },
      ),
    ).rejects.toThrow('request body is empty');
  });

  it('surfaces the oversize rejection from the server', async () => {
    const fetchImpl = vi.fn(
      async () =>
        new Response(
          JSON.stringify({ error: { code: 'payload_too_large', message: 'body is larger than 8 MiB' } }),
          { status: 413 },
        ),
    );
    await expect(
      uploadAttachment({ name: 'big.bin', data: new Uint8Array([1]) }, { session: 's_1' }, {
        fetchImpl: fetchImpl as unknown as typeof fetch,
      }),
    ).rejects.toThrow('body is larger than 8 MiB');
  });
});

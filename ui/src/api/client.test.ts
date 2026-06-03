import {
  ApiPreconditionError,
  createDocument,
  getDocument,
  isTextContentType,
  putDocument,
} from './client';

describe('Quarry API client', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('captures ETags from document reads', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        new Response('body', { headers: { ETag: '"v1"', 'x-quarry-document-id': 'doc-1' } })
      )
    );

    await expect(getDocument('notes', 'a.md')).resolves.toMatchObject({
      content: 'body',
      documentId: 'doc-1',
      etag: '"v1"',
      path: 'a.md',
    });
  });

  it('uses If-Match for existing document saves', async () => {
    const fetch = vi.fn(async () =>
      new Response(JSON.stringify({ version: { id: 'v2' } }), {
        headers: { ETag: '"v2"', 'content-type': 'application/json' },
      })
    );
    vi.stubGlobal('fetch', fetch);

    await putDocument('notes', 'a.md', 'next', '"v1"', 'text/markdown');

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/a.md',
      expect.objectContaining({
        method: 'PUT',
        headers: expect.objectContaining({ 'If-Match': '"v1"' }),
      })
    );
  });

  it('uses If-None-Match for creates', async () => {
    const fetch = vi.fn(async () =>
      new Response(JSON.stringify({ version: { id: 'v1' } }), {
        headers: { ETag: '"v1"', 'content-type': 'application/json' },
      })
    );
    vi.stubGlobal('fetch', fetch);

    await createDocument('notes', 'new.md', '# New');

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/new.md',
      expect.objectContaining({
        method: 'PUT',
        headers: expect.objectContaining({ 'If-None-Match': '*' }),
      })
    );
  });

  it('raises a typed stale-save error on 412 responses', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        new Response(JSON.stringify({ error: 'head changed' }), {
          status: 412,
          headers: { 'content-type': 'application/json' },
        })
      )
    );

    await expect(putDocument('notes', 'a.md', 'next', '"old"')).rejects.toBeInstanceOf(
      ApiPreconditionError
    );
  });

  it('does not classify XML-based image formats as editable text', () => {
    expect(isTextContentType('image/svg+xml')).toBe(false);
  });
});

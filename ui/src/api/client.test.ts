import {
  ApiPreconditionError,
  createCollabInvite,
  createDocument,
  deleteDocument,
  getDocument,
  isTextContentType,
  moveDocument,
  listAgentPresence,
  putDocument,
  restoreVersion,
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

  it('stamps existing document saves with the mutation origin id', async () => {
    const fetch = vi.fn(async () =>
      new Response(JSON.stringify({ version: { id: 'v2' } }), {
        headers: { ETag: '"v2"', 'content-type': 'application/json' },
      })
    );
    vi.stubGlobal('fetch', fetch);

    await putDocument('notes', 'a.md', 'next', '"v1"', 'text/markdown', {
      originId: 'browser:session-1',
    });

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/a.md',
      expect.objectContaining({
        headers: expect.objectContaining({ 'X-Quarry-Origin-Id': 'browser:session-1' }),
      })
    );
  });

  it('mints document-scoped collab invite tokens', async () => {
    const fetch = vi.fn(async () =>
      new Response(
        JSON.stringify({
          id: 'invite-1',
          document_id: 'doc-1',
          role: 'editor',
          by_hint: 'Avery',
          created_at: 'now',
          revoked_at: null,
        }),
        { headers: { 'content-type': 'application/json' } }
      )
    );
    vi.stubGlobal('fetch', fetch);

    await expect(
      createCollabInvite('notes', 'folder/live.md', { byHint: 'Avery' })
    ).resolves.toMatchObject({
      id: 'invite-1',
      role: 'editor',
    });

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/folder/live.md/share',
      expect.objectContaining({
        body: JSON.stringify({ byHint: 'Avery', role: 'editor' }),
        method: 'POST',
      })
    );
  });

  it('lists agent presence for a document', async () => {
    const fetch = vi.fn(async () =>
      new Response(
        JSON.stringify({
          presence: [
            {
              library: 'notes',
              path: 'folder/live.md',
              documentId: 'doc-1',
              agentId: 'ai:codex:abc',
              status: 'waiting',
              by: 'Codex',
              updatedAt: 'now',
            },
          ],
        }),
        { headers: { 'content-type': 'application/json' } }
      )
    );
    vi.stubGlobal('fetch', fetch);

    await expect(listAgentPresence('notes', 'folder/live.md')).resolves.toMatchObject({
      presence: [{ agentId: 'ai:codex:abc', status: 'waiting' }],
    });

    expect(fetch).toHaveBeenCalledWith('/v1/libraries/notes/documents/folder/live.md/presence', undefined);
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

  it('stamps document creates with the mutation origin id when provided', async () => {
    const fetch = vi.fn(async () =>
      new Response(JSON.stringify({ version: { id: 'v1' } }), {
        headers: { ETag: '"v1"', 'content-type': 'application/json' },
      })
    );
    vi.stubGlobal('fetch', fetch);

    await createDocument('notes', 'new.md', '# New', 'text/markdown', {
      originId: 'browser:session-1',
    });

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/new.md',
      expect.objectContaining({
        method: 'PUT',
        headers: expect.objectContaining({ 'X-Quarry-Origin-Id': 'browser:session-1' }),
      })
    );
  });

  it('stamps document deletes with the mutation origin id when provided', async () => {
    const fetch = vi.fn(async () => new Response(JSON.stringify({ id: 'tx-1' }), {
      headers: { 'content-type': 'application/json' },
    }));
    vi.stubGlobal('fetch', fetch);

    await deleteDocument('notes', 'old.md', { originId: 'browser:session-1' });

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/old.md',
      expect.objectContaining({
        method: 'DELETE',
        headers: expect.objectContaining({ 'X-Quarry-Origin-Id': 'browser:session-1' }),
      })
    );
  });

  it('stamps document moves with the mutation origin id when provided', async () => {
    const fetch = vi.fn(async () => new Response(JSON.stringify({ id: 'tx-1' }), {
      headers: { 'content-type': 'application/json' },
    }));
    vi.stubGlobal('fetch', fetch);

    await moveDocument('notes', 'old.md', 'new.md', { originId: 'browser:session-1' });

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/old.md/move',
      expect.objectContaining({
        method: 'POST',
        headers: expect.objectContaining({ 'X-Quarry-Origin-Id': 'browser:session-1' }),
      })
    );
  });

  it('stamps version restores with the mutation origin id when provided', async () => {
    const fetch = vi.fn(async () =>
      new Response(JSON.stringify({ version: { id: 'v2' } }), {
        headers: { ETag: '"v2"', 'content-type': 'application/json' },
      })
    );
    vi.stubGlobal('fetch', fetch);

    await restoreVersion('notes', 'daily.md', 'v1', { originId: 'browser:session-1' });

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/daily.md/versions/v1/restore',
      expect.objectContaining({
        method: 'POST',
        headers: expect.objectContaining({ 'X-Quarry-Origin-Id': 'browser:session-1' }),
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

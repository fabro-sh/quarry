import type { BlockTransactionRequest } from './generated/types';
import {
  ApiError,
  ApiPreconditionError,
  BlockTransactionError,
  createCollabInvite,
  createDocument,
  createTmpDocument,
  deleteDocument,
  deleteTmpDocument,
  getDocument,
  getDocumentBlocks,
  getTmpDocument,
  isTextContentType,
  moveDocument,
  listAgentPresence,
  postBlockTransaction,
  putDocument,
  putTmpDocument,
  restoreVersion,
  setDocumentTtl,
  promoteTmpDocument,
  setTmpDocumentTtl,
  tmpDocumentVersion,
  tmpVersions,
} from './client';

describe('Quarry API client', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('captures ETags from document reads', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        new Response('body', {
          headers: {
            ETag: '"v1"',
            'x-quarry-document-id': 'doc-1',
            'x-quarry-expires-at': '2099-01-01T00:00:00Z',
          },
        })
      )
    );

    await expect(getDocument('notes', 'a.md')).resolves.toMatchObject({
      content: 'body',
      documentId: 'doc-1',
      etag: '"v1"',
      expiresAt: '2099-01-01T00:00:00Z',
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

  it('stamps existing document saves with transaction metadata', async () => {
    const fetch = vi.fn(async () =>
      new Response(JSON.stringify({ version: { id: 'v2' } }), {
        headers: { ETag: '"v2"', 'content-type': 'application/json' },
      })
    );
    vi.stubGlobal('fetch', fetch);

    await putDocument('notes', 'a.md', 'next', '"v1"', 'text/markdown', {
      transactionActor: 'browser',
      transactionMessage: 'Autosaved edits',
      transactionProvenance: {
        history: { kind: 'autosave', reason: 'typing', session_id: 'browser:session-1' },
      },
    });

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/a.md',
      expect.objectContaining({
        headers: expect.objectContaining({
          'X-Quarry-Transaction-Actor': 'browser',
          'X-Quarry-Transaction-Message': 'Autosaved edits',
          'X-Quarry-Transaction-Provenance': JSON.stringify({
            history: { kind: 'autosave', reason: 'typing', session_id: 'browser:session-1' },
          }),
        }),
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

  it('creates tmp documents through the tmp collection route', async () => {
    const fetch = vi.fn(async () =>
      new Response(JSON.stringify({ version: { id: 'v1' } }), {
        status: 201,
        headers: { ETag: '"v1"', 'content-type': 'application/json' },
      })
    );
    vi.stubGlobal('fetch', fetch);

    await createTmpDocument({ path: 'scratch/new.md', content: '# New', contentType: 'text/markdown' });

    expect(fetch).toHaveBeenCalledWith(
      '/v1/tmp/documents',
      expect.objectContaining({
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          path: 'scratch/new.md',
          content: '# New',
          content_type: 'text/markdown',
          metadata: undefined,
          expires_at: undefined,
        }),
      })
    );
  });

  it('reads and saves tmp documents with tmp URLs and If-Match', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response('tmp body', {
          headers: {
            ETag: '"v1"',
            'content-type': 'text/plain',
            'x-quarry-document-id': 'tmp-1',
            'x-quarry-expires-at': '2099-01-01T00:00:00Z',
          },
        })
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ version: { id: 'v2' } }), {
          headers: { ETag: '"v2"', 'content-type': 'application/json' },
        })
      );
    vi.stubGlobal('fetch', fetch);

    await expect(getTmpDocument('scratch/note.txt')).resolves.toMatchObject({
      content: 'tmp body',
      documentId: 'tmp-1',
      etag: '"v1"',
      expiresAt: '2099-01-01T00:00:00Z',
    });
    await putTmpDocument('scratch/note.txt', 'next', '"v1"', 'text/plain');

    expect(fetch).toHaveBeenNthCalledWith(1, '/v1/tmp/documents/scratch/note.txt');
    expect(fetch).toHaveBeenNthCalledWith(
      2,
      '/v1/tmp/documents/scratch/note.txt',
      expect.objectContaining({
        method: 'PUT',
        headers: expect.objectContaining({ 'If-Match': '"v1"', 'content-type': 'text/plain' }),
        body: 'next',
      })
    );
  });

  it('exposes tmp versions ttl delete and promote helpers', async () => {
    const fetch = vi.fn(async () =>
      new Response(JSON.stringify({ ok: true }), {
        headers: { 'content-type': 'application/json' },
      })
    );
    vi.stubGlobal('fetch', fetch);

    await tmpVersions('scratch/note.txt');
    await tmpDocumentVersion('scratch/note.txt', 'v1');
    await setTmpDocumentTtl('scratch/note.txt', '2099-01-01T00:00:00Z');
    await promoteTmpDocument('scratch/note.txt', {
      library: 'notes',
      path: 'promoted/note.txt',
      ifMatch: 'v2',
    });
    await deleteTmpDocument('scratch/note.txt');

    expect(fetch).toHaveBeenNthCalledWith(
      1,
      '/v1/tmp/documents/scratch/note.txt/versions',
      undefined
    );
    expect(fetch).toHaveBeenNthCalledWith(
      2,
      '/v1/tmp/documents/scratch/note.txt/versions/v1',
      undefined
    );
    expect(fetch).toHaveBeenNthCalledWith(
      3,
      '/v1/tmp/documents/scratch/note.txt/ttl',
      expect.objectContaining({
        method: 'PATCH',
        body: JSON.stringify({ expires_at: '2099-01-01T00:00:00Z' }),
      })
    );
    expect(fetch).toHaveBeenNthCalledWith(
      4,
      '/v1/tmp/documents/scratch/note.txt/promote',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          library: 'notes',
          path: 'promoted/note.txt',
          if_match: 'v2',
        }),
      })
    );
    expect(fetch).toHaveBeenNthCalledWith(
      5,
      '/v1/tmp/documents/scratch/note.txt',
      expect.objectContaining({ method: 'DELETE' })
    );
  });

  it('sets and clears library document TTLs', async () => {
    const fetch = vi.fn(async () =>
      new Response(JSON.stringify({ expires_at: null }), {
        headers: { 'content-type': 'application/json' },
      })
    );
    vi.stubGlobal('fetch', fetch);

    await setDocumentTtl('notes', 'folder/live.md', '2099-01-01T00:00:00Z');
    await setDocumentTtl('notes', 'folder/live.md', null);

    expect(fetch).toHaveBeenNthCalledWith(
      1,
      '/v1/libraries/notes/documents/folder/live.md/ttl',
      expect.objectContaining({
        method: 'PATCH',
        body: JSON.stringify({ expires_at: '2099-01-01T00:00:00Z' }),
      })
    );
    expect(fetch).toHaveBeenNthCalledWith(
      2,
      '/v1/libraries/notes/documents/folder/live.md/ttl',
      expect.objectContaining({
        method: 'PATCH',
        body: JSON.stringify({ expires_at: null }),
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

  it('reads canonical block trees from the blocks route', async () => {
    const fetch = vi.fn(async () =>
      new Response(
        JSON.stringify({
          document_id: 'doc-1',
          document_clock: 'v2',
          blocks: [
            {
              block_id: 'b1',
              parent_block_id: null,
              position: 0,
              block_type: 'p',
              attrs: {},
              text: 'Hello',
              marks: [],
              links: [],
            },
          ],
        }),
        { headers: { 'content-type': 'application/json' } }
      )
    );
    vi.stubGlobal('fetch', fetch);

    await expect(getDocumentBlocks('notes', 'folder/doc.md')).resolves.toMatchObject({
      document_clock: 'v2',
      blocks: [{ block_id: 'b1', text: 'Hello' }],
    });

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/folder/doc.md/blocks',
      undefined
    );
  });

  it('posts block transactions and returns the ack', async () => {
    const fetch = vi.fn(async () =>
      new Response(
        JSON.stringify({
          status: 'committed',
          document_clock: 'v3',
          transaction_id: 'btx-1',
          changed_block_ids: ['b1'],
        }),
        { headers: { 'content-type': 'application/json' } }
      )
    );
    vi.stubGlobal('fetch', fetch);

    const request: BlockTransactionRequest = {
      client_tx_id: 'tx-1',
      base_clock: 'v2',
      actor: { kind: 'agent', id: 'agent-1' },
      ops: [{ op: 'replace_block_content', block_id: 'b1', text: 'Updated' }],
    };
    await expect(postBlockTransaction('notes', 'doc.md', request)).resolves.toMatchObject({
      status: 'committed',
      document_clock: 'v3',
      changed_block_ids: ['b1'],
    });

    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/notes/documents/doc.md/transactions',
      expect.objectContaining({
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(request),
      })
    );
  });

  it('raises typed block transaction errors with code and retryability', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        new Response(
          JSON.stringify({
            code: 'STALE_BASE',
            retryable: true,
            message: 'base_clock does not name a known version',
          }),
          { status: 412, headers: { 'content-type': 'application/json' } }
        )
      )
    );

    const failure = await postBlockTransaction('notes', 'doc.md', {
      client_tx_id: 'tx-1',
      actor: { kind: 'agent' },
      ops: [{ op: 'delete_block', block_id: 'b1' }],
    }).then(
      () => {
        throw new Error('expected a typed failure');
      },
      (error: unknown) => error
    );
    expect(failure).toBeInstanceOf(BlockTransactionError);
    if (failure instanceof BlockTransactionError) {
      expect(failure.code).toBe('STALE_BASE');
      expect(failure.retryable).toBe(true);
      expect(failure.status).toBe(412);
    }
  });

  it('falls back to the generic error mapping for untyped transaction failures', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        new Response(JSON.stringify({ error: 'not found: doc.md' }), {
          status: 404,
          headers: { 'content-type': 'application/json' },
        })
      )
    );

    const failure = await postBlockTransaction('notes', 'doc.md', {
      client_tx_id: 'tx-1',
      actor: { kind: 'agent' },
      ops: [{ op: 'delete_block', block_id: 'b1' }],
    }).then(
      () => {
        throw new Error('expected a generic failure');
      },
      (error: unknown) => error
    );
    expect(failure).toBeInstanceOf(ApiError);
    expect(failure).not.toBeInstanceOf(BlockTransactionError);
    if (failure instanceof ApiError) {
      expect(failure.message).toBe('not found: doc.md');
      expect(failure.status).toBe(404);
    }
  });
});

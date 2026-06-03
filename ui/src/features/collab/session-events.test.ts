import { classifyLiveDocumentEvent, type LiveCollabSession } from './session-events';

describe('collaboration session event classification', () => {
  const session: LiveCollabSession = {
    documentId: 'doc-1',
    path: 'notes/daily.md',
    sessionId: 'session-1',
    ackedFlushVersionIds: new Set(['v2']),
    ackedFlushEtags: new Set(['"v2"']),
  };

  it('passes unrelated document events through', () => {
    expect(
      classifyLiveDocumentEvent(
        { type: 'doc.changed', path: 'notes/other.md', doc_id: 'doc-2' },
        session
      )
    ).toEqual({ action: 'pass' });
  });

  it('does not use path fallback when the event carries a different document id', () => {
    expect(
      classifyLiveDocumentEvent(
        { type: 'doc.changed', path: 'notes/daily.md', doc_id: 'doc-2' },
        session
      )
    ).toEqual({ action: 'pass' });
  });

  it('ignores the live session own flush echo by session id', () => {
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.changed',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          collab_session_id: 'session-1',
        },
        session
      )
    ).toEqual({ action: 'ignore_flush_echo' });
  });

  it('ignores the live session own flush echo by acked version metadata', () => {
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.changed',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          version_id: 'v2',
        },
        session
      )
    ).toEqual({ action: 'ignore_flush_echo' });
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.changed',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          etag: '"v2"',
        },
        session
      )
    ).toEqual({ action: 'ignore_flush_echo' });
  });

  it('surfaces external writes without treating them as safe reloads', () => {
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.changed',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          version_id: 'v3',
        },
        session
      )
    ).toEqual({ action: 'external_change' });
  });

  it('keeps an externally deleted live document selected for discard or resurrection', () => {
    expect(
      classifyLiveDocumentEvent(
        { type: 'doc.deleted', path: 'notes/daily.md', doc_id: 'doc-1' },
        session
      )
    ).toEqual({ action: 'external_delete' });
  });

  it('retargets live sessions on document moves', () => {
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.moved',
          from: 'notes/daily.md',
          to: 'notes/renamed.md',
          doc_id: 'doc-1',
        },
        session
      )
    ).toEqual({ action: 'retarget_move', path: 'notes/renamed.md' });
  });
});

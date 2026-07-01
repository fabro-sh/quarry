import { classifyLiveDocumentEvent, type LiveCollabSession } from './session-events';

describe('collaboration session event classification', () => {
  const session: LiveCollabSession = {
    documentId: 'doc-1',
    path: 'notes/daily.md',
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

  it('passes everything through without a live session', () => {
    expect(
      classifyLiveDocumentEvent({ type: 'doc.changed', path: 'notes/daily.md' }, null)
    ).toEqual({ action: 'pass' });
  });

  it('classifies every change to the session document as a benign refresh', () => {
    // Checkpoints of the browser's own typing, agent transactions, and
    // whole-file writes all reach the editor through the live session; the
    // SSE event only refreshes metadata caches. Origin is irrelevant.
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.changed',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          origin_id: 'agent-injected:tx-1',
        },
        session
      )
    ).toEqual({ action: 'session_refresh' });
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.changed',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          origin_id: 'git:peer-1',
          version_id: 'v9',
        },
        session
      )
    ).toEqual({ action: 'session_refresh' });
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.changed',
          doc_id: 'doc-1',
          version_id: 'v10',
        },
        session
      )
    ).toEqual({ action: 'session_refresh' });
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

  it('leaves deletes to the generic handling (no draft to protect)', () => {
    expect(
      classifyLiveDocumentEvent(
        { type: 'doc.deleted', path: 'notes/daily.md', doc_id: 'doc-1' },
        session
      )
    ).toEqual({ action: 'pass' });
  });
});

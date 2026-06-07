import {
  classifyLiveDocumentEvent,
  isAdoptedFlushVersion,
  parseInjectionEnvelope,
  type LiveCollabSession,
} from './session-events';

describe('collaboration session event classification', () => {
  const session: LiveCollabSession = {
    documentId: 'doc-1',
    path: 'notes/daily.md',
    sessionId: 'browser:session-1',
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

  it('ignores the live session own change echo by origin id', () => {
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.changed',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          origin_id: 'browser:session-1',
        },
        session
      )
    ).toEqual({ action: 'ignore_own_mutation_echo' });
  });

  it('treats a same-document browser peer change as external', () => {
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.changed',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          origin_id: 'browser:peer',
        },
        session
      )
    ).toEqual({ action: 'external_change' });
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
    ).toEqual({ action: 'ignore_own_mutation_echo' });
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
    ).toEqual({ action: 'ignore_own_mutation_echo' });
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

  it('classifies server-injected agent writes as refresh-only wake signals', () => {
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.changed',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          version_id: 'v3',
          etag: '"v3"',
          origin_id: 'agent-injected:abc',
        },
        session
      )
    ).toEqual({ action: 'agent_injection_refresh' });
  });

  it('keeps an externally deleted live document selected for discard or resurrection', () => {
    expect(
      classifyLiveDocumentEvent(
        { type: 'doc.deleted', path: 'notes/daily.md', doc_id: 'doc-1' },
        session
      )
    ).toEqual({ action: 'external_delete' });
  });

  it('ignores the live session own delete echo by origin id', () => {
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.deleted',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          origin_id: 'browser:session-1',
        },
        session
      )
    ).toEqual({ action: 'ignore_own_mutation_echo' });
  });

  it('treats a same-document browser peer delete as external', () => {
    expect(
      classifyLiveDocumentEvent(
        {
          type: 'doc.deleted',
          path: 'notes/daily.md',
          doc_id: 'doc-1',
          origin_id: 'browser:peer',
        },
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

describe('parseInjectionEnvelope', () => {
  it('parses only the version identifiers and ignores legacy review payloads', () => {
    expect(
      parseInjectionEnvelope({
        etag: '"v3"',
        review: '{"not":"used"}',
        version_id: 'v3',
      })
    ).toEqual({ etag: '"v3"', versionId: 'v3' });
  });

  it('accepts an envelope without review metadata', () => {
    expect(parseInjectionEnvelope({ etag: '"v3"', version_id: 'v3' })).toEqual({
      etag: '"v3"',
      versionId: 'v3',
    });
  });

  it('rejects malformed envelopes instead of throwing', () => {
    expect(parseInjectionEnvelope(null)).toBeNull();
    expect(parseInjectionEnvelope({ etag: '"v3"' })).toBeNull();
    expect(parseInjectionEnvelope({ etag: '"v3"', version_id: 3 })).toBeNull();
  });
});

describe('isAdoptedFlushVersion', () => {
  const session: LiveCollabSession = {
    documentId: 'doc-1',
    path: 'notes/daily.md',
    sessionId: 'browser:session-1',
    ackedFlushVersionIds: new Set(['v2']),
    ackedFlushEtags: new Set(['"v2"']),
  };

  it('recognizes an adopted version by version id', () => {
    expect(isAdoptedFlushVersion(session, { versionId: 'v2' })).toBe(true);
  });

  it('recognizes an adopted version by etag', () => {
    expect(isAdoptedFlushVersion(session, { etag: '"v2"' })).toBe(true);
  });

  it('does not recognize an unrelated (genuinely external) version', () => {
    expect(isAdoptedFlushVersion(session, { versionId: 'v3', etag: '"v3"' })).toBe(false);
  });

  it('is false for a null session or empty identifiers', () => {
    expect(isAdoptedFlushVersion(null, { versionId: 'v2', etag: '"v2"' })).toBe(false);
    expect(isAdoptedFlushVersion(session, {})).toBe(false);
  });
});

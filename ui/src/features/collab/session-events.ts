export interface DocumentEventPayload {
  type: string;
  path?: string | null;
  from?: string | null;
  to?: string | null;
  doc_id?: string | null;
  version_id?: string | null;
  etag?: string | null;
  collab_session_id?: string | null;
}

export interface LiveCollabSession {
  documentId: string;
  path: string;
  sessionId?: string | null;
  ackedFlushVersionIds?: ReadonlySet<string>;
  ackedFlushEtags?: ReadonlySet<string>;
}

export type LiveDocumentEventDecision =
  | { action: 'pass' }
  | { action: 'ignore_flush_echo' }
  | { action: 'adopt_injected'; etag: string; versionId: string }
  | { action: 'external_change' }
  | { action: 'external_delete' }
  | { action: 'retarget_move'; path: string };

export function classifyLiveDocumentEvent(
  payload: DocumentEventPayload,
  session: LiveCollabSession | null
): LiveDocumentEventDecision {
  if (!session || !matchesLiveDocument(payload, session)) return { action: 'pass' };

  if (payload.type === 'doc.changed') {
    if (
      payload.collab_session_id?.startsWith('agent-injected:') &&
      payload.version_id &&
      payload.etag
    ) {
      return { action: 'adopt_injected', etag: payload.etag, versionId: payload.version_id };
    }
    return isOwnFlushEcho(payload, session)
      ? { action: 'ignore_flush_echo' }
      : { action: 'external_change' };
  }

  if (payload.type === 'doc.deleted') {
    return { action: 'external_delete' };
  }

  if (payload.type === 'doc.moved' && payload.to) {
    return { action: 'retarget_move', path: payload.to };
  }

  return { action: 'pass' };
}

function matchesLiveDocument(payload: DocumentEventPayload, session: LiveCollabSession) {
  if (payload.doc_id) return payload.doc_id === session.documentId;
  return payload.path === session.path || payload.from === session.path;
}

function isOwnFlushEcho(payload: DocumentEventPayload, session: LiveCollabSession) {
  if (payload.collab_session_id && payload.collab_session_id === session.sessionId) return true;
  if (payload.collab_session_id?.startsWith('browser:')) return true;
  return isAdoptedFlushVersion(session, {
    versionId: payload.version_id,
    etag: payload.etag,
  });
}

/**
 * Whether `session` has already adopted the given document version — either a
 * flush this browser/peer acknowledged or a server agent-injection it adopted
 * (both record into the acked sets). Used to recognize a save 412 whose remote
 * is a version we already have, so it reconciles silently instead of surfacing
 * a spurious conflict. A genuinely external version is in neither set.
 */
export function isAdoptedFlushVersion(
  session: LiveCollabSession | null,
  version: { versionId?: string | null; etag?: string | null }
): boolean {
  if (!session) return false;
  if (version.versionId && session.ackedFlushVersionIds?.has(version.versionId)) return true;
  if (version.etag && session.ackedFlushEtags?.has(version.etag)) return true;
  return false;
}

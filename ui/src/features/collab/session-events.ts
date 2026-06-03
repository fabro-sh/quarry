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
  | { action: 'external_change' }
  | { action: 'external_delete' }
  | { action: 'retarget_move'; path: string };

export function classifyLiveDocumentEvent(
  payload: DocumentEventPayload,
  session: LiveCollabSession | null
): LiveDocumentEventDecision {
  if (!session || !matchesLiveDocument(payload, session)) return { action: 'pass' };

  if (payload.type === 'doc.changed') {
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
  if (payload.version_id && session.ackedFlushVersionIds?.has(payload.version_id)) return true;
  if (payload.etag && session.ackedFlushEtags?.has(payload.etag)) return true;
  return false;
}

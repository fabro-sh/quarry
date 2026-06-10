export interface DocumentEventPayload {
  type: string;
  path?: string | null;
  from?: string | null;
  to?: string | null;
  doc_id?: string | null;
  version_id?: string | null;
  etag?: string | null;
  origin_id?: string | null;
}

export interface LiveCollabSession {
  documentId: string;
  path: string;
}

export type LiveDocumentEventDecision =
  | { action: 'pass' }
  | { action: 'session_refresh' }
  | { action: 'retarget_move'; path: string };

/**
 * SSE classification for the open session-backed document. Every write path
 * lands in the live doc through the gateway — checkpoints of the browser's
 * own typing, agent transactions, and whole-file writes all merge in as
 * collaborator edits over the websocket — so a `doc.changed` for the open
 * document never carries content the editor is missing. It is always a
 * benign refresh signal for the metadata caches (versions, links, search).
 * `doc.moved` retargets the open path. Everything else takes the caller's
 * generic handling. (The legacy "External version available" classification
 * died with the autosave/draft machinery in Phase 5.)
 */
export function classifyLiveDocumentEvent(
  payload: DocumentEventPayload,
  session: LiveCollabSession | null
): LiveDocumentEventDecision {
  if (!session || !matchesLiveDocument(payload, session)) return { action: 'pass' };

  if (payload.type === 'doc.changed') return { action: 'session_refresh' };
  if (payload.type === 'doc.moved' && payload.to) {
    return { action: 'retarget_move', path: payload.to };
  }
  return { action: 'pass' };
}

function matchesLiveDocument(payload: DocumentEventPayload, session: LiveCollabSession) {
  if (payload.doc_id) return payload.doc_id === session.documentId;
  return payload.path === session.path || payload.from === session.path;
}

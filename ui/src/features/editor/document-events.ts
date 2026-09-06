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

export interface OpenDocument {
  documentId: string;
  path: string;
}

export type DocumentEventDecision =
  | { action: 'pass' }
  | { action: 'document_refresh' }
  | { action: 'retarget_move'; path: string };

/** Document commands synchronize content; application events refresh metadata and paths. */
export function classifyDocumentEvent(
  payload: DocumentEventPayload,
  session: OpenDocument | null
): DocumentEventDecision {
  if (!session || !matchesLiveDocument(payload, session)) return { action: 'pass' };

  if (payload.type === 'doc.changed') return { action: 'document_refresh' };
  if (payload.type === 'doc.moved' && payload.to) {
    return { action: 'retarget_move', path: payload.to };
  }
  return { action: 'pass' };
}

function matchesLiveDocument(payload: DocumentEventPayload, session: OpenDocument) {
  if (payload.doc_id) return payload.doc_id === session.documentId;
  return payload.path === session.path || payload.from === session.path;
}

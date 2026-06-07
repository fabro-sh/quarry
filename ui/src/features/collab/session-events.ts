import type { ReviewMetaPatch } from '../review/rfm-types';
import { collabDebug } from './collab-debug';

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
  sessionId?: string | null;
  ackedFlushVersionIds?: ReadonlySet<string>;
  ackedFlushEtags?: ReadonlySet<string>;
}

export type LiveDocumentEventDecision =
  | { action: 'pass' }
  | { action: 'ignore_own_mutation_echo' }
  | { action: 'agent_injection_refresh' }
  | { action: 'external_change' }
  | { action: 'external_delete' }
  | { action: 'retarget_move'; path: string };

export function classifyLiveDocumentEvent(
  payload: DocumentEventPayload,
  session: LiveCollabSession | null
): LiveDocumentEventDecision {
  if (!session || !matchesLiveDocument(payload, session)) return { action: 'pass' };

  if (payload.type === 'doc.changed') {
    if (payload.origin_id?.startsWith('agent-injected:')) {
      return { action: 'agent_injection_refresh' };
    }
    return isOwnMutationEcho(payload, session)
      ? { action: 'ignore_own_mutation_echo' }
      : { action: 'external_change' };
  }

  if (payload.type === 'doc.deleted') {
    return isOwnMutationEcho(payload, session)
      ? { action: 'ignore_own_mutation_echo' }
      : { action: 'external_delete' };
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

function isOwnMutationEcho(payload: DocumentEventPayload, session: LiveCollabSession) {
  if (payload.origin_id && payload.origin_id === session.sessionId) return true;
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

export interface InjectionEnvelope {
  etag: string;
  review?: ReviewMetaPatch | null;
  versionId: string;
}

export function parseInjectionEnvelope(raw: unknown): InjectionEnvelope | null {
  if (!isRecord(raw)) return invalidEnvelope('not_object');
  const versionId = raw.version_id;
  const etag = raw.etag;
  if (typeof versionId !== 'string' || typeof etag !== 'string') {
    return invalidEnvelope('missing_version_or_etag');
  }

  if (raw.review === undefined || raw.review === null) {
    return { etag, versionId };
  }
  if (typeof raw.review !== 'string') return invalidEnvelope('review_not_string');
  try {
    const review = JSON.parse(raw.review) as unknown;
    if (!isRecord(review)) return invalidEnvelope('review_not_object');
    return { etag, review: review as ReviewMetaPatch, versionId };
  } catch {
    return invalidEnvelope('review_invalid_json');
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function invalidEnvelope(reason: string): null {
  collabDebug('inject.envelope.invalid', { reason });
  return null;
}

import { DocumentRequestError } from './document-outbox';

export const DOCUMENT_REQUEST_TIMEOUT_MS = 10000;

/** A deadline ends a network attempt, including reading its body. The durable
 * command remains in the outbox until its acknowledgement is recorded. */
export async function documentRequest(url: string, body?: unknown, signal?: AbortSignal, timeoutMs = DOCUMENT_REQUEST_TIMEOUT_MS) {
  const controller = new AbortController();
  const cancel = () => controller.abort(signal?.reason);
  signal?.addEventListener('abort', cancel, { once: true });
  const timer = setTimeout(() => controller.abort(new Error('The document request timed out')), timeoutMs);
  let rejectAborted!: () => void;
  const aborted = new Promise<never>((_resolve, reject) => {
    rejectAborted = () => reject(controller.signal.reason);
    controller.signal.addEventListener('abort', rejectAborted, { once: true });
  });
  if (signal?.aborted) cancel();
  try {
    return await Promise.race([aborted, (async () => {
      controller.signal.throwIfAborted();
      const response = await fetch(url, body === undefined ? { cache: 'no-store', signal: controller.signal } : {
        method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal: controller.signal,
      });
      const payload = await response.json().catch(() => {
        if (response.ok) throw new Error('Invalid document response');
        return { message: response.statusText || 'Invalid document response' };
      });
      controller.signal.throwIfAborted();
      if (!response.ok) throw new DocumentRequestError(response.status, payload.message ?? 'The document request failed');
      return payload;
    })()]);
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', cancel);
    controller.signal.removeEventListener('abort', rejectAborted);
  }
}

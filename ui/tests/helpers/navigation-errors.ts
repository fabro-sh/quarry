export type ReadAbortEvent = { phase: 'start' | 'abort'; id: string; url: string; closing?: boolean }
  | { phase: 'closing'; reads: Array<{ id: string; url: string }> };

/** WebKit reports some caught document-read cancellations as page errors. */
export class NavigationErrors {
  private readonly pending = new Map<object, string>();
  private readonly cancelled = new Map<string, object>();
  private readonly ambiguous = new Set<string>();
  private readonly observed = new Map<string, string>();
  private readonly foreignRedirects = new Set<string>();
  private readonly closing = new Set<string>();
  private readonly deferred: Array<{ url: string; index: number; errors: string[]; foreign?: boolean }> = [];

  constructor(private readonly browser: string, private readonly origin: string) {}

  observeRead(event: ReadAbortEvent) {
    if (event.phase === 'closing') {
      this.closing.clear();
      this.deferred.length = 0;
      for (const read of event.reads) {
        if (this.observed.get(read.url) === read.id) this.closing.add(read.id);
      }
      return;
    }
    if (!this.isDocumentRead(event.url)) return;
    if (event.phase === 'start') {
      this.cancelled.delete(event.url);
      this.foreignRedirects.delete(event.url);
      this.observed.set(event.url, event.id);
      if (event.closing) this.closing.add(event.id);
    } else if (this.observed.get(event.url) === event.id && !this.foreignRedirects.has(event.url)) {
      this.cancelled.set(event.url, event);
    }
  }

  foreignRedirect(url: string) {
    this.foreignRedirects.add(url);
    this.cancelled.delete(url);
    for (const candidate of this.deferred) if (candidate.url === url) candidate.foreign = true;
  }

  navigationCompleted() {
    // A cancelled beforeunload never reaches load, so its errors remain.
    for (const candidate of this.deferred.slice().reverse()) {
      if (!candidate.foreign && !this.foreignRedirects.has(candidate.url)) candidate.errors.splice(candidate.index, 1);
    }
    this.deferred.length = 0;
    this.closing.clear();
  }

  requestStarted(request: object, url: string, method: string, fetch: boolean) {
    if (method !== 'GET' || !fetch || !this.isDocumentRead(url)) return;
    // A later request cannot borrow an earlier request's cancellation.
    this.cancelled.delete(url);
    if ([...this.pending.values()].includes(url)) this.ambiguous.add(url);
    this.pending.set(request, url);
  }

  requestFinished(request: object) {
    const url = this.pending.get(request);
    this.pending.delete(request);
    if (url && ![...this.pending.values()].includes(url)) this.ambiguous.delete(url);
  }

  requestFailed(request: object, reason: string, sameOriginRedirectChain: boolean) {
    const url = this.pending.get(request);
    this.pending.delete(request);
    if (!url) return;
    this.cancelled.delete(url);
    if (!this.ambiguous.has(url) && sameOriginRedirectChain && /cancel(?:led|ed)|abort/i.test(reason)) {
      this.cancelled.set(url, request);
    }
    if (![...this.pending.values()].includes(url)) this.ambiguous.delete(url);
  }

  record(error: Pick<Error, 'message' | 'stack'>, errors: string[]) {
    const url = this.browser === 'webkit'
      ? error.stack?.match(/^Fetch API cannot load (https?:\/\/[^\s]+) due to access control checks\./)?.[1]
      : undefined;
    if (url && this.cancelled.has(url) && this.isHandledSessionRead(error.stack!)) {
      this.cancelled.delete(url);
      return;
    }
    const index = errors.push(error.message) - 1;
    const read = url && this.observed.get(url);
    if (url && read && this.closing.has(read) && !this.foreignRedirects.has(url) && this.isHandledSessionRead(error.stack!)) {
      // WebKit may report its own cancelled fetch diagnostic before either
      // Request.failed or AbortSignal is delivered. The document must actually
      // finish navigating before this handled, old-document read is ignored.
      this.deferred.push({ url, index, errors });
    }
  }

  private isHandledSessionRead(stack: string) {
    try {
      const frames = [...stack.matchAll(/^\s+at ([\w.<>]+) \((https?:\/\/[^\s]+):\d+:\d+\)$/gm)]
        .map((match) => ({ name: match[1], url: new URL(match[2]) }));
      const session = frames.some(({ url }) => url.origin === this.origin && url.pathname === '/src/features/editor/document-request.ts')
        && frames.some(({ name, url }) => (name === 'open' || name === 'refresh') && url.origin === this.origin && url.pathname === '/src/features/editor/document-session.ts');
      const metadata = frames.some(({ name, url }) => name === 'jsonRequest' && url.origin === this.origin && url.pathname === '/src/api/client.ts')
        && frames.some(({ name, url }) => name === 'execute' && url.origin === this.origin && url.pathname === '/node_modules/.vite/deps/swr.js');
      return session || metadata;
    } catch { return false; }
  }

  private isDocumentRead(value: string) {
    const url = new URL(value);
    return url.origin === this.origin && (
      /^\/v1\/(?:libraries\/[^/]+\/documents-by-id|tmp\/documents)\/[^/]+\/document-state$/.test(url.pathname)
      || /^\/v1\/libraries\/[^/]+\/documents(?:\/.+\/(?:presence|outgoing-links|backlinks))?$/.test(url.pathname)
    );
  }
}

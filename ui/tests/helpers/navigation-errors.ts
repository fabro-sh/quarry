/** WebKit reports some caught DocumentSession fetch aborts as page errors. */
export class NavigationErrors {
  private readonly pending = new Map<object, string>();
  private readonly cancelled = new Map<string, object>();
  private readonly ambiguous = new Set<string>();

  constructor(private readonly browser: string, private readonly origin: string) {}

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
    errors.push(error.message);
  }

  private isHandledSessionRead(stack: string) {
    try {
      const frames = [...stack.matchAll(/^\s+at (open|refresh|documentRequest|unknown) \((https?:\/\/[^\s]+):\d+:\d+\)$/gm)]
        .map((match) => ({ name: match[1], url: new URL(match[2]) }));
      return frames.some(({ url }) => url.origin === this.origin && url.pathname === '/src/features/editor/document-request.ts')
        && frames.some(({ name, url }) => (name === 'open' || name === 'refresh') && url.origin === this.origin && url.pathname === '/src/features/editor/document-session.ts');
    } catch { return false; }
  }

  private isDocumentRead(value: string) {
    const url = new URL(value);
    return url.origin === this.origin && /^\/v1\/(?:libraries\/[^/]+\/documents-by-id|tmp\/documents)\/[^/]+\/document-state$/.test(url.pathname);
  }
}

interface PendingError { url: string; index: number; errors: string[] }

/** Match WebKit teardown diagnostics to reads actually cancelled by navigation. */
export class NavigationErrors {
  private readonly pending = new Map<object, string>();
  private readonly departing = new Map<object, string>();
  private readonly cancelledUrls = new Set<string>();
  private readonly ambiguousUrls = new Set<string>();
  private readonly awaitingFailure: PendingError[] = [];
  private preparing = false;
  private navigating = false;

  constructor(private readonly browser: string, private readonly origin: string) {}

  requestStarted(request: object, url: string, method: string, fetch: boolean) {
    if (method !== 'GET' || !fetch || !this.isDocumentRead(url)) return;
    if (this.preparing) {
      // A URL cannot identify the departing request once the next page (or
      // another request on the old page) has fetched the same URL again.
      this.ambiguousUrls.add(url);
      this.cancelledUrls.delete(url);
    }
    this.pending.set(request, url);
  }

  requestFinished(request: object) {
    this.pending.delete(request); this.departing.delete(request);
  }

  requestFailed(request: object, reason: string, sameOriginRedirectChain: boolean) {
    const url = this.departing.get(request);
    this.requestFinished(request);
    if (!this.preparing || !url || this.ambiguousUrls.has(url) || !sameOriginRedirectChain || !/cancel(?:led|ed)|abort/i.test(reason)) return;
    this.cancelledUrls.add(url);
    if (this.navigating) this.discardCancelledErrors();
  }

  private discardCancelledErrors() {
    // WebKit can emit pageerror before requestfailed. Keep it until the
    // network event proves cancellation, rather than hiding generic failures.
    for (let i = this.awaitingFailure.length - 1; i >= 0; i--) {
      const candidate = this.awaitingFailure[i];
      if (this.ambiguousUrls.has(candidate.url) || !this.cancelledUrls.has(candidate.url)) continue;
      candidate.errors.splice(candidate.index, 1);
      this.awaitingFailure.splice(i, 1);
      for (const pending of this.awaitingFailure) {
        if (pending.errors === candidate.errors && pending.index > candidate.index) pending.index--;
      }
    }
  }

  navigationPrepared() {
    this.preparing = true;
    this.navigating = false;
    this.departing.clear(); this.cancelledUrls.clear(); this.ambiguousUrls.clear(); this.awaitingFailure.length = 0;
    for (const [request, url] of this.pending) this.departing.set(request, url);
  }

  navigationStarted() {
    if (!this.preparing) this.navigationPrepared();
    this.navigating = true;
    this.discardCancelledErrors();
  }

  navigationFinished() {
    this.preparing = this.navigating = false;
    for (const request of this.departing.keys()) this.pending.delete(request);
    this.departing.clear(); this.cancelledUrls.clear(); this.ambiguousUrls.clear(); this.awaitingFailure.length = 0;
  }

  record(error: Pick<Error, 'message' | 'stack'>, errors: string[]) {
    const url = this.browser === 'webkit' && this.preparing
      ? error.stack?.match(/^Fetch API cannot load (https?:\/\/[^\s]+) due to access control checks\./)?.[1]
      : undefined;
    if (url && this.navigating && this.cancelledUrls.has(url)) return;
    const index = errors.push(error.message) - 1;
    if (url && (this.cancelledUrls.has(url) || [...this.departing.values()].includes(url))) this.awaitingFailure.push({ url, index, errors });
  }

  private isDocumentRead(value: string) {
    const url = new URL(value);
    if (url.origin !== this.origin) return false;
    return /^\/v1\/libraries\/[^/]+\/documents(?:\/.+\/(?:presence|outgoing-links|backlinks))?$/.test(url.pathname)
      || /^\/v1\/(?:libraries\/[^/]+\/documents-by-id|tmp\/documents)\/[^/]+\/document-state$/.test(url.pathname);
  }
}

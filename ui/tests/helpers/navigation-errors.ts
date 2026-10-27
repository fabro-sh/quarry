export const DOCUMENT_READ_PATH = '^/v1/(?:libraries/[^/]+/documents(?:/.+/(?:presence|outgoing-links|backlinks))?|(?:libraries/[^/]+/documents-by-id|tmp/documents)/[^/]+/document-state)$';
export interface FetchRead { id: string; url: string; at: number }
export type FetchEvent =
  | ({ type: 'start' } & FetchRead)
  | { type: 'abort'; id: string; at: number }
  | { type: 'finish'; id: string; at: number; result: 'fulfilled' | 'rejected' }
  | { type: 'prepare'; at: number; reads: FetchRead[] };
interface ObservedFetch extends FetchRead { abortedAt?: number; finishedAt?: number; result?: 'fulfilled' | 'rejected' }
interface PendingError { url: string; index: number; errors: string[]; network: boolean; fetchId?: string }

/** Match WebKit teardown diagnostics to reads actually cancelled by navigation. */
export class NavigationErrors {
  private readonly pending = new Map<object, string>();
  private readonly departing = new Map<object, string>();
  private readonly cancelledUrls = new Set<string>();
  private readonly ambiguousUrls = new Set<string>();
  private readonly awaitingFailure: PendingError[] = [];
  private readonly fetches = new Map<string, ObservedFetch>();
  private readonly departingFetches = new Set<string>();
  private readonly networkReads: Array<{ url: string; at: () => number | undefined }> = [];
  private preparing = false;
  private navigating = false;
  private preparedAt = 0;

  constructor(private readonly browser: string, private readonly origin: string) {}

  requestStarted(request: object, url: string, method: string, fetch: boolean, at?: number | (() => number | undefined)) {
    if (method !== 'GET' || !fetch || !this.isDocumentRead(url)) return;
    this.networkReads.push({ url, at: typeof at === 'function' ? at : () => at });
    if (this.preparing) this.ambiguousUrls.add(url);
    this.pending.set(request, url);
  }

  requestFinished(request: object) { this.pending.delete(request); this.departing.delete(request); }

  requestFailed(request: object, reason: string, sameOriginRedirectChain: boolean) {
    const url = this.departing.get(request);
    this.requestFinished(request);
    if (!this.preparing || !url || !sameOriginRedirectChain || !/cancel(?:led|ed)|abort/i.test(reason)) return;
    this.cancelledUrls.add(url);
    if (this.navigating) this.discardCancelledErrors(false);
  }

  fetchEvent(event: FetchEvent) {
    if (!Number.isFinite(event.at)) return;
    if (event.type === 'prepare') {
      for (const read of event.reads) this.addFetch(read);
      if (this.navigating) {
        // Network navigation and exposed JS bindings use separate protocol
        // messages. A late beforeunload report must not undo confirmation.
        this.preparedAt = event.at;
        for (const read of this.fetches.values()) {
          if (read.at <= event.at && (read.finishedAt === undefined || read.finishedAt >= event.at)) this.departingFetches.add(read.id);
        }
      } else this.navigationPrepared(event.at);
      return;
    }
    if (event.type === 'start') { this.addFetch(event); return; }
    const read = this.fetches.get(event.id);
    if (!read) return;
    if (event.type === 'abort') read.abortedAt = event.at;
    else { read.finishedAt = event.at; read.result = event.result; }
  }

  private addFetch(read: FetchRead) {
    if (!this.isDocumentRead(read.url) || !Number.isFinite(read.at) || this.fetches.has(read.id)) return;
    this.fetches.set(read.id, { id: read.id, url: read.url, at: read.at });
    if (this.preparing) {
      if (read.at <= this.preparedAt) this.departingFetches.add(read.id);
      else this.ambiguousUrls.add(read.url);
    }
  }

  private preNetworkCancellation(id: string) {
    const read = this.fetches.get(id);
    if (!read || read.abortedAt === undefined || read.abortedAt < this.preparedAt || read.result !== 'rejected'
      || read.finishedAt === undefined || read.finishedAt < read.abortedAt) return false;
    // A real network request must use the network evidence path, including its
    // redirect checks. Epoch timestamps also match late protocol notifications.
    return !this.networkReads.some((request) => {
      if (request.url !== read.url) return false;
      const at = request.at();
      return at === undefined || at >= read.at - 1 && at <= read.abortedAt! + 1;
    });
  }

  private discardCancelledErrors(includePreNetwork: boolean) {
    for (let i = this.awaitingFailure.length - 1; i >= 0; i--) {
      const candidate = this.awaitingFailure[i];
      const proven = candidate.network && this.cancelledUrls.has(candidate.url)
        || includePreNetwork && candidate.fetchId !== undefined && this.preNetworkCancellation(candidate.fetchId);
      if (!proven) continue;
      candidate.errors.splice(candidate.index, 1);
      this.awaitingFailure.splice(i, 1);
      for (const pending of this.awaitingFailure) {
        if (pending.errors === candidate.errors && pending.index > candidate.index) pending.index--;
      }
    }
  }

  navigationPrepared(at = Date.now()) {
    this.preparing = true; this.navigating = false; this.preparedAt = at;
    this.departing.clear(); this.cancelledUrls.clear(); this.ambiguousUrls.clear(); this.awaitingFailure.length = 0;
    this.departingFetches.clear();
    for (const [request, url] of this.pending) this.departing.set(request, url);
    for (const read of this.fetches.values()) {
      if (read.at <= at && (read.finishedAt === undefined || read.finishedAt >= at)) this.departingFetches.add(read.id);
    }
  }

  navigationStarted() {
    if (!this.preparing) this.navigationPrepared();
    this.navigating = true; this.discardCancelledErrors(false);
  }

  navigationFinished(committed = true) {
    if (committed && this.navigating) this.discardCancelledErrors(true);
    this.preparing = this.navigating = false;
    for (const request of this.departing.keys()) this.pending.delete(request);
    for (const id of this.departingFetches) this.fetches.delete(id);
    for (const [id, read] of this.fetches) if (read.finishedAt !== undefined) this.fetches.delete(id);
    this.departing.clear(); this.departingFetches.clear(); this.cancelledUrls.clear(); this.ambiguousUrls.clear(); this.awaitingFailure.length = 0;
  }

  record(error: Pick<Error, 'message' | 'stack'>, errors: string[]) {
    const url = this.browser === 'webkit' && this.preparing
      ? error.stack?.match(/^Fetch API cannot load (https?:\/\/[^\s]+) due to access control checks\./)?.[1]
      : undefined;
    const unambiguous = url !== undefined && !this.ambiguousUrls.has(url);
    if (unambiguous && this.navigating && this.cancelledUrls.has(url)) return;
    const index = errors.push(error.message) - 1;
    if (!unambiguous) return;
    const reads = [...this.departingFetches].filter((id) => this.fetches.get(id)?.url === url);
    const network = this.cancelledUrls.has(url) || [...this.departing.values()].includes(url);
    if (network || reads.length === 1) this.awaitingFailure.push({ url, index, errors, network, fetchId: reads.length === 1 ? reads[0] : undefined });
  }

  private isDocumentRead(value: string) {
    const url = new URL(value);
    return url.origin === this.origin && new RegExp(DOCUMENT_READ_PATH).test(url.pathname);
  }
}

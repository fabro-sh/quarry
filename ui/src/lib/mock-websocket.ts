/** In-memory transport for UI tests; real socket behavior has browser coverage. */
export class MockWebSocket {
  static instances: MockWebSocket[] = [];
  onopen: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onclose: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  closed = false;

  constructor(public readonly url: string) {
    MockWebSocket.instances.push(this);
    queueMicrotask(() => { if (!this.closed) this.onopen?.(new Event('open')); });
  }
  close() { this.closed = true; }
  emit(type: string, payload: Record<string, unknown> = {}) {
    this.onmessage?.(new MessageEvent('message', { data: JSON.stringify({ ...payload, type }) }));
  }
}

import type { TextPoint } from './document-model';

export interface DocumentSelection { client_id: string; author: string; points: TextPoint[] }

/** Ephemeral selections carry native cursors. They never create history. */
export class DocumentPresence {
  private points: TextPoint[] = [];
  private timer?: ReturnType<typeof setTimeout>;
  private closed = false;
  private running = false;
  private started = false;
  constructor(private readonly url: string, private readonly client: string, private readonly author: string,
    private readonly changed: (peers: DocumentSelection[]) => void) {}
  selection(points: TextPoint[]) {
    if (JSON.stringify(points) === JSON.stringify(this.points)) return;
    this.points = points;
    if (this.started) this.schedule(150);
  }
  start() { this.started = true; this.schedule(0); }
  close() {
    this.closed = true; clearTimeout(this.timer);
    if (this.started) void this.post([]).catch(() => {});
  }
  private schedule(delay: number) {
    if (this.closed) return;
    clearTimeout(this.timer); this.timer = setTimeout(() => void this.sync(), delay);
  }
  private post(points: TextPoint[]) {
    return fetch(this.url, { method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ client_id: this.client, author: this.author, points }), keepalive: true });
  }
  private async sync() {
    if (this.closed) return;
    if (this.running) { this.schedule(150); return; }
    this.running = true;
    try {
      const response = await this.post(this.points);
      if (response.ok && !this.closed) this.changed((await response.json() as DocumentSelection[]).filter((peer) => peer.client_id !== this.client));
      else if (!this.closed) this.changed([]);
    } catch { if (!this.closed) this.changed([]); }
    finally { this.running = false; this.schedule(1500); }
  }
}

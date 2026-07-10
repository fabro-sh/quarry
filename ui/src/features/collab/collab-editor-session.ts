import type { CollabSaveState } from './save-state';
import { recordCollabLifecycleEvent } from './collab-debug';

export type CollabSessionLifecycle =
  | 'disabled'
  | 'initializing'
  | 'live'
  | 'reconnecting'
  | 'refused'
  | 'disposed';

export interface CollabSessionSnapshot {
  readonly epoch: number;
  readonly lifecycle: CollabSessionLifecycle;
  readonly readOnly: boolean;
  readonly refusalReason: string | null;
  readonly saveState: CollabSaveState | null;
}

interface ProbeSocket {
  onclose: ((event: CloseEvent) => void) | null;
  onopen: ((event: Event) => void) | null;
  close(): void;
}

interface CollabEditorSessionOptions {
  readonly baseUrl: string;
  readonly enabled: boolean;
  readonly retryMs?: number;
  readonly roomName: string;
  readonly socketFactory?: (url: string) => ProbeSocket;
}

type Listener = () => void;

const DEFAULT_RETRY_MS = 2_000;

/** Owns the reconnect state machine for one document identity. */
export class CollabEditorSession implements Disposable {
  private readonly baseUrl: string;
  private readonly enabled: boolean;
  private readonly listeners = new Set<Listener>();
  private readonly retryMs: number;
  private readonly roomName: string;
  private readonly socketFactory: (url: string) => ProbeSocket;
  private initCompleted = false;
  private probe: ProbeSocket | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private snapshot: CollabSessionSnapshot;

  constructor(options: CollabEditorSessionOptions) {
    this.baseUrl = options.baseUrl;
    this.enabled = options.enabled;
    this.retryMs = options.retryMs ?? DEFAULT_RETRY_MS;
    this.roomName = options.roomName;
    this.socketFactory = options.socketFactory ?? ((url) => new WebSocket(url));
    this.snapshot = options.enabled
      ? {
          epoch: 0,
          lifecycle: 'initializing',
          readOnly: true,
          refusalReason: null,
          saveState: 'reconnecting',
        }
      : {
          epoch: 0,
          lifecycle: 'disabled',
          readOnly: false,
          refusalReason: null,
          saveState: null,
        };
  }

  readonly getSnapshot = (): CollabSessionSnapshot => this.snapshot;

  readonly subscribe = (listener: Listener): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  start(): void {
    if (this.snapshot.lifecycle !== 'disposed') return;
    this.initCompleted = false;
    this.update(
      this.enabled
        ? {
            epoch: 0,
            lifecycle: 'initializing',
            readOnly: true,
            refusalReason: null,
            saveState: 'reconnecting',
          }
        : {
            epoch: 0,
            lifecycle: 'disabled',
            readOnly: false,
            refusalReason: null,
            saveState: null,
          }
    );
  }

  markInitialized(): void {
    if (!this.enabled || this.isTerminal()) return;
    this.initCompleted = true;
    if (this.snapshot.saveState !== 'reconnecting') return;
    this.update({ lifecycle: 'reconnecting', readOnly: true });
    this.startProbe();
  }

  observeSaveState(saveState: CollabSaveState): void {
    if (!this.enabled || this.isTerminal()) return;
    if (saveState === 'refused') {
      this.refuse('Live editing unavailable');
      return;
    }
    if (saveState === 'reconnecting') {
      const lifecycle = this.initCompleted ? 'reconnecting' : 'initializing';
      this.update({ lifecycle, readOnly: true, saveState });
      if (this.initCompleted) this.startProbe();
      return;
    }
    this.stopProbe();
    this.update({ lifecycle: 'live', readOnly: false, saveState });
  }

  refuse(reason: string): void {
    if (!this.enabled || this.snapshot.lifecycle === 'disposed') return;
    this.stopProbe();
    this.update({
      lifecycle: 'refused',
      readOnly: true,
      refusalReason: reason,
      saveState: 'refused',
    });
  }

  /** React lifetime cleanup: stop resources without publishing teardown state. */
  suspend(): void {
    this.stopProbe();
  }

  [Symbol.dispose](): void {
    if (this.snapshot.lifecycle === 'disposed') return;
    this.stopProbe();
    this.update({ lifecycle: 'disposed', readOnly: true });
  }

  private isTerminal(): boolean {
    return this.snapshot.lifecycle === 'disposed' || this.snapshot.lifecycle === 'refused';
  }

  private startProbe(): void {
    if (this.probe || this.retryTimer || this.isTerminal()) return;
    this.attemptProbe();
  }

  private attemptProbe(): void {
    if (this.isTerminal()) return;
    recordCollabLifecycleEvent('probe_attempted');
    let socket: ProbeSocket;
    try {
      socket = this.socketFactory(`${this.baseUrl}/${this.roomName}`);
    } catch {
      this.scheduleProbe();
      return;
    }
    this.probe = socket;
    socket.onopen = () => {
      if (this.probe !== socket || this.isTerminal()) return;
      socket.onopen = null;
      socket.onclose = null;
      this.probe = null;
      socket.close();
      this.initCompleted = false;
      recordCollabLifecycleEvent('session_epoch_started');
      this.update({
        epoch: this.snapshot.epoch + 1,
        lifecycle: 'initializing',
        readOnly: true,
        refusalReason: null,
        saveState: 'reconnecting',
      });
    };
    socket.onclose = () => {
      if (this.probe !== socket) return;
      this.probe = null;
      this.scheduleProbe();
    };
  }

  private scheduleProbe(): void {
    if (this.retryTimer || this.isTerminal()) return;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      this.attemptProbe();
    }, this.retryMs);
  }

  private stopProbe(): void {
    if (this.retryTimer) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
    if (this.probe) {
      const probe = this.probe;
      this.probe = null;
      probe.onopen = null;
      probe.onclose = null;
      probe.close();
    }
  }

  private update(update: Partial<CollabSessionSnapshot>): void {
    const next = { ...this.snapshot, ...update };
    if (
      next.epoch === this.snapshot.epoch &&
      next.lifecycle === this.snapshot.lifecycle &&
      next.readOnly === this.snapshot.readOnly &&
      next.refusalReason === this.snapshot.refusalReason &&
      next.saveState === this.snapshot.saveState
    ) {
      return;
    }
    this.snapshot = next;
    for (const listener of this.listeners) listener();
  }
}

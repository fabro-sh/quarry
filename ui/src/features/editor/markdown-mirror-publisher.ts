import type { Descendant } from 'platejs';

import { recordCollabLifecycleEvent } from '../collab/collab-debug';
import type { ReviewMeta } from '../review/rfm-types';

interface MarkdownMirrorPublisherOptions {
  readonly debounceMs: number;
  readonly getMeta: () => ReviewMeta;
  readonly getValue: () => readonly Descendant[];
  readonly publish: (markdown: string, guardUnhydratedBlank: boolean) => void;
  readonly serialize: (
    value: readonly Descendant[],
    meta: ReviewMeta
  ) => Promise<string | null>;
}

interface ScheduleMirrorOptions {
  readonly guardUnhydratedBlank?: boolean;
}

/** Owns debounce state and latest-wins receipts for the Markdown UI mirror. */
export class MarkdownMirrorPublisher implements Disposable {
  private readonly debounceMs: number;
  private readonly getMeta: () => ReviewMeta;
  private readonly getValue: () => readonly Descendant[];
  private readonly publish: (markdown: string, guardUnhydratedBlank: boolean) => void;
  private readonly serialize: MarkdownMirrorPublisherOptions['serialize'];
  private guardUnhydratedBlank = false;
  private receipt = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(options: MarkdownMirrorPublisherOptions) {
    this.debounceMs = options.debounceMs;
    this.getMeta = options.getMeta;
    this.getValue = options.getValue;
    this.publish = options.publish;
    this.serialize = options.serialize;
  }

  readonly schedule = (options: ScheduleMirrorOptions = {}): void => {
    recordCollabLifecycleEvent('mirror_scheduled');
    if (options.guardUnhydratedBlank) this.guardUnhydratedBlank = true;
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = null;
      const guardUnhydratedBlank = this.guardUnhydratedBlank;
      this.guardUnhydratedBlank = false;
      const receipt = ++this.receipt;
      void this.serialize(this.getValue(), this.getMeta()).then((markdown) => {
        if (markdown === null || receipt !== this.receipt) return;
        recordCollabLifecycleEvent('mirror_completed');
        this.publish(markdown, guardUnhydratedBlank);
      });
    }, this.debounceMs);
  };

  cancel(): void {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.guardUnhydratedBlank = false;
    this.receipt += 1;
  }

  [Symbol.dispose](): void {
    this.cancel();
  }
}

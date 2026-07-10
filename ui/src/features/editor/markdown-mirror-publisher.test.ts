import { afterEach, describe, expect, test, vi } from 'vitest';

import {
  collabLifecycleSnapshot,
  resetCollabLifecycleCounters,
} from '../collab/collab-debug';
import { emptyReviewMeta } from '../review/rfm-types';
import { MarkdownMirrorPublisher } from './markdown-mirror-publisher';

function deferred<T>(): { readonly promise: Promise<T>; readonly resolve: (value: T) => void } {
  let resolvePromise = (_value: T): void => {
    throw new Error('deferred promise was not initialized');
  };
  const promise = new Promise<T>((resolve) => {
    resolvePromise = resolve;
  });
  return { promise, resolve: resolvePromise };
}

afterEach(() => {
  vi.useRealTimers();
  resetCollabLifecycleCounters();
});

describe('MarkdownMirrorPublisher', () => {
  test('coalesces a typing burst into one serialization', async () => {
    vi.useFakeTimers();
    const publish = vi.fn();
    const serialize = vi.fn(async () => '# Guideabc');
    const publisher = new MarkdownMirrorPublisher({
      debounceMs: 25,
      getMeta: () => emptyReviewMeta(),
      getValue: () => [{ type: 'p', children: [{ text: 'Guideabc' }] }],
      publish,
      serialize,
    });

    publisher.schedule();
    publisher.schedule();
    publisher.schedule();
    await vi.advanceTimersByTimeAsync(25);

    expect(serialize).toHaveBeenCalledTimes(1);
    expect(publish).toHaveBeenCalledWith('# Guideabc', false);
    expect(collabLifecycleSnapshot()).toMatchObject({
      mirror_completed: 1,
      mirror_scheduled: 3,
    });
    publisher[Symbol.dispose]();
  });

  test('drops an in-flight receipt after disposal', async () => {
    vi.useFakeTimers();
    const serialization = deferred<string>();
    const publish = vi.fn();
    const publisher = new MarkdownMirrorPublisher({
      debounceMs: 0,
      getMeta: () => emptyReviewMeta(),
      getValue: () => [],
      publish,
      serialize: () => serialization.promise,
    });

    publisher.schedule({ guardUnhydratedBlank: true });
    await vi.advanceTimersByTimeAsync(0);
    publisher[Symbol.dispose]();
    serialization.resolve('# stale');
    await Promise.resolve();

    expect(publish).not.toHaveBeenCalled();
  });
});

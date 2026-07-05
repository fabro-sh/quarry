import * as Comlink from 'comlink';
import type { Descendant } from 'platejs';

import type { ReviewMeta } from '../review/rfm-types';
import { serializeMirror } from './mirror-serialize';
import type { MirrorSerializerWorkerApi } from './mirror-serializer.worker';

/** The worker-side API surface (see mirror-serializer.worker.ts). */
export interface MirrorSerializeRemote {
  serialize(value: Descendant[], meta: ReviewMeta): Promise<string>;
}

export interface MirrorSerializer {
  /**
   * Serializes for the mirror publish. Latest-wins: resolves `null` when a
   * newer serialize was requested before this one finished.
   */
  serialize(value: Descendant[], meta: ReviewMeta): Promise<string | null>;
  dispose(): void;
}

export interface MirrorSerializerOptions {
  /**
   * Builds the remote endpoint; `null` means no worker is available and
   * serialization runs synchronously on the caller's thread. Injectable for
   * tests; the default spawns the Comlink worker.
   */
  createRemote?: () => MirrorSerializeRemote | null;
}

/**
 * A worker that fails to load (or crashes) never answers Comlink calls, so
 * the remote's promises would stay pending forever. Racing every call against
 * the worker's `error` event turns that hang into a rejection, which
 * `createMirrorSerializer` catches to downgrade to synchronous serialization.
 */
export function rejectOnWorkerError(
  worker: { addEventListener(type: 'error', listener: (event: ErrorEvent) => void): void },
  remote: MirrorSerializeRemote
): MirrorSerializeRemote {
  const failure = new Promise<never>((_, reject) => {
    worker.addEventListener('error', (event) => {
      reject(new Error(`mirror serializer worker failed: ${event.message}`));
    });
  });
  // The failure promise outlives individual calls; without a handler of its
  // own it would surface as an unhandled rejection.
  failure.catch(() => {});
  return {
    serialize: (value, meta) => Promise.race([remote.serialize(value, meta), failure]),
  };
}

function createWorkerRemote(): MirrorSerializeRemote | null {
  // jsdom (tests) and any environment without workers use the synchronous
  // fallback; the editor behaves identically, just on the main thread.
  if (typeof Worker === 'undefined') return null;
  try {
    const worker = new Worker(new URL('./mirror-serializer.worker.ts', import.meta.url), {
      type: 'module',
    });
    const remote = Comlink.wrap<MirrorSerializerWorkerApi>(worker);
    return rejectOnWorkerError(worker, {
      serialize: (value, meta) => remote.serialize(value, meta),
    });
  } catch {
    return null;
  }
}

export function createMirrorSerializer(options: MirrorSerializerOptions = {}): MirrorSerializer {
  const createRemote = options.createRemote ?? createWorkerRemote;
  let remote: MirrorSerializeRemote | null | undefined;
  let latest = 0;

  return {
    async serialize(value, meta) {
      const seq = ++latest;
      if (remote === undefined) remote = createRemote();
      let markdown: string;
      if (remote) {
        try {
          markdown = await remote.serialize(value, meta);
        } catch {
          // A crashed or unloadable worker downgrades to the synchronous
          // path for the rest of the session rather than failing publishes.
          remote = null;
          markdown = serializeMirror(value, meta);
        }
      } else {
        markdown = serializeMirror(value, meta);
      }
      return seq === latest ? markdown : null;
    },
    dispose() {
      latest += 1;
      remote = undefined;
    },
  };
}

let appSerializer: MirrorSerializer | null = null;

/** The app-wide serializer (one worker, shared across editor mounts). */
export function getMirrorSerializer(): MirrorSerializer {
  appSerializer ??= createMirrorSerializer();
  return appSerializer;
}

import * as Comlink from 'comlink';
import type { Descendant } from 'platejs';

import type { ReviewMeta } from '../review/rfm-types';
import { serializeMirror } from './mirror-serialize';

// The mirror serialization is O(document size) with a heavy constant
// (per-node option merging in Plate's markdown serializer) — north of a
// second for very large documents. Off the main thread it can't block
// typing, scrolling, or paint. The whole codec graph is headless (Base*
// Plate plugins, core createSlateEditor), so it runs here unchanged.
const api = {
  serialize(value: Descendant[], meta: ReviewMeta): string {
    return serializeMirror(value, meta);
  },
};

export type MirrorSerializerWorkerApi = typeof api;

Comlink.expose(api);

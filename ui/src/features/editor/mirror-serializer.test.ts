import { describe, expect, it } from 'vitest';
import type { Descendant } from 'platejs';

import { emptyReviewMeta } from '../review/rfm-types';
import { createMirrorSerializer, type MirrorSerializeRemote } from './mirror-serializer';
import { serializeMirror } from './mirror-serialize';

const VALUE: Descendant[] = [{ type: 'p', children: [{ text: 'hello mirror' }] }];

describe('createMirrorSerializer', () => {
  it('serializes synchronously when no worker is available', async () => {
    const serializer = createMirrorSerializer({ createRemote: () => null });

    const markdown = await serializer.serialize(VALUE, emptyReviewMeta());

    expect(markdown).toContain('hello mirror');
    serializer.dispose();
  });

  it('resolves superseded requests with null (latest wins)', async () => {
    // A remote whose responses can be released out from under the caller.
    const pending: Array<() => void> = [];
    const remote: MirrorSerializeRemote = {
      serialize: (value, meta) =>
        new Promise((resolve) => {
          pending.push(() => resolve(serializeMirror(value, meta)));
        }),
    };
    const serializer = createMirrorSerializer({ createRemote: () => remote });

    const first = serializer.serialize(VALUE, emptyReviewMeta());
    const second = serializer.serialize(VALUE, emptyReviewMeta());
    pending[0]();
    pending[1]();

    expect(await first).toBeNull();
    expect(await second).toContain('hello mirror');
    serializer.dispose();
  });

  it('falls back to synchronous serialization when the remote fails', async () => {
    const remote: MirrorSerializeRemote = {
      serialize: () => Promise.reject(new Error('worker exploded')),
    };
    const serializer = createMirrorSerializer({ createRemote: () => remote });

    const markdown = await serializer.serialize(VALUE, emptyReviewMeta());

    expect(markdown).toContain('hello mirror');
    serializer.dispose();
  });
});

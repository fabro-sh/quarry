import { circularLayout, layoutGraphNodes } from './layout';

describe('graph layout', () => {
  it('computes stable circular coordinates without a worker', async () => {
    const positions = await layoutGraphNodes(
      [
        { id: 'a' },
        { id: 'b' },
        { id: 'c' },
      ],
      { createWorker: () => null }
    );

    expect(positions.map((position) => position.id)).toEqual(['a', 'b', 'c']);
    expect(positions[0]).toMatchObject({ id: 'a', x: 1, y: 0 });
  });

  it('accepts worker-produced positions when a worker is available', async () => {
    const positions = await layoutGraphNodes([{ id: 'a' }], {
      createWorker: () => ({
        onmessage: null,
        onerror: null,
        postMessage(message: unknown) {
          expect(message).toEqual({ nodes: [{ id: 'a' }] });
          queueMicrotask(() => {
            this.onmessage?.({ data: [{ id: 'a', x: 4, y: 2 }] });
          });
        },
        terminate() {},
      }),
    });

    expect(positions).toEqual([{ id: 'a', x: 4, y: 2 }]);
  });

  it('falls back to circular layout when worker layout fails', async () => {
    await expect(
      layoutGraphNodes([{ id: 'a' }], {
        createWorker: () => ({
          onmessage: null,
          onerror: null,
          postMessage() {
            queueMicrotask(() => this.onerror?.(new Error('worker failed')));
          },
          terminate() {},
        }),
      })
    ).resolves.toEqual(circularLayout([{ id: 'a' }]));
  });
});

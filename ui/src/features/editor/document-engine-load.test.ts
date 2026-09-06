import { loadDocumentEngine } from './document-model';

const { initialize } = vi.hoisted(() => ({ initialize: vi.fn() }));
vi.mock('../../generated/document/quarry_document', () => ({ default: initialize }));

test('failed initialization can retry while concurrent and successful callers share one load', async () => {
  const failure = new Error('Engine download interrupted');
  initialize.mockRejectedValueOnce(failure).mockResolvedValueOnce({});
  const first = loadDocumentEngine();
  const concurrent = loadDocumentEngine();
  expect(concurrent).toBe(first);
  await expect(first).rejects.toBe(failure);
  await expect(concurrent).rejects.toBe(failure);
  expect(initialize).toHaveBeenCalledTimes(1);

  const retry = loadDocumentEngine();
  expect(retry).not.toBe(first);
  await expect(retry).resolves.toEqual({});
  expect(loadDocumentEngine()).toBe(retry);
  expect(initialize).toHaveBeenCalledTimes(2);
});

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { initSync } from '../../generated/document/quarry_document';
import { DocumentModel } from './document-model';

beforeAll(() => initSync({ module: readFileSync(resolve(process.cwd(), 'src/generated/document/quarry_document_bg.wasm')) }));

test('block marker reads stay current after local edits, transactions, remote changes and restoring saved state', () => {
  const model = DocumentModel.fromMarkdown('Before TARGET after.');
  const id = model.view().blocks[0].block.id;
  const owner = { kind: 'block' as const, id };
  model.edit((draft) => draft.apply([{ op: 'add_comment', id: 'comment', author: 'Reviewer', body: 'Keep target', ranges: draft.model.selection(id, 7, 13) }]));
  const saved = model.save();
  const view = vi.spyOn(model, 'view').mockImplementation(() => { throw new Error('Marker reads must not project the entire document'); });
  try {
    expect(model.reviewMarkers(owner)).toEqual([['comment', 'comment', 7, 13]]);
    model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point(id, 0), text: 'X' }]));
    expect(model.reviewMarkers(owner)).toEqual([['comment', 'comment', 8, 14]]);
    const transaction = model.beginTransaction();
    transaction.apply([{ op: 'insert_text', at: transaction.point(id, 0), text: 'Y' }]);
    transaction.finish();
    expect(model.reviewMarkers(owner)).toEqual([['comment', 'comment', 9, 15]]);
    const remote = new DocumentModel(model.save());
    try {
      remote.edit((draft) => draft.apply([{ op: 'resolve_comment', id: 'comment', resolved: true }]));
      model.merge(remote.save());
      expect(model.reviewMarkers(owner)).toEqual([]);
    } finally { remote.dispose(); }
    view.mockRestore();
    model.useSavedVersion(saved);
    expect(model.reviewMarkers(owner)).toEqual([['comment', 'comment', 7, 13]]);
  } finally { view.mockRestore(); model.dispose(); }
});

import { buildDocumentTree, droppedDocumentPath, flattenTreePaths } from './tree-model';

describe('buildDocumentTree', () => {
  it('derives folders from document paths without treating empty directories as documents', () => {
    const tree = buildDocumentTree([
      { id: '1', path: 'daily/2026-05-28.md', title: 'May 28' },
      { id: '2', path: 'daily/2026-05-29.md', title: 'May 29' },
      { id: '3', path: 'projects/quarry/spec.md', title: 'Spec' },
    ]);

    expect(tree).toMatchObject([
      {
        id: 'folder:daily',
        name: 'daily',
        kind: 'folder',
        children: [
          { id: 'doc:daily/2026-05-28.md', name: 'May 28', kind: 'document' },
          { id: 'doc:daily/2026-05-29.md', name: 'May 29', kind: 'document' },
        ],
      },
      {
        id: 'folder:projects',
        name: 'projects',
        kind: 'folder',
        children: [
          {
            id: 'folder:projects/quarry',
            name: 'quarry',
            kind: 'folder',
            children: [{ id: 'doc:projects/quarry/spec.md', name: 'Spec' }],
          },
        ],
      },
    ]);
    expect(flattenTreePaths(tree)).toEqual([
      'daily',
      'daily/2026-05-28.md',
      'daily/2026-05-29.md',
      'projects',
      'projects/quarry',
      'projects/quarry/spec.md',
    ]);
  });

  it('keeps the tree model bounded for ten thousand document paths', () => {
    const documents = Array.from({ length: 10_000 }, (_, index) => ({
      id: String(index),
      path: `folder-${Math.floor(index / 100)}/note-${index}.md`,
      title: `Note ${index}`,
    }));

    const tree = buildDocumentTree(documents);

    expect(tree).toHaveLength(100);
    expect(flattenTreePaths(tree)).toHaveLength(10_100);
  });

  it('preserves the filename when computing a drag-and-drop folder move', () => {
    expect(
      droppedDocumentPath(
        { id: 'doc:notes/source.md', name: 'Source', path: 'notes/source.md', kind: 'document' },
        { id: 'folder:archive', name: 'archive', path: 'archive', kind: 'folder' }
      )
    ).toBe('archive/source.md');
    expect(
      droppedDocumentPath(
        { id: 'doc:notes/source.md', name: 'Source', path: 'notes/source.md', kind: 'document' },
        null
      )
    ).toBe('source.md');
  });
});

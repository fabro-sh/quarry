import { clearDraft, draftKey, loadDraft, saveDraft } from './drafts';

describe('editor drafts', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('keys drafts by library, path, and base ETag', () => {
    expect(draftKey('notes', 'daily/today.md', '"v1"')).toBe(
      'quarry:draft:notes:daily%2Ftoday.md:%22v1%22'
    );
  });

  it('persists and clears unsaved markdown drafts', () => {
    saveDraft('notes', 'a.md', '"v1"', '# Local');

    expect(loadDraft('notes', 'a.md', '"v1"')).toMatchObject({
      content: '# Local',
      library: 'notes',
      path: 'a.md',
      etag: '"v1"',
    });

    clearDraft('notes', 'a.md', '"v1"');

    expect(loadDraft('notes', 'a.md', '"v1"')).toBeNull();
  });
});

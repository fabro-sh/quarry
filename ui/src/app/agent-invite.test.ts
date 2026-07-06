import { tmpWorkspaceRouteForDocument, workspaceRouteForDocument } from './agent-invite';

describe('agent invite helpers', () => {
  it('builds library workspace routes with nested path segments encoded', () => {
    expect(workspaceRouteForDocument('team notes', 'folder/live doc.md')).toBe(
      '/lib/team%20notes/documents/folder/live%20doc.md'
    );
  });

  it('builds tmp workspace routes from the secret', () => {
    expect(tmpWorkspaceRouteForDocument('72cb58585aa73e35758bc1141f79e32e')).toBe(
      '/tmp/72cb58585aa73e35758bc1141f79e32e'
    );
  });
});

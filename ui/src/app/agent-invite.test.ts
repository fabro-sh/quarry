import {
  buildAddAgentPrompt,
  buildTokenizedDocumentUrl,
  workspaceRouteForDocument,
} from './agent-invite';

describe('agent invite helpers', () => {
  it('builds tokenized document urls with nested path segments encoded', () => {
    expect(workspaceRouteForDocument('team notes', 'folder/live doc.md')).toBe(
      '/lib/team%20notes/documents/folder/live%20doc.md'
    );
    expect(
      buildTokenizedDocumentUrl({
        origin: 'http://127.0.0.1:5173',
        library: 'team notes',
        path: 'folder/live doc.md',
        token: 'invite-token',
      })
    ).toBe('http://127.0.0.1:5173/lib/team%20notes/documents/folder/live%20doc.md?token=invite-token');
  });

  it('generates the Proof-style agent prompt with all required Quarry endpoints', () => {
    const prompt = buildAddAgentPrompt({
      origin: 'http://127.0.0.1:5173',
      library: 'team notes',
      path: 'folder/live doc.md',
      tokenizedDocUrl:
        'http://127.0.0.1:5173/lib/team%20notes/documents/folder/live%20doc.md?token=invite-token',
    });

    expect(prompt).toContain('Quarry is a local-first collaborative Markdown editor');
    expect(prompt).toContain(
      'http://127.0.0.1:5173/lib/team%20notes/documents/folder/live%20doc.md?token=invite-token'
    );
    expect(prompt).toContain('trusted-localhost');
    expect(prompt).toContain('REST agent endpoints on this host do not currently enforce bearer-token auth');
    expect(prompt).toContain('API base: http://127.0.0.1:5173/v1');
    expect(prompt).toContain('Library: team notes');
    expect(prompt).toContain('Document path: folder/live doc.md');
    expect(prompt).toContain(
      'POST http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/presence'
    );
    expect(prompt).not.toContain('/snapshot');
    expect(prompt).toContain(
      'Connected in Quarry and ready.\n   <one-sentence summary of the document>'
    );
    expect(prompt).toContain(
      'GET http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/events/stream'
    );
    expect(prompt).toContain(
      'GET http://127.0.0.1:5173/v1/libraries/team%20notes/events/pending?after=<last-seen-id>'
    );
    expect(prompt).toContain(
      'GET http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/blocks'
    );
    expect(prompt).toContain(
      'POST http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/transactions'
    );
    expect(prompt).toContain(
      'GET http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/review'
    );
    expect(prompt).toContain('client_tx_id');
    expect(prompt).toContain('base_clock');
    expect(prompt).toContain('suggestion.accept, suggestion.reject');
    expect(prompt).toContain('{code, retryable, message}');
    // The quarantined legacy facades are no longer advertised.
    expect(prompt).not.toContain('/edit');
    expect(prompt).not.toContain('/ops');
    expect(prompt).not.toContain('baseToken');
    expect(prompt).toContain('Skill: http://127.0.0.1:5173/quarry.SKILL.md');
    expect(prompt).toContain('Docs: http://127.0.0.1:5173/agent-docs');
    expect(prompt).toContain('Discovery: http://127.0.0.1:5173/.well-known/agent.json');
  });
});

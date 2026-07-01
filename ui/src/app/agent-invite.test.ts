import {
  buildAddAgentPrompt,
  buildTokenizedDocumentUrl,
  tmpWorkspaceRouteForDocument,
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

  it('builds tmp tokenized URLs and tmp-scoped agent prompts', () => {
    expect(tmpWorkspaceRouteForDocument('scratch/live doc.md')).toBe('/tmp/scratch/live%20doc.md');
    const tokenizedDocUrl = buildTokenizedDocumentUrl({
      origin: 'http://127.0.0.1:5173',
      scope: 'tmp',
      path: 'scratch/live doc.md',
      token: 'tmp-invite-token',
    });
    expect(tokenizedDocUrl).toBe(
      'http://127.0.0.1:5173/tmp/scratch/live%20doc.md?token=tmp-invite-token'
    );

    const prompt = buildAddAgentPrompt({
      origin: 'http://127.0.0.1:5173',
      scope: 'tmp',
      path: 'scratch/live doc.md',
      tokenizedDocUrl,
    });

    expect(prompt).toContain('Scope: tmp document');
    expect(prompt).not.toContain('Library:');
    expect(prompt).toContain(
      'POST http://127.0.0.1:5173/v1/tmp/documents/scratch/live%20doc.md/presence'
    );
    expect(prompt).toContain(
      'GET http://127.0.0.1:5173/v1/tmp/documents/scratch/live%20doc.md/events/stream'
    );
    expect(prompt).toContain(
      'GET http://127.0.0.1:5173/v1/tmp/documents/scratch/live%20doc.md/blocks'
    );
    expect(prompt).toContain(
      'POST http://127.0.0.1:5173/v1/tmp/documents/scratch/live%20doc.md/transactions'
    );
    expect(prompt).toContain(
      'GET http://127.0.0.1:5173/v1/tmp/documents/scratch/live%20doc.md/review'
    );
    expect(prompt).toContain('re-POST presence at least once per minute');
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
    // The whole-document Markdown PUT is advertised as a write path.
    expect(prompt).toContain(
      'PUT http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md with a plain Markdown body'
    );
    expect(prompt).toContain('If-Match: "<document_clock>"');
    expect(prompt).toContain('{code, retryable, message}');
    // The quarantined legacy facades are no longer advertised.
    expect(prompt).not.toContain('/edit');
    expect(prompt).not.toContain('/ops');
    expect(prompt).not.toContain('baseToken');
    expect(prompt).toContain('Skill: http://127.0.0.1:5173/quarry.SKILL.md');
    expect(prompt).toContain('Docs: http://127.0.0.1:5173/agent-docs');
    expect(prompt).toContain('Discovery: http://127.0.0.1:5173/.well-known/agent.json');
  });

  it('frames reading the skill as a numbered prerequisite to writing, not an optional extra', () => {
    const prompt = buildAddAgentPrompt({
      origin: 'http://127.0.0.1:5173',
      library: 'team notes',
      path: 'folder/live doc.md',
      tokenizedDocUrl:
        'http://127.0.0.1:5173/lib/team%20notes/documents/folder/live%20doc.md?token=invite-token',
    });

    expect(prompt).toContain(
      '4. Read the skill document BEFORE your first edit, comment, or suggestion.'
    );
    expect(prompt).toContain('Do not guess these.');
    // The vocabulary agents guess wrong without the skill is called out inline.
    expect(prompt).toContain('there is no list type');
    expect(prompt).not.toContain('If you need setup details');
    // The skill step comes before the monitoring and editing steps.
    const skillStep = prompt.indexOf('Read the skill document');
    const editStep = prompt.indexOf('Do not edit until the user gives further instructions');
    expect(skillStep).toBeGreaterThan(-1);
    expect(skillStep).toBeLessThan(editStep);
  });
});

import 'fake-indexeddb/auto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { cleanup, render, screen, waitFor, within, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { initSync } from '../../generated/document/quarry_document';
import { DocumentModel, type DocumentBatch } from './document-model';
import { DocumentMarkdownEditor } from './DocumentMarkdownEditor';
import { acceptAllDocumentSuggestions } from './document-actions';

beforeAll(() => {
  initSync({ module: readFileSync(resolve(process.cwd(), 'src/generated/document/quarry_document_bg.wasm')) });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function server() {
  const authority = DocumentModel.create(crypto.randomUUID());
  authority.edit((draft) => {
    draft.apply([{ op: 'insert_block', block: { id: 'a', kind: 'p', parent: null, position: 0, attrs: {}, text: 'TARGET' } }]);
    draft.apply([{ op: 'add_comment', id: 'comment', author: 'Reviewer', body: 'Keep this text', ranges: draft.model.selection('a', 0, 6) },
      { op: 'propose_insertion', id: 'proposal', author: 'Agent', block: 'a', at: draft.model.point('a', 0), text: 'APPROVED ' }]);
  });
  const requests: DocumentBatch[] = []; const receipts = new Set<string>();
  const streams: EventTarget[] = [];
  vi.stubGlobal('EventSource', class extends EventTarget { constructor() { super(); streams.push(this); } close() {} });
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    if (url.endsWith('/document-state')) return Response.json({ format: 'automerge', document_clock: String(requests.length), bytes: Array.from(authority.save()) });
    if (url.endsWith('/document-commands')) {
      const batch = JSON.parse(String(init?.body)) as DocumentBatch;
      if (!receipts.has(batch.request_id)) { authority.applyBatch(batch); requests.push(batch); receipts.add(batch.request_id); }
      return Response.json({ request_id: batch.request_id });
    }
    throw new Error('Unexpected document request');
  }));
  const collab = { documentId: authority.view().document_id, sessionId: crypto.randomUUID(), onSaveStateChange: vi.fn() };
  return { authority, collab, requests, changed() { for (const stream of streams) stream.dispatchEvent(new Event('doc.changed')); } };
}

test('the native page saves review decisions and replies through the durable command session', async () => {
  const fixture = server();
  try {
    render(<DocumentMarkdownEditor documentUrl="/document" config={fixture.collab} author="Reviewer" mode="viewing" />);
    await screen.findByText('Keep this text');
    const suggestion = screen.getByLabelText('Suggestion by Agent');
    await userEvent.click(within(suggestion).getByRole('button', { name: 'Accept suggestion' }));
    await waitFor(() => expect(fixture.authority.view().blocks[0].text).toBe('APPROVED TARGET'));
    const comment = screen.getByLabelText('Comment by Reviewer');
    await userEvent.click(comment);
    await userEvent.type(within(comment).getByRole('textbox', { name: 'Reply' }), 'Still attached');
    await userEvent.click(within(comment).getByRole('button', { name: 'Submit reply' }));
    await waitFor(() => expect(fixture.authority.view().comments.some((v) => v.comment.body === 'Still attached')).toBe(true));
    expect(fixture.authority.view().comments.find((v) => v.comment.id === 'comment')!.target.attachments[0].quote).toBe('TARGET');
    await waitFor(() => expect(fixture.collab.onSaveStateChange).toHaveBeenLastCalledWith('saved'));
  } finally { cleanup(); fixture.authority.dispose(); }
});

test('a remote split updates the page and its highlights without changing viewing mode', async () => {
  const fixture = server();
  try {
    const rendered = render(<DocumentMarkdownEditor documentUrl="/document" config={fixture.collab} author="Reviewer" mode="viewing" />);
    await screen.findByText('Keep this text');
    fixture.authority.edit((draft) => draft.apply([{ op: 'split_block', block: 'a', at: draft.model.point('a', 3), new_block: 'tail' }]));
    await act(async () => fixture.changed());
    await waitFor(() => expect([...rendered.container.querySelectorAll('.slate-p')].at(1)?.textContent).toBe('GET'));
    expect([...rendered.container.querySelectorAll('[data-comment-id="comment"]')].map((node) => node.textContent).join('')).toBe('TARGET');
    expect(rendered.container.querySelector('.quarry-document-body [role="textbox"]')).toHaveAttribute('contenteditable', 'false');
    rendered.rerender(<DocumentMarkdownEditor documentUrl="/document" config={fixture.collab} author="Reviewer" mode="editing" />);
    expect(rendered.container.querySelector('.quarry-document-body [role="textbox"]')).toHaveAttribute('contenteditable', 'true');
    expect(fixture.requests).toHaveLength(0);
  } finally { cleanup(); fixture.authority.dispose(); }
});

test('a failed reply keeps the typed draft for correction', async () => {
  const fixture = server();
  try {
    render(<DocumentMarkdownEditor documentUrl="/document" config={fixture.collab} author="Reviewer" mode="viewing" />);
    await screen.findByText('Keep this text');
    const thread = screen.getByLabelText('Comment by Reviewer');
    await userEvent.click(thread);
    const input = within(thread).getByRole('textbox', { name: 'Reply' });
    await userEvent.type(input, 'Do not lose my draft');
    const mint = vi.spyOn(crypto, 'randomUUID').mockReturnValueOnce('comment' as ReturnType<typeof crypto.randomUUID>);
    await userEvent.click(within(thread).getByRole('button', { name: 'Submit reply' }));
    await screen.findByRole('alert');
    expect(input).toHaveValue('Do not lose my draft');
    expect(fixture.authority.view().comments).toHaveLength(1);
    mint.mockRestore();
  } finally { cleanup(); fixture.authority.dispose(); }
});

test('a remote source edit preserves the private form and cannot be overwritten by a stale save', async () => {
  const fixture = server();
  fixture.authority.edit((draft) => draft.apply([{ op: 'insert_block', block: {
    id: 'raw', kind: 'raw_markdown', parent: null, position: 1, text: '', attrs: { markdown: '<custom>Original</custom>' },
  } }]));
  try {
    const rendered = render(<DocumentMarkdownEditor documentUrl="/document" config={fixture.collab} author="Reviewer" mode="editing" />);
    await userEvent.click(await screen.findByRole('button', { name: 'Edit source' }));
    const input = screen.getByRole('textbox', { name: 'Markdown source' });
    await userEvent.clear(input); await userEvent.type(input, 'Private draft');
    fixture.authority.edit((draft) => draft.apply([
      { op: 'set_block', block: 'raw', kind: 'raw_markdown', attrs: { markdown: '<custom>Agent</custom>' } },
      { op: 'insert_text', at: fixture.authority.point('a', 6), text: ' arrived' },
    ]));
    await act(async () => fixture.changed());
    await waitFor(() => expect(rendered.container.querySelector('.slate-p')?.textContent).toContain('arrived'));
    expect(screen.getByRole('textbox', { name: 'Markdown source' })).toBe(input);
    expect(input).toHaveValue('Private draft');
    await userEvent.click(screen.getByRole('button', { name: 'Save source' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('source changed');
    expect(input).toHaveValue('Private draft');
    expect(fixture.authority.view().blocks.find((view) => view.block.id === 'raw')?.block.attrs.markdown).toBe('<custom>Agent</custom>');
  } finally { cleanup(); fixture.authority.dispose(); }
});

test('Suggesting mode saves source edits as native proposals and acceptance updates the same block', async () => {
  const fixture = server();
  fixture.authority.edit((draft) => draft.apply([{ op: 'insert_block', block: {
    id: 'raw', kind: 'raw_markdown', parent: null, position: 1, text: '', attrs: { markdown: '<custom>Original</custom>' },
  } }]));
  try {
    render(<DocumentMarkdownEditor documentUrl="/document" config={fixture.collab} author="Reviewer" mode="suggesting" />);
    await userEvent.click(await screen.findByRole('button', { name: 'Edit source' }));
    const input = screen.getByRole('textbox', { name: 'Markdown source' });
    await userEvent.clear(input); await userEvent.type(input, 'Proposed source');
    await userEvent.click(screen.getByRole('button', { name: 'Save source' }));
    await waitFor(() => expect(fixture.authority.view().proposals.some((view) => view.proposal.action.kind === 'update_block')).toBe(true));
    expect(fixture.authority.view().blocks.find((view) => view.block.id === 'raw')?.block.attrs.markdown).toBe('<custom>Original</custom>');
    const suggestion = await screen.findByLabelText('Suggestion by Reviewer');
    await userEvent.click(within(suggestion).getByRole('button', { name: 'Accept suggestion' }));
    await waitFor(() => expect(fixture.authority.view().blocks.find((view) => view.block.id === 'raw')?.block.attrs.markdown).toBe('Proposed source'));
    expect(fixture.authority.view().blocks.filter((view) => view.block.kind === 'raw_markdown')).toHaveLength(1);
  } finally { cleanup(); fixture.authority.dispose(); }
});

test('the application accept-all action uses one durable native batch and preserves review targets', async () => {
  const fixture = server();
  try {
    fixture.authority.edit((draft) => {
      draft.apply([{ op: 'propose_insertion', id: 'second', author: 'Agent', block: 'a', at: draft.model.point('a', 6), text: ' after' }]);
    });
    render(<DocumentMarkdownEditor documentUrl="/document" config={fixture.collab} author="Reviewer" mode="viewing" />);
    await screen.findByText('Keep this text');
    act(() => acceptAllDocumentSuggestions(fixture.collab.documentId));
    await waitFor(() => expect(fixture.authority.view().blocks[0].text).toBe('APPROVED TARGET after'));
    expect(fixture.requests).toHaveLength(1);
    expect(fixture.requests[0].requests.flatMap((request) => request.commands).filter((command) => command.op === 'accept_proposal')).toHaveLength(2);
    expect(fixture.authority.view().comments[0].target.attachments[0].quote).toBe('TARGET');
  } finally { cleanup(); fixture.authority.dispose(); }
});

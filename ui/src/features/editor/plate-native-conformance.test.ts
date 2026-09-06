import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { createSlateEditor, deserializeHtml, type TElement } from 'platejs';
import { toggleCodeBlock } from '@platejs/code-block';
import { toggleList } from '@platejs/list';
import { upsertLink, unwrapLink } from '@platejs/link';
import type { EditorMode } from './editor-types';
import { initSync } from '../../generated/document/quarry_document';
import { DocumentModel, type DocumentBatch, type DocumentView } from './document-model';
import { PlateDocumentAdapter } from './plate-document-adapter';
import { nativeNodeIdOptions, projectPlate, slatePoint } from './plate-document-projection';
import { sourceDrafts } from './source-drafts';
import { applyBlockType, plateMarkdownPlugins, turnIntoList } from './PlateMarkdownEditor';

const root = resolve(process.cwd(), '..');
beforeAll(() => {
  initSync({ module: readFileSync(resolve(process.cwd(), 'src/generated/document/quarry_document_bg.wasm')) });
  execFileSync('cargo', ['build', '--quiet', '--locked', '--offline', '-p', 'quarry-document', '--example', 'document_host'], { cwd: root });
}, 60000);
function native(input: unknown): { bytes: number[]; view: DocumentView } {
  return JSON.parse(execFileSync(resolve(root, process.env.CARGO_TARGET_DIR ?? 'target', 'debug/examples/document_host'), [], { input: JSON.stringify(input), encoding: 'utf8' }));
}
function fixture(markdown = '😀 See TARGET here.\n\nSee TARGET elsewhere.') {
  const model = DocumentModel.fromMarkdown(markdown);
  const bytes = Array.from(model.save());
  const batches: DocumentBatch[] = [], errors: unknown[] = [];
  const editor = createSlateEditor({ plugins: plateMarkdownPlugins as never, nodeId: nativeNodeIdOptions, value: projectPlate(model.view(), true, (point) => model.locate(point)) });
  let mode: EditorMode = 'editing';
  const adapter = new PlateDocumentAdapter(editor, model, { mode: () => mode, author: () => 'Reviewer', changed: (batch) => batches.push(batch), error: (error) => errors.push(error) });
  const select = (id: string, from: number, to = from) => {
    adapter.endIntent();
    const anchor = slatePoint(editor.children, id, from), focus = slatePoint(editor.children, id, to);
    expect(anchor).toBeDefined(); expect(focus).toBeDefined(); editor.tf.select({ anchor: anchor!, focus: focus! });
  };
  const check = () => {
    adapter.flush(); expect(errors.map((error) => error instanceof Error ? error.stack : String(error))).toEqual([]);
    expect(native({ bytes, requests: batches.flatMap((batch) => batch.requests) }).view).toEqual(model.view());
  };
  const comment = (id: string, block: string, from: number, to: number) => {
    select(block, from, to); const draft = adapter.beginComment();
    adapter.command([{ op: 'add_comment', id, author: 'Reviewer', body: 'Keep', ranges: draft.ranges }], draft.base);
  };
  const receive = (remote: DocumentModel) => { adapter.flush(); adapter.rememberSelection(); model.merge(remote.save()); adapter.refresh(); };
  return { model, editor, adapter, batches, errors, select, comment, check, receive, setMode(value: EditorMode) { mode = value; }, close() { adapter.dispose(); model.dispose(); } };
}

test.each(['disc', 'decimal', 'todo'])('converting a %s list item removes list properties and preserves native review', (style) => {
  for (const kind of ['p', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'blockquote']) {
    const t = fixture('Before TARGET after.\n\nSecond item.');
    try {
      const [first, second] = t.model.view().blocks;
      const attrs = { listStyleType: style, indent: 2, ...(style === 'decimal' ? { listStart: 7, listRestart: 7, listRestartPolite: 7 } : {}), ...(style === 'todo' ? { checked: true } : {}), align: 'right' };
      t.adapter.command([{ op: 'set_block', block: first.block.id, kind: 'p', attrs },
        { op: 'set_block', block: second.block.id, kind: 'p', attrs: { listStyleType: style, indent: 2 } }]);
      t.comment('target', first.block.id, 7, 13);
      const before = t.model.view().blocks[0];
      t.select(first.block.id, 0);
      applyBlockType(t.editor, kind); t.check();
      expect(t.model.view().blocks[0]).toMatchObject({ text: first.text, block: { id: first.block.id, kind, attrs: { align: 'right' }, segments: before.block.segments } });
      expect(t.model.view().blocks[0].block.attrs).toEqual({ align: 'right' });
      expect(t.model.view().blocks[1].block.attrs.listStyleType).toBe(style);
      expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
      t.adapter.history(false); t.check();
      expect(t.model.view().blocks[0]).toEqual(before);
      t.adapter.history(true); t.check();
      expect(t.model.view().blocks[0].block.kind).toBe(kind);
      expect(t.model.view().blocks[0].block.attrs).toEqual({ align: 'right' });
    } finally { t.close(); }
  }
});

test.each(['disc', 'decimal', 'todo'])('a suggested %s list conversion is one review decision', (style) => {
  for (const kind of ['h3', 'p', 'blockquote']) {
    const t = fixture('Before TARGET after.');
    try {
      const block = t.model.view().blocks[0].block.id;
      t.adapter.command([{ op: 'set_block', block, kind: 'p', attrs: { listStyleType: style, indent: 2, ...(style === 'todo' ? { checked: true } : {}) } }]);
      t.comment('target', block, 7, 13); t.setMode('suggesting'); t.select(block, 0);
      const before = t.model.view().blocks[0];
      applyBlockType(t.editor, kind); t.check();
      expect(t.model.view().blocks[0]).toEqual(before);
      expect(t.model.view().proposals).toHaveLength(1);
      const proposal = t.model.view().proposals[0].proposal;
      expect(proposal.action).toMatchObject({ kind: 'update_block', block_kind: kind, attrs: {}, expected_attrs: before.block.attrs });
      t.adapter.history(false); t.check();
      expect(t.model.view().proposals.filter((view) => view.proposal.state === 'open')).toHaveLength(0);
      t.adapter.history(true); t.check();
      // A concurrent text edit must not invalidate the block-property decision.
      t.adapter.command([{ op: 'insert_text', at: t.model.point(block, 0), text: 'Agent ' }]);
      t.adapter.command([{ op: 'accept_proposal', id: proposal.id }]); t.check();
      expect(t.model.view().blocks[0]).toMatchObject({ text: 'Agent Before TARGET after.', block: { id: block, kind, attrs: {} } });
      expect(t.model.view().blocks[0].block.attrs).toEqual({});
      expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    } finally { t.close(); }
  }
});

test.each(['editing', 'suggesting'] as const)('a list inside a block proposal converts in %s mode without copying its target', (mode) => {
  const t = fixture('Existing');
  try {
    t.adapter.command([{ op: 'propose_blocks', id: 'proposal', author: 'Agent', parent: null, before: null,
      blocks: [{ id: 'item', kind: 'p', parent: null, position: 0, attrs: { listStyleType: 'todo', indent: 2, checked: true }, text: 'TARGET' }] }]);
    const [node] = t.editor.api.node<TElement>({ at: [], match: (node) => node.quarryProposedBlock === 'item' })!;
    t.comment('target', String(node.id), 0, 6); t.setMode(mode); t.select(String(node.id), 0);
    applyBlockType(t.editor, 'h3'); t.check();
    expect(t.model.view().proposals).toHaveLength(1);
    expect(t.model.view().proposals[0].blocks[0].block).toMatchObject({ id: 'item', kind: 'h3', attrs: {} });
    expect(t.model.view().proposals[0].blocks[0].block.attrs).toEqual({});
    t.adapter.command([{ op: 'accept_proposal', id: 'proposal' }]); t.check();
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ owner: { kind: 'block', id: 'item' }, quote: 'TARGET' });
  } finally { t.close(); }
});

test.each(['disc', 'decimal', 'todo'])('a %s list item converts to code with its native text and comment intact', (style) => {
  const t = fixture('Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.adapter.command([{ op: 'set_block', block, kind: 'p', attrs: { listStyleType: style, indent: 2 } }]);
    t.comment('target', block, 7, 13); t.select(block, 0);
    const before = t.model.view().blocks[0];
    applyBlockType(t.editor, 'code_block'); t.check();
    const line = t.model.view().blocks.find((view) => view.block.id === block)!;
    expect(line.block.kind).toBe('code_line'); expect(line.block.attrs).toEqual({});
    expect(line.block.segments).toEqual(before.block.segments);
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    t.adapter.history(false); t.check(); expect(t.model.view().blocks[0]).toEqual(before);
  } finally { t.close(); }
});

test('changing a list style in Suggesting mode does not first propose leaving the list', () => {
  const t = fixture('- TARGET');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.setMode('suggesting'); t.select(block, 0);
    turnIntoList(t.editor, 'decimal'); t.check();
    expect(t.model.view().proposals).toHaveLength(1);
    expect(t.model.view().proposals[0].proposal.action).toMatchObject({ kind: 'update_block', block_kind: 'p', attrs: { listStyleType: 'decimal' } });
    expect(t.model.view().blocks[0].block.attrs.listStyleType).toBe('disc');
  } finally { t.close(); }
});

test('the native schema still rejects a heading with list membership', () => {
  const model = DocumentModel.fromMarkdown('- TARGET');
  try {
    const before = model.view(), block = before.blocks[0].block;
    expect(() => model.edit((draft) => draft.apply([{ op: 'set_block', block: block.id, kind: 'h3', attrs: block.attrs }]))).toThrow('Only paragraphs can be list items');
    expect(model.view()).toEqual(before);
  } finally { model.dispose(); }
});

test.each(['disc', 'decimal', 'todo'])('heading autoformat and Plate toggle use native conversion for a %s list', (style) => {
  for (const entry of ['autoformat', 'toggle']) {
    const t = fixture('Before TARGET after.');
    try {
      const block = t.model.view().blocks[0].block.id;
      t.adapter.command([{ op: 'set_block', block, kind: 'p', attrs: { listStyleType: style, indent: 2 } }]);
      t.comment('target', block, 7, 13); t.select(block, 0);
      if (entry === 'autoformat') for (const char of '### ') t.editor.tf.insertText(char);
      else t.editor.tf.toggleBlock('h3');
      t.check();
      expect(t.model.view().blocks[0]).toMatchObject({ text: 'Before TARGET after.', block: { id: block, kind: 'h3', attrs: {} } });
      expect(t.model.view().blocks[0].block.attrs).toEqual({});
      expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
      expect(t.batches.flatMap((batch) => batch.requests.flatMap((request) => request.commands)).some((command) => command.op === 'edit' && command.action.op === 'convert_block')).toBe(true);
    } finally { t.close(); }
  }
});

test.each(['editing', 'suggesting'] as const)('code conversion is a native edit in %s mode and keeps agent text through acceptance', (mode) => {
  const t = fixture('- Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.comment('target', block, 7, 13); t.setMode(mode); t.select(block, 0);
    applyBlockType(t.editor, 'code_block'); t.check();
    if (mode === 'suggesting') {
      expect(t.model.view().blocks[0].block.kind).toBe('p');
      expect(t.model.view().proposals).toHaveLength(1);
      t.adapter.command([{ op: 'insert_text', at: t.model.point(block, 0), text: 'Agent ' }]);
      t.adapter.command([{ op: 'accept_proposal', id: t.model.view().proposals[0].proposal.id }]); t.check();
    }
    expect(t.model.view().blocks.find((view) => view.block.id === block)?.block.kind).toBe('code_line');
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    t.setMode('editing'); t.select(block, 0); applyBlockType(t.editor, 'h3'); t.check();
    expect(t.model.view().blocks[0].block.kind).toBe('h3');
    expect(t.model.view().blocks[0].block.id).toBe(block);
  } finally { t.close(); }
});

test.each(['editing', 'suggesting'] as const)('converting the empty input creates one native block in %s mode', (mode) => {
  for (const kind of ['h3', 'code_block']) {
    const t = fixture('');
    try {
      t.setMode(mode); t.editor.tf.select({ path: [0, 0], offset: 0 });
      applyBlockType(t.editor, kind); t.check();
      if (mode === 'suggesting') {
        expect(t.model.view().proposals).toHaveLength(1);
        t.adapter.command([{ op: 'accept_proposal', id: t.model.view().proposals[0].proposal.id }]); t.check();
      }
      expect(t.model.view().blocks[0].block.kind).toBe(kind);
      expect(t.model.view().blocks).toHaveLength(kind === 'code_block' ? 2 : 1);
    } finally { t.close(); }
  }
});

test.each(['backward', 'forward'] as const)('adjacent %s deletions form one native proposal and undo group', (direction) => {
  const t = fixture('Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.comment('target', block, 7, 13);
    t.setMode('suggesting'); t.select(block, direction === 'backward' ? 13 : 7);
    for (let i = 0; i < 6; i++) {
      if (direction === 'backward') t.editor.tf.deleteBackward('character');
      else t.editor.tf.deleteForward('character');
      t.check();
    }
    const proposals = t.model.view().proposals.filter((view) => view.proposal.state === 'open');
    expect(proposals).toHaveLength(1);
    expect(proposals[0].proposal.action.original_quote).toBe('TARGET');
    expect(proposals[0].text).toBe('');
    expect(t.model.view().blocks[0].text).toBe('Before TARGET after.');
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    const id = proposals[0].proposal.id;
    t.adapter.history(false); t.check();
    expect(t.model.view().proposals.filter((view) => view.proposal.state === 'open')).toHaveLength(0);
    t.adapter.history(true); t.check();
    expect(t.model.view().proposals.find((view) => view.proposal.id === id)?.proposal.action.original_quote).toBe('TARGET');
    t.adapter.command([{ op: 'accept_proposal', id }]); t.check();
    expect(t.model.view().blocks[0].text).toBe('Before  after.');
    t.adapter.history(false); t.check();
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
  } finally { t.close(); }
});

test('deleting a selection then backspacing extends the same deletion without a direct canonical edit', () => {
  const t = fixture('Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.setMode('suggesting'); t.select(block, 10, 13);
    t.editor.tf.deleteFragment(); t.check();
    const id = t.model.view().proposals[0].proposal.id;
    for (let i = 0; i < 3; i++) { t.editor.tf.deleteBackward('character'); t.check(); }
    expect(t.model.view().proposals.filter((view) => view.proposal.state === 'open')).toHaveLength(1);
    expect(t.model.view().proposals[0].proposal).toMatchObject({ id, action: { original_quote: 'TARGET' } });
  } finally { t.close(); }
});

test('typing after a cross-block selection deletion continues one native replacement', () => {
  const t = fixture('Before TARGET\n\nSECOND after.');
  try {
    const [first, second] = t.model.view().blocks;
    t.comment('first', first.block.id, 7, 13);
    t.comment('second', second.block.id, 0, 6);
    t.setMode('suggesting'); t.adapter.endIntent();
    t.editor.tf.select({ anchor: slatePoint(t.editor.children, first.block.id, 7)!, focus: slatePoint(t.editor.children, second.block.id, 6)! });
    t.editor.tf.deleteFragment(); t.check();
    const id = t.model.view().proposals[0].proposal.id;
    t.editor.tf.insertText('😀New'); t.check();
    expect(t.model.view().proposals).toHaveLength(1);
    expect(t.model.view().proposals[0]).toMatchObject({ proposal: { id, action: { original_quote: 'TARGETSECOND' } }, text: '😀New' });
    expect(t.model.view().blocks.map((block) => block.text)).toEqual(['Before TARGET', 'SECOND after.']);
    expect(t.model.view().comments.map((view) => view.target.attachments[0].quote)).toEqual(['TARGET', 'SECOND']);
    t.adapter.command([{ op: 'accept_proposal', id }]); t.check();
    expect(t.model.view().blocks.map((block) => block.text)).toEqual(['Before 😀New', ' after.']);
    t.adapter.history(false); t.check();
    expect(t.model.view().comments.map((view) => view.target.attachments[0].quote)).toEqual(['TARGET', 'SECOND']);
  } finally { t.close(); }
});

test('deletion intents respect authors and gaps, and a native reload starts a fresh intent', () => {
  const t = fixture('Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.adapter.command([{ op: 'propose_replacement', id: 'other', author: 'Agent', block,
      at: t.model.point(block, 10), ranges: t.model.selection(block, 10, 13), text: '' }]);
    t.setMode('suggesting'); t.select(block, 10);
    t.editor.tf.deleteBackward('character'); t.check();
    const own = t.model.view().proposals.find((view) => view.proposal.author === 'Reviewer')!;
    expect(own.proposal.action.original_quote).toBe('R');
    expect(t.model.view().proposals.find((view) => view.proposal.id === 'other')?.proposal.action.original_quote).toBe('GET');
    t.select(block, 3); t.editor.tf.deleteBackward('character'); t.check();
    expect(t.model.view().proposals.filter((view) => view.proposal.state === 'open')).toHaveLength(3);
    const loaded = new DocumentModel(t.model.save());
    const errors: unknown[] = [];
    const editor = createSlateEditor({ plugins: plateMarkdownPlugins as never, nodeId: nativeNodeIdOptions, value: projectPlate(loaded.view(), true, (point) => loaded.locate(point)) });
    const adapter = new PlateDocumentAdapter(editor, loaded, { mode: () => 'suggesting', author: () => 'Reviewer', changed: () => {}, error: (error) => errors.push(error) });
    try {
      editor.tf.select(slatePoint(editor.children, block, 9)!);
      editor.tf.deleteBackward('character'); adapter.flush();
      expect(errors).toEqual([]);
      expect(loaded.view().proposals.find((view) => view.proposal.id === own.proposal.id)?.proposal.action.original_quote).toBe('R');
      expect(loaded.view().proposals.filter((view) => view.proposal.state === 'open')).toHaveLength(4);
    } finally { adapter.dispose(); loaded.dispose(); }
  } finally { t.close(); }
});

test.each(['backward', 'forward'] as const)('deletion at an inserted proposal boundary stays a native proposal (%s)', (direction) => {
  const t = fixture('Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.setMode('suggesting'); t.select(block, 7); t.editor.tf.insertText('NEW'); t.check();
    const id = t.model.view().proposals[0].proposal.id;
    t.adapter.endIntent(); t.editor.tf.select(slatePoint(t.editor.children, id, direction === 'backward' ? 0 : 3, true)!);
    if (direction === 'backward') t.editor.tf.deleteBackward('character'); else t.editor.tf.deleteForward('character');
    t.check();
    expect(t.model.view().blocks[0].text).toBe('Before TARGET after.');
    expect(t.model.view().proposals.find((view) => view.proposal.id === id)?.text).toBe('NEW');
    expect(t.model.view().proposals.find((view) => view.proposal.id !== id)?.proposal.action.original_quote).toBe(direction === 'backward' ? ' ' : 'T');
  } finally { t.close(); }
});

test('explicit selection ends continuation even when the caret position does not change', () => {
  const t = fixture('Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.setMode('suggesting'); t.select(block, 13);
    t.editor.tf.deleteBackward('character'); t.check();
    const first = t.model.view().proposals[0].proposal.id;
    t.select(block, 12);
    t.editor.tf.deleteBackward('character'); t.check();
    expect(t.model.view().proposals).toHaveLength(2);
    expect(t.model.view().proposals.find((v) => v.proposal.id === first)?.proposal.action.original_quote).toBe('T');
    expect(t.model.view().proposals.find((v) => v.proposal.id !== first)?.proposal.action.original_quote).toBe('E');
  } finally { t.close(); }
});

test('a repeated selection synchronization keeps the active input intent', () => {
  const t = fixture('Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.setMode('suggesting'); t.select(block, 13);
    t.editor.tf.deleteBackward('character'); t.check();
    // Slate flushes its throttled DOM selection before the next input. This
    // carries no new pointer or navigation action from the user.
    t.editor.tf.select(t.editor.selection!);
    t.editor.tf.deleteBackward('character'); t.check();
    expect(t.model.view().proposals).toHaveLength(1);
    expect(t.model.view().proposals[0].proposal.action.original_quote).toBe('ET');
  } finally { t.close(); }
});

test('suggestion identity survives an undo time boundary and uses independent requests', () => {
  const t = fixture('Before TARGET after.');
  const clock = vi.spyOn(Date, 'now'); let now = 10000; clock.mockImplementation(() => now);
  try {
    const block = t.model.view().blocks[0].block.id;
    t.setMode('suggesting'); t.select(block, 13);
    t.editor.tf.deleteBackward('character'); t.check();
    const id = t.model.view().proposals[0].proposal.id;
    now += 1000;
    t.editor.tf.deleteBackward('character'); t.check();
    expect(t.model.view().proposals).toHaveLength(1);
    expect(t.model.view().proposals[0].proposal.action.original_quote).toBe('ET');
    expect(new Set(t.batches.flatMap((batch) => batch.requests.map((request) => request.request_id))).size).toBe(2);
    t.adapter.history(false); t.check();
    expect(t.model.view().proposals[0].proposal.id).toBe(id);
    expect(t.model.view().proposals[0].proposal.action.original_quote).toBe('T');
    t.editor.tf.deleteBackward('character'); t.check();
    expect(t.model.view().proposals).toHaveLength(2);
  } finally { clock.mockRestore(); t.close(); }
});

test('low-level browser text operations use the same native intent as selection deletion', () => {
  const t = fixture('Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.setMode('suggesting'); t.select(block, 13);
    for (let index = 12; index >= 7; index--) {
      const point = slatePoint(t.editor.children, block, index)!;
      t.editor.tf.apply({ type: 'remove_text', path: point.path, offset: point.offset, text: 'Before TARGET after.'[index] });
      t.check();
    }
    expect(t.model.view().proposals).toHaveLength(1);
    expect(t.model.view().proposals[0].proposal.action.original_quote).toBe('TARGET');
    expect(t.model.view().blocks[0].text).toBe('Before TARGET after.');
  } finally { t.close(); }
});

test('deleting a selection then typing creates one replacement with native proposed-text edits', () => {
  const t = fixture('Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.setMode('suggesting'); t.select(block, 7, 13);
    t.editor.tf.deleteFragment(); t.check();
    const id = t.model.view().proposals[0].proposal.id;
    t.editor.tf.insertText('New'); t.check();
    t.editor.tf.insertText(' text'); t.check();
    expect(t.model.view().proposals).toHaveLength(1);
    expect(t.model.view().proposals[0]).toMatchObject({ proposal: { id, action: { original_quote: 'TARGET' } }, text: 'New text' });
    t.adapter.command([{ op: 'accept_proposal', id }]); t.check();
    expect(t.model.view().blocks[0].text).toBe('Before New text after.');
  } finally { t.close(); }
});

test('word deletion and block removal use native suggestions without changing canonical owners', () => {
  const t = fixture('Before TARGET after.\n\nKeep this block.');
  try {
    const [first, second] = t.model.view().blocks.map((v) => v.block.id);
    t.comment('block-comment', second, 0, 4);
    t.setMode('suggesting'); t.select(first, 13);
    t.editor.tf.deleteBackward('word'); t.check();
    expect(t.model.view().proposals[0].proposal.action.original_quote).toBe('TARGET');
    const entry = t.editor.api.node({ at: [], match: (node) => node.id === second })!;
    t.editor.tf.removeNodes({ at: entry[1] }); t.check();
    const proposal = t.model.view().proposals.find((v) => v.proposal.action.kind === 'delete_block')!;
    expect(proposal.proposal.action.block).toBe(second);
    expect(t.model.view().blocks).toHaveLength(2);
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('Keep');
    t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
    expect(t.model.view().blocks).toHaveLength(1);
    t.adapter.history(false); t.check();
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('Keep');
  } finally { t.close(); }
});

test('Plate requests replay identically in Rust, including newly typed text, marks and a split', () => {
  const t = fixture();
  try {
    t.editor.tf.insertNodes({ type: 'p', children: [{ text: 'TARGET' }] }, { at: [2], select: true }); t.adapter.flush();
    const block = t.model.view().blocks[2].block.id;
    t.select(block, 3); t.editor.tf.insertText('!'); t.adapter.flush();
    t.comment('new', block, 0, 7);
    t.select(block, 0, 7); t.editor.tf.addMark('bold', true); t.adapter.flush();
    t.select(block, 3); t.editor.tf.insertBreak(); t.check();
    expect(t.model.view().comments[0].target.attachments.map((part) => part.quote).join('')).toBe('TAR!GET');
  } finally { t.close(); }
});

test('cross-paragraph replacement transfers the surviving characters and undo restores their ownership', () => {
  const t = fixture();
  try {
    const [a, b] = t.model.view().blocks.map((view) => view.block.id);
    t.comment('surviving', b, 4, 10);
    t.editor.tf.select({ anchor: slatePoint(t.editor.children, a, 3)!, focus: slatePoint(t.editor.children, b, 4)! });
    t.editor.tf.insertText('Replacement '); t.check();
    expect(t.model.view().blocks).toHaveLength(1);
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ owner: { id: a }, quote: 'TARGET' });
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks).toHaveLength(2);
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ owner: { id: b }, quote: 'TARGET' });
    t.adapter.history(true); t.check();
  } finally { t.close(); }
});

test('Suggesting replaces an exact selection across a table without changing canonical text before acceptance', () => {
  const t = fixture('Before TARGET\n\n| Head |\n| --- |\n| Before TARGET |');
  try {
    const initial = t.model.view().blocks;
    const paragraph = initial[0], cell = initial.find((view) => view.text === 'Before TARGET' && view.block.parent !== null)!;
    t.comment('surviving', cell.block.id, 7, 13);
    const canonical = t.model.view().blocks;
    t.setMode('suggesting');
    t.editor.tf.select({ anchor: slatePoint(t.editor.children, paragraph.block.id, 7)!, focus: slatePoint(t.editor.children, cell.block.id, 7)! });
    t.editor.tf.insertText('NEW'); t.check();
    expect(t.model.view().blocks).toEqual(canonical);
    const proposal = t.model.view().proposals[0];
    expect(proposal.text).toBe('NEW');
    expect(proposal.acceptance_error).toBeNull();
    t.adapter.command([{ op: 'add_comment', id: 'proposed', author: 'Reader', body: 'Keep new text', ranges: t.model.selectionFor({ kind: 'proposal', id: proposal.proposal.id }, 0, 3) }]);
    t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
    expect(t.model.view().blocks[0].text).toBe('Before NEW');
    expect(t.model.view().blocks.find((view) => view.block.id === cell.block.id)!.text).toBe('TARGET');
    expect(t.model.view().blocks.map((view) => view.block.kind)).toEqual(initial.map((view) => view.block.kind));
    expect(t.model.view().comments.find((view) => view.comment.id === 'surviving')!.target.attachments[0]).toMatchObject({ quote: 'TARGET', owner: { id: cell.block.id } });
    expect(t.model.view().comments.find((view) => view.comment.id === 'proposed')!.target.attachments[0]).toMatchObject({ quote: 'NEW', owner: { id: paragraph.block.id } });
    t.adapter.history(false); t.check();
    const content = (blocks: DocumentView['blocks']) => blocks.map(({ block, text }) => ({ id: block.id, kind: block.kind, parent: block.parent, text }));
    expect(content(t.model.view().blocks)).toEqual(content(canonical));
    expect(t.model.view().comments.find((view) => view.comment.id === 'surviving')!.target.attachments[0]).toMatchObject({ quote: 'TARGET', owner: { id: cell.block.id } });
  } finally { t.close(); }
});

test('Suggesting in an empty document keeps the input scaffold private and proposes a complete paragraph', () => {
  const t = fixture('');
  try {
    expect(t.model.view().blocks).toEqual([]);
    t.setMode('suggesting'); t.editor.tf.select({ path: [0, 0], offset: 0 });
    t.editor.tf.insertText('New'); t.check();
    expect(t.model.view().blocks).toEqual([]);
    const proposal = t.model.view().proposals[0];
    expect(proposal.proposal.action.kind).toBe('insert_blocks');
    expect(proposal.blocks).toHaveLength(1);
    expect(proposal.blocks[0].text).toBe('New');
    t.editor.tf.insertText(' text'); t.check();
    expect(t.model.view().proposals[0].text).toBe('New text');
    t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
    expect(t.model.view().blocks.map((view) => view.text)).toEqual(['New text']);
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks).toEqual([]);
  } finally { t.close(); }
});

for (const kind of ['paragraphs', 'list', 'code', 'proposed paragraphs', 'proposed code']) {
for (const action of ['delete', 'type', 'paste'] as const) test(`replacing three ${kind} with ${action} joins the surviving native text`, () => {
  const code = kind.includes('code'), proposed = kind.startsWith('proposed');
  const markdown = code ? '```\nKeep FIRST\nRemove middle\nLAST TARGET\n```' : kind === 'list' ? '- Keep FIRST\n- Remove middle\n- LAST TARGET' : 'Keep FIRST\n\nRemove middle\n\nLAST TARGET';
  const t = fixture(proposed ? 'Canonical' : markdown);
  try {
    if (proposed) {
      t.setMode('suggesting');
      const children = ['Keep FIRST', 'Remove middle', 'LAST TARGET'].map((text) => ({ type: code ? 'code_line' : 'p', children: [{ text }] }));
      t.editor.tf.insertNodes<TElement>(code ? { type: 'code_block', children } : children, { at: [1] }); t.check();
      t.setMode('editing');
    }
    const views = () => (proposed ? t.model.view().proposals[0].blocks : t.model.view().blocks).filter((view) => view.block.kind !== 'code_block');
    const [first, middle, last] = views();
    const slateId = (id: string) => proposed ? String(t.editor.api.node<TElement>({ at: [], match: (node) => node.quarryProposedBlock === id })![0].id) : id;
    t.comment('tail', slateId(last.block.id), 5, 11);
    t.comment('removed', slateId(middle.block.id), 0, middle.text.length);
    t.editor.tf.select({ anchor: slatePoint(t.editor.children, slateId(first.block.id), 5)!, focus: slatePoint(t.editor.children, slateId(last.block.id), 5)! });
    if (action === 'delete') t.editor.tf.deleteFragment();
    else if (action === 'type') t.editor.tf.insertText('New ');
    else t.editor.tf.insertFragment([{ type: code ? 'code_line' : 'p', children: [{ text: 'New ' }] }]);
    t.check();
    expect(views().map((view) => view.text)).toEqual([action === 'delete' ? 'Keep TARGET' : 'Keep New TARGET']);
    expect(t.model.view().comments.find((view) => view.comment.id === 'tail')!.target.attachments[0]).toMatchObject({ quote: 'TARGET', owner: { id: proposed ? t.model.view().proposals[0].proposal.id : first.block.id } });
    expect(t.model.view().comments.find((view) => view.comment.id === 'removed')!.target.state).toBe('hidden');
    t.adapter.history(false); t.check();
    expect(views().map((view) => view.text)).toEqual(['Keep FIRST', 'Remove middle', 'LAST TARGET']);
    expect(t.model.view().comments.find((view) => view.comment.id === 'removed')!.target.state).toBe('attached');
    t.adapter.history(true); t.check();
    if (proposed) {
      expect(t.model.view().blocks.map((view) => view.text)).toEqual(['Canonical']);
      t.adapter.command([{ op: 'accept_proposal', id: t.model.view().proposals[0].proposal.id }]); t.check();
      expect(t.model.view().comments.find((view) => view.comment.id === 'tail')!.target.attachments[0]).toMatchObject({ quote: 'TARGET', owner: { id: first.block.id } });
    }
  } finally { t.close(); }
});
}

for (const count of [2, 3, 5]) for (const backward of [false, true]) test(`deleting a ${backward ? 'backward' : 'forward'} selection over ${count} blocks keeps Unicode and bold text`, () => {
  const t = fixture(['😀 Start', ...Array<string>(count - 2).fill('Middle'), 'End **TARGET**'].join('\n\n'));
  try {
    const initial = t.model.view().blocks;
    t.comment('tail', initial.at(-1)!.block.id, 4, 10);
    const anchor = slatePoint(t.editor.children, initial[0].block.id, 3)!, focus = slatePoint(t.editor.children, initial.at(-1)!.block.id, 4)!;
    t.editor.tf.select(backward ? { anchor: focus, focus: anchor } : { anchor, focus });
    t.editor.tf.deleteFragment(); t.check();
    expect(t.model.view().blocks[0].text).toBe('😀 TARGET');
    expect(t.model.view().blocks[0].runs.find((run) => run.text === 'TARGET')!.marks.bold).toBe(true);
    expect(t.editor.selection!.anchor).toEqual(slatePoint(t.editor.children, initial[0].block.id, 3));
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks.map((view) => view.text)).toEqual(initial.map((view) => view.text));
  } finally { t.close(); }
});

test('moving then joining blocks within one input keeps native operation order', () => {
  const t = fixture('First\n\nMiddle\n\nTARGET');
  try {
    const last = t.model.view().blocks[2]; t.comment('tail', last.block.id, 0, 6);
    t.editor.tf.withoutNormalizing(() => {
      t.editor.tf.moveNodes({ at: [2], to: [1] });
      t.editor.tf.mergeNodes({ at: [1] });
    }); t.check();
    expect(t.model.view().blocks.map((view) => view.text)).toEqual(['FirstTARGET', 'Middle']);
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
  } finally { t.close(); }
});

test('a second browser projects a delayed deletion after a concurrent edit in the absorbed source', () => {
  const t = fixture('Keep FIRST\n\nRemove middle\n\nLAST TARGET');
  const authority = new DocumentModel(t.model.save()), reader = new DocumentModel(t.model.save());
  try {
    const [first, , last] = t.model.view().blocks;
    let base = authority.heads();
    reader.view();
    authority.edit((draft) => draft.apply([{ op: 'insert_text', at: authority.point(last.block.id, 8), text: '!' }]));
    reader.merge(authority.changesSince(base), base, authority.heads()); reader.view();
    t.editor.tf.select({ anchor: slatePoint(t.editor.children, first.block.id, 5)!, focus: slatePoint(t.editor.children, last.block.id, 5)! });
    t.editor.tf.deleteFragment(); t.check();
    base = authority.heads();
    authority.applyBatch(t.batches.at(-1)!);
    reader.merge(authority.changesSince(base), base, authority.heads());
    expect(authority.view().blocks.map((view) => view.text)).toEqual(['Keep TAR!GET']);
    expect(reader.view()).toEqual(authority.view());
  } finally { t.close(); authority.dispose(); reader.dispose(); }
});

test('multiline paste splits existing text without replacing its comment identity', () => {
  const t = fixture();
  try {
    const block = t.model.view().blocks[0].block.id;
    t.comment('paste', block, 7, 13); t.select(block, 7);
    t.editor.tf.insertFragment([{ type: 'p', children: [{ text: 'Intro' }] }, { type: 'p', children: [{ text: 'New paragraph: ' }] }]); t.check();
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    expect(t.model.view().comments[0].target.attachments[0].owner.id).not.toBe(block);
  } finally { t.close(); }
});

test('a comment on a selected wiki chip targets its full native syntax and survives formatting', () => {
  const source = 'See [[Other#Part|label]] here. [[Other#Part|label]]';
  const t = fixture(source);
  try {
    const block = t.model.view().blocks[0].block.id;
    const syntax = '[[Other#Part|label]]';
    t.comment('wiki', block, 4, 4 + syntax.length); t.check();
    expect(t.model.view().comments[0].target.attachments.map((part) => part.quote).join('')).toBe(syntax);
    t.adapter.command([{ op: 'format', ranges: t.model.selection(block, 4 + syntax.indexOf('label'), 4 + syntax.indexOf('label') + 5), name: 'bold', value: true }]);
    expect(t.editor.children[0].children.filter((node) => node.type === 'wikilink')).toHaveLength(2);
    expect(t.editor.children[0].children.find((node) => node.type === 'wikilink')!.quarryReviewKey).toContain('wiki');
    t.select(block, 4, 4 + syntax.length); t.editor.tf.addMark('italic', true); t.check();
    expect(t.model.view().blocks[0].runs.filter((run) => run.marks.wikilink && run.marks.italic)).not.toHaveLength(0);
    t.select(block, 0); t.editor.tf.insertText('New '); t.check();
    expect(t.model.view().blocks[0].runs.find((run) => run.text === 'label')!.marks.bold).toBe(true);
    expect(t.model.view().comments[0].target.attachments.map((part) => part.quote).join('')).toBe(syntax);
    expect(t.model.view().comments[0].target.attachments[0].start).toBe(8);
    expect(t.model.markdown()).toBe(`New See *${syntax}* here. ${syntax}\n`);
    const restored = DocumentModel.fromArchive(t.model.archive());
    try { expect(restored.view()).toEqual(t.model.view()); } finally { restored.dispose(); }
  } finally { t.close(); }
});

test('rich HTML paste retains formatting and structure, mints IDs and leaves existing review targets intact', () => {
  const t = fixture();
  try {
    const block = t.model.view().blocks[0].block.id;
    t.comment('paste', block, 7, 13); t.select(block, 7);
    const fragment = deserializeHtml(t.editor, { element: `<h2 data-block-id="${block}">Pasted 😀</h2><p><strong>Bold</strong> <a href="https://example.com">link</a></p><table><tr><th>Header</th></tr><tr><td><em>Cell</em></td></tr></table><pre>line 1\nline 2</pre>` });
    t.editor.tf.insertFragment(fragment); t.check();
    const view = t.model.view();
    expect(view.blocks.filter((view) => view.block.id === block)).toHaveLength(1);
    expect(view.blocks.filter((view) => view.block.kind === 'table')).toHaveLength(1);
    expect(view.blocks.filter((view) => view.block.kind === 'code_line').map((view) => view.text)).toEqual(['line 1', 'line 2']);
    expect(view.blocks.find((view) => view.text === 'Bold link')!.runs).toEqual(expect.arrayContaining([
      expect.objectContaining({ text: 'Bold', marks: { bold: true } }), expect.objectContaining({ text: 'link', marks: { link: 'https://example.com' } }),
    ]));
    expect(view.comments[0].target.attachments[0].quote).toBe('TARGET');
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks.map((view) => view.text)).toEqual(['😀 See TARGET here.', 'See TARGET elsewhere.']);
  } finally { t.close(); }
});

test('a delayed comment draft still targets its original characters after a remote boundary insertion', () => {
  const t = fixture();
  const remote = new DocumentModel(t.model.save());
  try {
    const block = t.model.view().blocks[0].block.id;
    t.select(block, 7, 13); const draft = t.adapter.beginComment();
    t.batches.push(remote.edit((draft) => draft.apply([{ op: 'insert_text', at: remote.point(block, 13), text: '!' }])));
    t.receive(remote);
    t.adapter.command([{ op: 'add_comment', id: 'late', author: 'Reviewer', body: 'Original', ranges: draft.ranges }], draft.base); t.check();
    expect(t.model.view().comments[0].target.attachments.map((part) => part.quote).join('')).toBe('TARGET');
  } finally { remote.dispose(); t.close(); }
});

test('formatting across blocks can undo after a late comment without hiding the comment', () => {
  const t = fixture();
  const remote = new DocumentModel(t.model.save());
  try {
    const [a, b] = t.model.view().blocks;
    t.editor.tf.select({ anchor: slatePoint(t.editor.children, a.block.id, 0)!, focus: slatePoint(t.editor.children, b.block.id, b.text.length)! });
    t.editor.tf.addMark('bold', true); t.check();
    t.batches.push(remote.edit((draft) => draft.apply([{ op: 'add_comment', id: 'late', author: 'Agent', body: 'Keep', ranges: remote.selection(a.block.id, 7, 13) }])));
    t.receive(remote); t.adapter.history(false); t.check();
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    expect(t.model.view().blocks.every((view) => view.runs.every((run) => !run.marks.bold))).toBe(true);
  } finally { remote.dispose(); t.close(); }
});

test('typing undo groups adjacent keys and keeps an intervening agent edit', () => {
  const t = fixture();
  try {
    const [a, b] = t.model.view().blocks;
    t.select(a.block.id, 0);
    for (const text of 'One') { t.editor.tf.insertText(text); t.adapter.flush(); }
    const remote = new DocumentModel(t.model.save());
    try {
      t.batches.push(remote.edit((draft) => draft.apply([{ op: 'insert_text', at: remote.point(b.block.id, 0), text: 'Agent ' }])));
      t.receive(remote);
    } finally { remote.dispose(); }
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks.map((view) => view.text)).toEqual([a.text, 'Agent ' + b.text]);
  } finally { t.close(); }
});

test('recovery can replace an unsaved text owner while the caret is in that owner', () => {
  const t = fixture();
  const saved = t.model.save();
  try {
    t.editor.tf.insertNodes({ type: 'p', children: [{ text: 'Unsaved' }] }, { at: [2], select: true }); t.adapter.flush();
    t.adapter.rememberSelection(); t.model.useSavedVersion(saved);
    expect(() => t.adapter.refresh()).not.toThrow();
    expect(t.editor.children).toEqual(projectPlate(t.model.view()));
    expect(t.model.view().blocks).toHaveLength(2);
  } finally { t.close(); }
});

test('block conversion retains comments, while duplication creates new native text identity', () => {
  const t = fixture();
  try {
    const block = t.model.view().blocks[0].block.id;
    t.comment('original', block, 7, 13); t.select(block, 0); toggleCodeBlock(t.editor); t.check();
    const line = t.editor.api.node<TElement>({ at: [], match: (node) => node.id === block })!;
    t.editor.tf.insertNodes(structuredClone(line[0]), { at: [...line[1].slice(0, -1), line[1].at(-1)! + 1] }); t.check();
    const lines = t.model.view().blocks.filter((view) => view.block.kind === 'code_line');
    expect(lines).toHaveLength(2); expect(lines[0].text).toBe(lines[1].text);
    expect(lines[0].block.segments).not.toEqual(lines[1].block.segments);
    expect(t.model.view().comments[0].target.attachments).toHaveLength(1);
    t.editor.tf.removeNodes({ at: line[1] }); t.check();
    expect(t.model.view().comments[0].target.state).toBe('hidden');
    t.adapter.history(false); t.check();
    expect(t.model.view().comments[0].target.state).toBe('attached');
  } finally { t.close(); }
});

test.each([['# ', 'h1', undefined], ['> ', 'blockquote', undefined], ['- ', 'p', 'disc'], ['3. ', 'p', 'decimal'], ['[x] ', 'p', 'todo']])('Plate Markdown shortcut %s preserves the following comment target', (shortcut, kind, list) => {
  const t = fixture('TARGET');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.comment('shortcut', block, 0, 6); t.select(block, 0);
    for (const key of shortcut!) { t.editor.tf.insertText(key); t.adapter.flush(); }
    t.check();
    expect(t.model.view().blocks[0].block.kind).toBe(kind);
    expect(t.model.view().blocks[0].block.attrs.listStyleType).toBe(list);
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
  } finally { t.close(); }
});

test('an agent block ID cannot collide with the virtual input surface', () => {
  const t = fixture('---');
  try {
    const id = `input:${t.model.view().document_id}`;
    t.adapter.command([{ op: 'insert_block', block: { id, kind: 'p', parent: null, position: 0, attrs: {}, text: 'TARGET' } }]);
    t.select(id, 3); t.editor.tf.insertText('!'); t.check();
    expect(t.model.view().blocks.find((view) => view.block.id === id)?.text).toBe('TAR!GET');
  } finally { t.close(); }
});

test('native archives retain hidden review text and history and reject invalid bytes', () => {
  const t = fixture('See {==TARGET==}{>>Keep<<}{#c} and {~~old~>new~~}{#s}.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.adapter.command([{ op: 'delete_text', ranges: t.model.selection(block, 4, 10) }]);
    const restored = DocumentModel.fromArchive(t.model.archive());
    try {
      expect(restored.view()).toEqual(t.model.view());
      expect(restored.view().comments[0].target.state).toBe('deleted');
      expect(native({ bytes: Array.from(restored.save()) }).view).toEqual(t.model.view());
    } finally { restored.dispose(); }
    expect(() => DocumentModel.fromArchive('{"format":"quarry-document","version":1,"bytes":[999]}')).toThrow();
    expect(() => DocumentModel.fromArchive('{"format":"quarry-document","version":2,"bytes":[]}')).toThrow();
  } finally { t.close(); }
});


test('structured proposals edit exact block boundaries and keep a delayed comment through acceptance and undo', () => {
  const t = fixture();
  try {
    const before = t.model.view().blocks[1].block.id;
    t.adapter.command([{ op: 'propose_blocks', id: 'structure', author: 'Agent', parent: null, before, blocks: [
      { id: 'heading', kind: 'h2', parent: null, position: 0, attrs: {}, text: 'Heading' },
      { id: 'paragraph', kind: 'p', parent: null, position: 1, attrs: {}, text: 'TARGET' },
    ] }]); t.check();
    const proposed = (id: string) => t.editor.children.find((node) => node.quarryProposedBlock === id)!;
    expect(proposed('heading').type).toBe('h2');
    t.select(String(proposed('heading').id), 7); t.editor.tf.insertText('!'); t.check();
    expect(t.model.view().proposals[0].blocks.map((view) => view.text)).toEqual(['Heading!', 'TARGET']);
    t.select(String(proposed('paragraph').id), 0); t.adapter.rememberSelection();
    const remote = new DocumentModel(t.model.save());
    try {
      t.batches.push(remote.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.proposedBlockPoint('structure', 'heading', 0), text: 'Agent ' }])));
      t.receive(remote);
    } finally { remote.dispose(); }
    expect(t.editor.selection!.anchor).toEqual(slatePoint(t.editor.children, String(proposed('paragraph').id), 0));
    t.editor.tf.insertText('Here '); t.check();
    expect(t.model.view().proposals[0].blocks.map((view) => view.text)).toEqual(['Agent Heading!', 'Here TARGET']);
    t.select(String(proposed('paragraph').id), 5, 11); const draft = t.adapter.beginComment();
    t.adapter.command([{ op: 'accept_proposal', id: 'structure' }]); t.check();
    t.adapter.command([{ op: 'add_comment', id: 'delayed', author: 'Reviewer', body: 'Keep', ranges: draft.ranges }], draft.base); t.check();
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ owner: { kind: 'block', id: 'paragraph' }, quote: 'TARGET' });
    t.adapter.history(false); t.adapter.history(false); t.check();
    expect(t.model.view().blocks).toHaveLength(2);
    expect(proposed('heading')).toBeDefined();
  } finally { t.close(); }
});

test('proposed tables keep per-cell text, marks and comment ownership while canonical blocks are reordered', () => {
  const t = fixture();
  try {
    const canonical = t.model.view().blocks.map((view) => view.block.id);
    t.adapter.command([{ op: 'propose_blocks', id: 'table-proposal', author: 'Agent', parent: null, before: canonical[1], blocks: [
      { id: 'table', kind: 'table', parent: null, position: 0, attrs: {}, text: '' },
      { id: 'row', kind: 'tr', parent: 'table', position: 0, attrs: {}, text: '' },
      { id: 'a-cell', kind: 'td', parent: 'row', position: 0, attrs: {}, text: '' },
      { id: 'b-cell', kind: 'td', parent: 'row', position: 1, attrs: {}, text: '' },
      // Source order is independent of visual order.
      { id: 'b', kind: 'p', parent: 'b-cell', position: 0, attrs: {}, text: 'TARGET' },
      { id: 'a', kind: 'p', parent: 'a-cell', position: 0, attrs: {}, text: 'Left' },
    ] }]); t.check();
    const a = t.editor.api.node<TElement>({ at: [], match: (node) => node.quarryProposedBlock === 'a' })!;
    const b = t.editor.api.node<TElement>({ at: [], match: (node) => node.quarryProposedBlock === 'b' })!;
    t.select(String(a[0].id), 4); t.editor.tf.insertText('!'); t.check();
    t.select(String(b[0].id), 0, 6); t.editor.tf.addMark('bold', true); t.check();
    t.comment('cell', String(b[0].id), 0, 6);
    expect(t.model.view().proposals[0].blocks.find((view) => view.block.id === 'a')!.text).toBe('Left!');
    expect(t.model.view().proposals[0].blocks.find((view) => view.block.id === 'b')!.runs[0].marks.bold).toBe(true);
    t.editor.tf.moveNodes({ at: [2], to: [0] }); t.check();
    expect(t.model.view().blocks.map((view) => view.block.id)).toEqual([...canonical].reverse());
    t.adapter.command([{ op: 'accept_proposal', id: 'table-proposal' }]); t.check();
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ owner: { kind: 'block', id: 'b' }, quote: 'TARGET' });
    t.adapter.history(false); t.check();
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ owner: { kind: 'proposal', id: 'table-proposal' }, quote: 'TARGET' });
    t.adapter.history(true); t.check();
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ owner: { kind: 'block', id: 'b' }, quote: 'TARGET' });
  } finally { t.close(); }
});

test('Plate list and to-do controls propose properties without changing canonical text or comment identity', () => {
  const t = fixture('TARGET');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.comment('list', block, 0, 6); t.setMode('suggesting'); t.select(block, 0);
    toggleList(t.editor, { listStyleType: 'todo' }); t.check();
    expect(t.model.view().blocks[0].block.attrs.listStyleType).toBeUndefined();
    const proposal = t.model.view().proposals[0];
    expect(proposal.proposal.action).toMatchObject({ kind: 'update_block', attrs: { listStyleType: 'todo' } });
    t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
    t.editor.tf.setNodes({ checked: true }, { at: [0] }); t.check();
    expect(t.model.view().blocks[0].block.attrs.checked).toBe(false);
    const checkbox = t.model.view().proposals.find((view) => view.proposal.state === 'open')!;
    t.adapter.command([{ op: 'accept_proposal', id: checkbox.proposal.id }]); t.check();
    expect(t.model.view().blocks[0].block.attrs.checked).toBe(true);
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
  } finally { t.close(); }
});

test('Plate link wrapping, address editing and removal propose formatting of the original characters', () => {
  const t = fixture('Before TARGET after.');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.comment('link', block, 7, 13); t.setMode('suggesting');
    for (const url of ['https://example.com/first', 'https://example.com/second', null]) {
      t.select(block, 7, 13);
      if (url) upsertLink(t.editor, { url }); else unwrapLink(t.editor);
      t.check();
      const proposal = t.model.view().proposals.find((view) => view.proposal.state === 'open')!;
      expect(proposal.proposal.action).toMatchObject({ kind: 'format', name: 'link', value: url });
      expect(t.model.view().blocks[0].runs.find((run) => run.text === 'TARGET')?.marks.link ?? null).not.toBe(url);
      t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
      expect(t.model.view().blocks[0].runs.find((run) => run.text === 'TARGET')?.marks.link ?? null).toBe(url);
      expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    }
  } finally { t.close(); }
});

test('replacement from a paragraph into a table preserves unselected cell characters and undo restores ownership', () => {
  const t = fixture('Start TARGET\n\n| Header | Second |\n| --- | --- |\n| Inside TARGET | Keep |\n\nEnd TARGET');
  try {
    const start = t.model.view().blocks.find((view) => view.text === 'Start TARGET')!.block.id;
    const inside = t.model.view().blocks.find((view) => view.text === 'Inside TARGET')!.block.id;
    t.comment('cell', inside, 7, 13);
    t.editor.tf.select({ anchor: slatePoint(t.editor.children, start, 6)!, focus: slatePoint(t.editor.children, inside, 7)! });
    t.editor.tf.insertText('Replacement'); t.check();
    expect(t.model.view().blocks.find((view) => view.block.id === start)!.text).toBe('Start Replacement');
    expect(t.model.view().blocks.find((view) => view.block.id === inside)!.text).toBe('TARGET');
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ owner: { id: inside }, quote: 'TARGET' });
    expect(t.model.view().blocks.some((view) => view.text === 'Keep')).toBe(true);
    expect(t.model.view().blocks.some((view) => view.text === 'End TARGET')).toBe(true);
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks.find((view) => view.block.id === inside)!.text).toBe('Inside TARGET');
  } finally { t.close(); }
});

test('Plate rich paste removes active HTML while preserving safe text and bold formatting', () => {
  const t = fixture('TARGET');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.select(block, 0);
    const fragment = t.editor.api.html.deserialize({ element: '<p>Safe <strong>bold</strong><script>bad()</script><a href="javascript:bad">link</a><iframe src="https://example.com"></iframe></p>' });
    t.editor.tf.insertFragment(fragment); t.check();
    expect(t.model.view().blocks[0].text).toBe('Safe boldlinkTARGET');
    expect(t.model.view().blocks[0].runs.some((run) => run.text.includes('bold') && run.marks.bold)).toBe(true);
    expect(t.model.view().blocks[0].runs.some((run) => String(run.marks.link).startsWith('javascript:'))).toBe(false);
  } finally { t.close(); }
});

test('review-only updates invalidate the exact canonical or proposed owner and retain unrelated Slate nodes', () => {
  const t = fixture('First TARGET\n\nUnchanged TARGET');
  try {
    const [first, second] = t.model.view().blocks.map((view) => view.block.id);
    const untouched = t.editor.children[1];
    t.comment('canonical-review', first, 6, 12); t.check();
    expect(t.editor.children[0].quarryReviewKey).toContain('canonical-review');
    expect(t.editor.children[1]).toBe(untouched);
    t.adapter.command([{ op: 'propose_insertion', id: 'inline', author: 'Agent', block: first, at: t.model.point(first, 0), text: 'PROPOSED' }]);
    const proposalNode = () => t.editor.api.node<TElement>({ at: [], match: (node) => node.type === 'quarry_proposal' })![0];
    const before = proposalNode();
    t.adapter.command([{ op: 'add_comment', id: 'proposal-review', author: 'Reviewer', body: 'Keep', ranges: t.model.selectionFor({ kind: 'proposal', id: 'inline' }, 0, 8) }]); t.check();
    expect(proposalNode()).not.toBe(before);
    expect(proposalNode().quarryReviewKey).toContain('proposal-review');
    expect(t.editor.children.find((node) => node.id === second)).toBe(untouched);
    t.adapter.command([{ op: 'resolve_comment', id: 'proposal-review', resolved: true }]); t.check();
    expect(proposalNode().quarryReviewKey).toBeUndefined();
    expect(t.editor.children[1]).toBe(untouched);
  } finally { t.close(); }
});

test('copying a structured proposal creates independent content without moving its review targets', () => {
  const t = fixture('Original');
  try {
    const original = t.model.view().blocks[0].block.id;
    t.adapter.command([{ op: 'propose_blocks', id: 'copy-source', author: 'Agent', parent: null, before: original, blocks: [
      { id: 'heading', kind: 'h2', parent: null, position: 0, attrs: {}, text: 'TARGET' },
    ] }]);
    const proposed = t.editor.children.find((node) => node.quarryProposedBlock === 'heading')!;
    t.comment('copied', String(proposed.id), 0, 6);
    t.select(original, 8);
    t.editor.tf.insertFragment([proposed]); t.check();
    expect(t.model.view().proposals[0].blocks[0].text).toBe('TARGET');
    expect(t.model.view().blocks.map((view) => view.text).join('')).toBe('OriginalTARGET');
    expect(t.model.view().comments[0].target.attachments).toEqual([
      expect.objectContaining({ owner: { kind: 'proposal', id: 'copy-source' }, quote: 'TARGET' }),
    ]);
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks.map((view) => view.text)).toEqual(['Original']);
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
  } finally { t.close(); }
});

test('a long composition across a table boundary is one native undo group', () => {
  const t = fixture('Before\n\n| Header |\n| --- |\n| Inside TARGET |');
  const now = Date.now();
  const clock = vi.spyOn(Date, 'now').mockReturnValue(now);
  try {
    const first = t.model.view().blocks[0].block.id;
    const cell = t.model.view().blocks.find((view) => view.text === 'Inside TARGET')!.block.id;
    t.comment('composition', cell, 7, 13);
    t.editor.tf.select({ anchor: slatePoint(t.editor.children, first, 3)!, focus: slatePoint(t.editor.children, cell, 7)! });
    t.adapter.setComposing(true); t.editor.tf.deleteFragment(); t.check();
    clock.mockReturnValue(now + 5000);
    t.editor.tf.insertText('你好'); t.check(); t.adapter.setComposing(false);
    expect(t.model.view().blocks.find((view) => view.block.id === first)!.text).toBe('Bef你好');
    expect(t.model.view().blocks.find((view) => view.block.id === cell)!.text).toBe('TARGET');
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks.find((view) => view.block.id === first)!.text).toBe('Before');
    expect(t.model.view().blocks.find((view) => view.block.id === cell)!.text).toBe('Inside TARGET');
    t.adapter.history(true); t.check();
    expect(t.model.view().blocks.find((view) => view.block.id === first)!.text).toBe('Bef你好');
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
  } finally { clock.mockRestore(); t.close(); }
});


test('Suggesting inserts a formatted table as proposal-owned blocks and preserves comments on acceptance and undo', () => {
  const t = fixture('Before TARGET.\n\nAfter TARGET.');
  try {
    const [first, second] = t.model.view().blocks;
    t.comment('original', second.block.id, 6, 12);
    const canonical = t.model.view().blocks;
    t.setMode('suggesting');
    t.editor.tf.insertNodes({ type: 'table', children: [
      { type: 'tr', children: [{ type: 'th', children: [{ text: 'Header', bold: true }] }] },
      { type: 'tr', children: [{ type: 'td', children: [{ text: 'TARGET', italic: true }] }] },
    ] }, { at: [1], select: true });
    t.check();
    expect(t.model.view().blocks).toEqual(canonical);
    const proposal = t.model.view().proposals[0];
    expect(proposal.proposal.action).toMatchObject({ kind: 'insert_blocks', before: second.block.id });
    expect(proposal.blocks.map((view) => view.block.kind)).toEqual(['table', 'tr', 'th', 'p', 'tr', 'td', 'p']);
    expect(proposal.blocks.find((view) => view.text === 'TARGET')!.runs[0].marks.italic).toBe(true);
    const anchor = slatePoint(t.editor.children, proposal.proposal.id, 6, true)!;
    const focus = slatePoint(t.editor.children, proposal.proposal.id, 12, true)!;
    t.editor.tf.select({ anchor, focus });
    const draft = t.adapter.beginComment();
    t.adapter.command([{ op: 'add_comment', id: 'proposed', author: 'Reader', body: 'Keep proposed text', ranges: draft.ranges }], draft.base);
    t.adapter.command([{ op: 'insert_text', at: t.model.point(first.block.id, 0), text: 'Agent ' }]);
    t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
    const target = t.model.view().blocks.find((view) => view.text === 'TARGET')!;
    expect(t.model.view().comments.find((view) => view.comment.id === 'proposed')!.target.attachments[0]).toMatchObject({ owner: { kind: 'block', id: target.block.id }, quote: 'TARGET' });
    expect(t.model.view().comments.find((view) => view.comment.id === 'original')!.target.attachments[0]).toMatchObject({ owner: { id: second.block.id }, quote: 'TARGET' });
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks.map((view) => view.block.id)).toEqual(canonical.map((view) => view.block.id));
    expect(t.model.view().comments.find((view) => view.comment.id === 'proposed')!.target.attachments[0].owner.kind).toBe('proposal');
  } finally { t.close(); }
});

test('Suggesting inserts images and duplicates with fresh identities and no canonical changes', () => {
  for (const node of [
    { type: 'img', url: '/assets/test.png', caption: [{ text: 'Image' }], children: [{ text: '' }] },
    { type: 'p', children: [{ text: 'TARGET', bold: true }] },
  ]) {
    const t = fixture('TARGET');
    try {
      const canonical = t.model.view().blocks;
      t.comment('original', canonical[0].block.id, 0, 6);
      t.setMode('suggesting');
      t.editor.tf.insertNodes({ ...node, id: canonical[0].block.id }, { at: [1], select: true }); t.check();
      expect(t.model.view().blocks).toEqual(canonical);
      const proposal = t.model.view().proposals[0];
      expect(proposal.blocks[0].block.id).not.toBe(canonical[0].block.id);
      expect(proposal.blocks[0].block.kind).toBe(node.type);
      t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
      expect(t.model.view().blocks[1].block.kind).toBe(node.type);
      expect(t.model.view().comments[0].target.attachments[0].owner.id).toBe(canonical[0].block.id);
    } finally { t.close(); }
  }
});


test('Suggesting a Slate block move keeps canonical order until acceptance and carries late edits and comments', () => {
  for (const markdown of ['First TARGET.\n\nSecond.\n\nLast TARGET.', '| TARGET |\n| --- |\n| Cell |\n\nLast.']) {
    const t = fixture(markdown);
    try {
      const initial = t.model.view().blocks, first = initial[0];
      const target = initial.find((view) => view.text.includes('TARGET'))!;
      const start = target.text.indexOf('TARGET');
      t.comment('keep', target.block.id, start, start + 6);
      const canonical = t.model.view().blocks;
      t.setMode('suggesting');
      const last = t.editor.children.length - 1;
      t.editor.tf.moveNodes({ at: [0], to: [last] }); t.check();
      expect(t.model.view().blocks).toEqual(canonical);
      const proposal = t.model.view().proposals[0];
      expect(proposal.proposal.action).toMatchObject({ kind: 'move_block', block: first.block.id, before: null });
      t.adapter.command([{ op: 'insert_text', at: t.model.point(target.block.id, 0), text: 'Agent ' }]);
      t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
      expect(t.model.view().blocks.filter((view) => view.block.parent === null).at(-1)!.block.id).toBe(first.block.id);
      expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ owner: { id: target.block.id }, quote: 'TARGET' });
      t.adapter.history(false); t.check();
      expect(t.model.view().blocks.filter((view) => view.block.parent === null)[0].block.id).toBe(first.block.id);
    } finally { t.close(); }
  }
});


test('source and property edits stay inside a proposed block, and a private draft follows acceptance', () => {
  const t = fixture('Original.');
  try {
    const canonical = t.model.view().blocks;
    t.setMode('suggesting');
    t.editor.tf.insertNodes({ type: 'mermaid', code: 'flowchart LR\nA --> B', children: [{ text: '' }] }, { at: [1] }); t.check();
    const node = t.editor.children.find((node) => node.type === 'mermaid')!;
    const base = t.adapter.beginSourceEdit(String(node.id));
    expect(base.proposal).toBe(t.model.view().proposals[0].proposal.id);
    t.adapter.saveSourceEdit(base, { ...base.attrs, code: 'flowchart LR\nX --> Y' }); t.check();
    expect(t.model.view().blocks).toEqual(canonical);
    expect(t.model.view().proposals).toHaveLength(1);
    expect(t.model.view().proposals[0].blocks[0].block.attrs.code).toBe('flowchart LR\nX --> Y');
    expect(() => t.adapter.saveSourceEdit(base, { code: 'stale source' })).toThrow('The source changed');
    const updated = t.adapter.beginSourceEdit(String(node.id));
    const store = sourceDrafts(t.editor);
    store.open(String(node.id), updated, 'code', 'flowchart LR\nPrivate --> Draft');
    t.adapter.command([{ op: 'accept_proposal', id: base.proposal! }]); t.check();
    const draft = store.get(base.id)!;
    expect(draft.value).toBe('flowchart LR\nPrivate --> Draft');
    expect(draft.base.proposal).toBeUndefined();
    expect(draft.unavailable).toBeUndefined();
    expect(store.get(String(node.id))).toBeUndefined();
    expect(t.model.view().blocks[1].block.attrs.code).toBe('flowchart LR\nX --> Y');
    t.setMode('editing');
    t.adapter.saveSourceEdit(draft.base, { ...draft.base.attrs, code: draft.value }); t.check();
    expect(t.model.view().blocks[1].block.attrs.code).toBe(draft.value);
  } finally { t.close(); }
});

test('Plate properties of proposed images and headings can change before acceptance', () => {
  const t = fixture('Original.');
  try {
    const canonical = t.model.view().blocks;
    t.setMode('suggesting');
    t.editor.tf.insertNodes([{ type: 'img', url: '/assets/example.png', children: [{ text: '' }] }, { type: 'p', children: [{ text: 'TARGET' }] }], { at: [1] }); t.check();
    t.editor.tf.setNodes({ width: 320 }, { at: [1] });
    t.editor.tf.setNodes({ type: 'h2' }, { at: [2] }); t.check();
    const proposal = t.model.view().proposals[0];
    expect(t.model.view().blocks).toEqual(canonical);
    expect(proposal.blocks[0].block.attrs.width).toBe(320);
    expect(proposal.blocks[1].block.kind).toBe('h2');
    t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
    expect(t.model.view().blocks[1].block.attrs.width).toBe(320);
    expect(t.model.view().blocks[2].block.kind).toBe('h2');
  } finally { t.close(); }
});

test('editing a proposed table retains cell sources through column insertion, row movement, removal and undo', () => {
  const t = fixture('Canonical TARGET');
  try {
    const canonical = t.model.view().blocks;
    t.setMode('suggesting');
    t.editor.tf.insertNodes({ type: 'table', children: [
      { type: 'tr', children: [{ type: 'th', children: [{ text: 'Header' }] }] },
      { type: 'tr', children: [{ type: 'td', children: [{ text: 'TARGET', italic: true }] }] },
    ] }, { at: [1] }); t.check();
    const proposal = t.model.view().proposals[0].proposal.id;
    const original = t.model.view().proposals[0].blocks.find((view) => view.text === 'TARGET')!;
    const node = () => t.editor.api.node<TElement>({ at: [], match: (node) => node.quarryProposedBlock === original.block.id })!;
    t.comment('original-cell', String(node()[0].id), 0, 6);
    t.editor.tf.withoutNormalizing(() => {
      for (const [index, type] of ['th', 'td'].entries()) t.editor.tf.insertNodes({ type, children: [{ type: 'p', children: [{ text: 'New TARGET', bold: true }] }] }, { at: [1, index, 0] });
      t.editor.tf.insertNodes({ type: 'tr', children: [{ type: 'td', children: [{ text: 'Last' }] }, { type: 'td', children: [{ text: 'Row' }] }] }, { at: [1, 2] });
    }); t.check();
    expect(t.model.view().blocks).toEqual(canonical);
    expect(t.model.view().proposals[0].blocks.filter((view) => view.text === 'New TARGET').every((view) => view.runs[0].marks.bold)).toBe(true);
    t.editor.tf.moveNodes({ at: [1, 2], to: [1, 1] }); t.check();
    expect(t.model.view().proposals[0].blocks.find((view) => view.block.id === original.block.id)!.block.segments).toEqual(original.block.segments);
    t.editor.tf.removeNodes({ at: [1, 2] }); t.check();
    expect(t.model.view().comments[0].target.state).toBe('hidden');
    t.adapter.history(false); t.check();
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    t.select(String(node()[0].id), 0); t.editor.tf.insertText('Agent '); t.check();
    t.adapter.command([{ op: 'accept_proposal', id: proposal }]); t.check();
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ owner: { kind: 'block', id: original.block.id }, quote: 'TARGET' });
    expect(t.model.view().blocks.find((view) => view.block.id === original.block.id)!.text).toBe('Agent TARGET');
  } finally { t.close(); }
});

test('deleting all proposed blocks rejects the suggestion and undo restores its comments', () => {
  const t = fixture('Canonical TARGET');
  try {
    const canonical = t.model.view().blocks;
    t.setMode('suggesting');
    t.editor.tf.insertNodes({ type: 'p', children: [{ text: 'TARGET' }] }, { at: [1] }); t.check();
    const id = String(t.editor.children[1].id);
    t.comment('proposed', id, 0, 6);
    t.adapter.deleteBlock(id); t.check();
    expect(t.model.view().blocks).toEqual(canonical);
    expect(t.model.view().proposals[0].proposal.state).toBe('rejected');
    expect(t.model.view().comments[0].target.state).toBe('hidden');
    t.adapter.history(false); t.check();
    expect(t.model.view().proposals[0].proposal.state).toBe('open');
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
  } finally { t.close(); }
});

for (const kind of ['p', 'code_line']) test(`proposed ${kind} Enter, join, undo and acceptance keep characters and the exact caret`, () => {
  const t = fixture('Canonical TARGET');
  try {
    const canonical = t.model.view().blocks;
    t.setMode('suggesting');
    const text = { type: kind, children: [{ text: '😀TARGET' }] };
    t.editor.tf.insertNodes(kind === 'p' ? text : { type: 'code_block', children: [text] }, { at: [1] }); t.check();
    const proposal = t.model.view().proposals[0].proposal.id;
    const original = t.model.view().proposals[0].blocks.find((view) => view.text === '😀TARGET')!;
    const node = (id: string) => t.editor.api.node<TElement>({ at: [], match: (node) => node.quarryProposedBlock === id })![0];
    t.comment('exact', String(node(original.block.id).id), 2, 8);
    t.select(String(node(original.block.id).id), 5);
    t.editor.tf.insertBreak(); t.check();
    let parts = t.model.view().proposals[0].blocks.filter((view) => view.block.kind === kind);
    expect(parts.map((view) => view.text)).toEqual(['😀TAR', 'GET']);
    expect(parts[1].block.segments[0].source).toBe(original.block.segments[0].source);
    const right = parts[1].block.id;
    expect(t.editor.selection!.anchor).toEqual(slatePoint(t.editor.children, String(node(right).id), 0));
    t.select(String(node(right).id), 1); t.adapter.rememberSelection();
    t.adapter.command([{ op: 'insert_text', at: t.model.proposedBlockPoint(proposal, original.block.id, 0), text: 'Agent ' }]); t.check();
    expect(t.editor.selection!.anchor).toEqual(slatePoint(t.editor.children, String(node(right).id), 1));
    t.select(String(node(right).id), 0); t.editor.tf.deleteBackward('character'); t.check();
    parts = t.model.view().proposals[0].blocks.filter((view) => view.block.kind === kind);
    expect(parts.map((view) => view.text)).toEqual(['Agent 😀TARGET']);
    t.adapter.history(false); t.check();
    expect(t.model.view().proposals[0].blocks.filter((view) => view.block.kind === kind).map((view) => view.text)).toEqual(['Agent 😀TAR', 'GET']);
    expect(t.model.view().blocks).toEqual(canonical);
    t.adapter.command([{ op: 'accept_proposal', id: proposal }]); t.check();
    expect(t.model.view().comments[0].target.attachments.map((part) => part.quote).join('')).toBe('TARGET');
    expect(t.model.view().comments[0].target.attachments.map((part) => part.owner.id)).toEqual([original.block.id, right]);
  } finally { t.close(); }
});

for (const count of [2, 3]) test(`pasting ${count} rich paragraphs within a proposed paragraph retains characters through splits, joins and undo`, () => {
  const t = fixture('Canonical TARGET');
  try {
    const canonical = t.model.view().blocks;
    t.setMode('suggesting');
    t.editor.tf.insertNodes({ type: 'p', children: [{ text: '😀TARGET' }] }, { at: [1] }); t.check();
    const proposal = t.model.view().proposals[0].proposal.id;
    const id = String(t.editor.children[1].id);
    t.comment('original', id, 2, 8);
    t.comment('tail', id, 5, 8);
    const quote = (id: string) => t.model.view().comments.find((view) => view.comment.id === id)!.target.attachments.map((part) => part.quote).join('');
    t.select(id, 5);
    t.editor.tf.insertFragment([
      { type: 'p', children: [{ text: 'New', bold: true }] },
      ...(count === 3 ? [{ type: 'p', children: [{ text: 'Middle', underline: true }] }] : []),
      { type: 'p', children: [{ text: 'Last', italic: true }] },
    ]); t.check();
    const expected = ['😀TARNew', ...(count === 3 ? ['Middle'] : []), 'LastGET'];
    expect(t.model.view().proposals).toHaveLength(1);
    expect(t.model.view().proposals[0].blocks.map((view) => view.text)).toEqual(expected);
    expect(t.model.view().blocks).toEqual(canonical);
    expect(quote('tail')).toBe('GET');
    expect(t.model.view().comments.find((view) => view.comment.id === 'original')!.comment.original_quote).toBe('TARGET');
    t.adapter.history(false); t.check();
    expect(t.model.view().proposals[0].blocks.map((view) => view.text)).toEqual(['😀TARGET']);
    expect(quote('original')).toBe('TARGET'); expect(quote('tail')).toBe('GET');
    t.adapter.history(true); t.check();
    t.adapter.command([{ op: 'accept_proposal', id: proposal }]); t.check();
    expect(t.model.view().blocks.slice(1).map((view) => view.text)).toEqual(expected);
    expect(quote('tail')).toBe('GET');
    expect(t.model.view().blocks[1].runs.find((run) => run.text === 'New')!.marks.bold).toBe(true);
    expect(t.model.view().blocks.at(-1)!.runs.find((run) => run.text === 'Last')!.marks.italic).toBe(true);
  } finally { t.close(); }
});

for (const before of [false, true]) test(`merging a new proposed line before publication keeps existing sources (new line first: ${before})`, () => {
  const t = fixture('Canonical TARGET');
  try {
    t.setMode('suggesting');
    t.editor.tf.insertNodes({ type: 'code_block', children: [{ type: 'code_line', children: [{ text: 'TARGET' }] }] }, { at: [1] }); t.check();
    t.comment('existing', String(t.editor.children[1].children[0].id), 0, 6);
    t.editor.tf.withoutNormalizing(() => {
      t.editor.tf.insertNodes({ type: 'code_line', children: [{ text: 'New' }] }, { at: [1, before ? 0 : 1] });
      t.editor.tf.mergeNodes({ at: [1, 1] });
    }); t.check();
    expect(t.model.view().proposals[0].blocks.filter((view) => view.block.kind === 'code_line').map((view) => view.text)).toEqual([before ? 'NewTARGET' : 'TARGETNew']);
    expect(t.model.view().comments[0].target.attachments.map((part) => part.quote).join('')).toBe('TARGET');
    t.adapter.history(false); t.check();
    expect(t.model.view().proposals[0].blocks.filter((view) => view.block.kind === 'code_line').map((view) => view.text)).toEqual(['TARGET']);
  } finally { t.close(); }
});

for (const proposed of [false, true]) for (const count of [3, 5, 8]) test(`ordered plugin sequences match native replay and fresh readers (${proposed ? 'proposal' : 'canonical'}, ${count} blocks)`, () => {
  const t = fixture(proposed ? 'Canonical' : Array.from({ length: count }, (_, index) => `Block ${index} 😀 TARGET`).join('\n\n'));
  let reader: DocumentModel | undefined;
  try {
    if (proposed) {
      t.setMode('suggesting');
      t.editor.tf.insertNodes(Array.from({ length: count }, (_, index) => ({ type: 'p', children: [{ text: `Block ${index} 😀 TARGET` }] })), { at: [1] }); t.check();
      t.setMode('editing');
    }
    const views = () => proposed ? t.model.view().proposals[0].blocks : t.model.view().blocks;
    const last = views().at(-1)!;
    const display = (id: string) => proposed ? String(t.editor.api.node<TElement>({ at: [], match: (node) => node.quarryProposedBlock === id })![0].id) : id;
    t.comment('target', display(last.block.id), last.text.length - 6, last.text.length);
    reader = new DocumentModel(t.model.save());
    const initial = reader.heads(); reader.view();
    const offset = proposed ? 1 : 0;
    const before = t.batches.length;
    t.editor.tf.withoutNormalizing(() => {
      t.editor.tf.moveNodes({ at: [offset + count - 1], to: [offset + 1] });
      t.editor.tf.removeNodes({ at: [offset + count - 1] });
      t.editor.tf.mergeNodes({ at: [offset + 1] });
      t.editor.tf.select({ path: [offset, 0], offset: 2 });
      t.editor.tf.insertBreak();
    }); t.check();
    expect(t.batches.length - before).toBe(1);
    expect(t.batches.at(-1)!.requests).toHaveLength(1);
    reader.merge(t.model.changesSince(initial), initial, t.model.heads());
    expect(reader.view()).toEqual(t.model.view());
    const fresh = new DocumentModel(reader.save());
    try { expect(reader.view()).toEqual(fresh.view()); } finally { fresh.dispose(); }
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    t.adapter.history(false); t.check();
    expect(views()).toHaveLength(count);
    t.adapter.history(true); t.check();
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
  } finally { reader?.dispose(); t.close(); }
});

test.each(['before', 'after', 'another block'] as const)('moving an existing marked text leaf %s preserves review and one transaction', (destination) => {
  const t = fixture('Start **TARGET** end.\n\nSecond.');
  try {
    const [first, second] = t.model.view().blocks;
    t.comment('target', first.block.id, 6, 12);
    const before = t.batches.length;
    t.editor.tf.moveNodes({ at: [0, 1], to: destination === 'before' ? [0, 0] : destination === 'after' ? [0, 2] : [1, 0] }); t.check();
    expect(t.batches.length - before).toBe(1);
    const expected = destination === 'before' ? ['TARGETStart  end.', 'Second.'] : destination === 'after' ? ['Start  end.TARGET', 'Second.'] : ['Start  end.', 'TARGETSecond.'];
    expect(t.model.view().blocks.map((view) => view.text)).toEqual(expected);
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ quote: 'TARGET', owner: { id: destination === 'another block' ? second.block.id : first.block.id } });
    const moved = t.model.view().blocks[destination === 'another block' ? 1 : 0];
    expect(moved.runs.find((run) => run.text === 'TARGET')!.marks.bold).toBe(true);
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks.map((view) => view.text)).toEqual([first.text, second.text]);
  } finally { t.close(); }
});

for (const mode of ['editing', 'suggesting'] as const) test(`moving proposed text between blocks keeps its review in ${mode} mode`, () => {
  const t = fixture('Canonical');
  try {
    t.setMode('suggesting');
    t.editor.tf.insertNodes([{ type: 'p', children: [{ text: 'Start ' }, { text: 'TARGET', bold: true }, { text: ' end.' }] }, { type: 'p', children: [{ text: 'Second.' }] }], { at: [1] }); t.check();
    const proposal = t.model.view().proposals[0];
    const source = proposal.blocks[0];
    const node = t.editor.api.node<TElement>({ at: [], match: (node) => node.quarryProposedBlock === source.block.id })![0];
    t.comment('target', String(node.id), 6, 12); t.setMode(mode);
    t.editor.tf.moveNodes({ at: [1, 1], to: [2, 0] }); t.check();
    expect(t.model.view().blocks.map((view) => view.text)).toEqual(['Canonical']);
    expect(t.model.view().proposals[0].blocks.map((view) => view.text)).toEqual(['Start  end.', 'TARGETSecond.']);
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    t.adapter.history(false); t.check();
    expect(t.model.view().proposals[0].blocks.map((view) => view.text)).toEqual(['Start TARGET end.', 'Second.']);
    t.adapter.history(true); t.check();
    t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
    expect(t.model.view().comments[0].target.attachments[0]).toMatchObject({ quote: 'TARGET', owner: { id: proposal.blocks[1].block.id } });
  } finally { t.close(); }
});

test('a plugin split applies its new block properties in the same native request', () => {
  const t = fixture('Start TARGET');
  try {
    const block = t.model.view().blocks[0].block.id;
    t.comment('target', block, 6, 12);
    const before = t.batches.length;
    t.editor.tf.withoutNormalizing(() => {
      t.editor.tf.apply({ type: 'split_node', path: [0, 0], position: 6, properties: {} });
      t.editor.tf.apply({ type: 'split_node', path: [0], position: 1, properties: { type: 'h3' } });
    }); t.check();
    expect(t.batches.length - before).toBe(1);
    expect(t.model.view().blocks.map((view) => [view.block.kind, view.text])).toEqual([['p', 'Start '], ['h3', 'TARGET']]);
    expect(t.model.view().comments[0].target.attachments[0].quote).toBe('TARGET');
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks.map((view) => view.text)).toEqual(['Start TARGET']);
  } finally { t.close(); }
});

test('selection replacement can end in the virtual input after repeated undo', () => {
  const t = fixture('### Keep TARGET');
  try {
    const block = t.model.view().blocks[0].block.id;
    for (let n = 0; n < 3; n++) {
      t.editor.tf.select({ anchor: slatePoint(t.editor.children, block, 5)!, focus: { path: [1, 0], offset: 0 } });
      t.editor.tf.deleteFragment(); t.check();
      expect(t.model.view().blocks.map((view) => view.text)).toEqual(['Keep ']);
      t.adapter.history(false); t.check();
      expect(t.model.view().blocks.map((view) => view.text)).toEqual(['Keep TARGET']);
    }
  } finally { t.close(); }
});

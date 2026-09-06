import { MermaidPlugin } from './mermaid-block';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { BaseParagraphPlugin, TrailingBlockPlugin, createSlateEditor, type SlateEditor } from 'platejs';
import { BaseBoldPlugin, BaseH1Plugin } from '@platejs/basic-nodes';
import { BaseCodeBlockPlugin, BaseCodeLinePlugin, toggleCodeBlock } from '@platejs/code-block';
import { BaseLinkPlugin, upsertLink, unwrapLink } from '@platejs/link';
import { initSync } from '../../generated/document/quarry_document';
import { DocumentModel, type DocumentBatch } from './document-model';
import { ImageKit } from './image-element';
import { NativeProposalPlugin } from './plate-review-decoration';
import { WikiLinkPlugin } from './wiki-link-element';
import { PlateDocumentAdapter } from './plate-document-adapter';
import { BaseTablePlugin, BaseTableRowPlugin, BaseTableCellPlugin, BaseTableCellHeaderPlugin, insertTable, insertTableRow, insertTableColumn } from '@platejs/table';
import type { EditorMode } from './editor-types';
import { nativeNodeIdOptions, projectPlate } from './plate-document-projection';

beforeAll(() => initSync({ module: readFileSync(resolve(process.cwd(), 'src/generated/document/quarry_document_bg.wasm')) }));

function setup(markdown = 'Before TARGET after.\n\nSecond TARGET.') {
  const model = DocumentModel.fromMarkdown(markdown); const first = model.view().blocks[0].block.id;
  model.edit((draft) => draft.apply([{ op: 'add_comment', id: 'c', author: 'Agent', body: 'First only', ranges: draft.model.selection(first, 7, 13) }]));
  const replay = new DocumentModel(model.save()); const batches: DocumentBatch[] = []; const errors: unknown[] = [];
  const editor = createSlateEditor({ nodeId: nativeNodeIdOptions, value: projectPlate(model.view()), plugins: [TrailingBlockPlugin, MermaidPlugin, ...ImageKit, NativeProposalPlugin, WikiLinkPlugin, BaseParagraphPlugin, BaseH1Plugin, BaseBoldPlugin, BaseLinkPlugin, BaseCodeBlockPlugin, BaseCodeLinePlugin, BaseTablePlugin.configure({ options: { disableMerge: true } }), BaseTableRowPlugin, BaseTableCellPlugin, BaseTableCellHeaderPlugin] });
  let mode: EditorMode = 'editing';
  const adapter = new PlateDocumentAdapter(editor, model, { mode: () => mode, author: () => 'Reviewer', changed: (batch) => batches.push(batch), error: (error) => errors.push(error) });
  const check = () => {
    adapter.flush(); expect(errors).toEqual([]);
    for (const batch of batches.splice(0)) replay.applyBatch(batch);
    expect(model.view()).toEqual(replay.view());
    expect(model.view().comments.find((view) => view.comment.id === 'c')!.target.attachments.map((part) => part.quote).join('')).toBe('TARGET');
  };
  const close = () => { adapter.dispose(); model.dispose(); replay.dispose(); };
  return { model, replay, editor, adapter, first, check, close, errors, setMode(value: EditorMode) { mode = value; } };
}

test('real Slate typing, formatting, split, move, join and native undo preserve exact comment identity', () => {
  const t = setup();
  try {
    t.editor.tf.select({ path: [0, 0], offset: 0 }); t.editor.tf.insertText('😀Hello '); t.check();
    t.editor.tf.select({ anchor: { path: [0, 0], offset: 0 }, focus: { path: [0, 0], offset: 8 } });
    t.editor.tf.addMark('bold', true); t.check();
    t.editor.tf.select({ path: [0, 1], offset: 7 }); t.editor.tf.insertBreak(); t.check();
    t.editor.tf.moveNodes({ at: [1], to: [3] }); t.check();
    t.adapter.history(false); t.check();
    t.adapter.history(false); t.check();
    t.adapter.history(true); t.check();
  } finally { t.close(); }
});

test('Plate code-block wrap and unwrap publish one valid request and retain the commented characters', () => {
  const t = setup();
  try {
    t.editor.tf.select({ path: [0, 0], offset: 7 });
    toggleCodeBlock(t.editor); t.check();
    expect(t.model.view().blocks.some((view) => view.block.kind === 'code_block')).toBe(true);
    toggleCodeBlock(t.editor); t.check();
    expect(t.model.view().blocks.some((view) => view.block.kind === 'code_block')).toBe(false);
  } finally { t.close(); }
});


test('Plate link wrapping and unwrapping format the original commented characters', () => {
  const t = setup();
  try {
    t.editor.tf.select({ anchor: { path: [0, 0], offset: 7 }, focus: { path: [0, 0], offset: 13 } });
    upsertLink(t.editor, { url: 'https://example.com' }); t.check();
    expect(t.model.view().blocks[0].runs.find((run) => run.text === 'TARGET')?.marks.link).toBe('https://example.com');
    unwrapLink(t.editor); t.check();
    expect(t.model.view().blocks[0].runs.every((run) => !run.marks.link)).toBe(true);
  } finally { t.close(); }
});

test('Plate table insertion, row and column controls, and widths replay as native structure', () => {
  const t = setup();
  try {
    t.editor.tf.select({ path: [1, 0], offset: 14 });
    insertTable(t.editor, { colCount: 2, rowCount: 2, header: true }, { select: true }); t.check();
    insertTableRow(t.editor); t.check();
    insertTableColumn(t.editor); t.check();
    const table = t.editor.api.node({ match: { type: 'table' } });
    expect(table).toBeDefined();
    t.editor.tf.setNodes({ colSizes: [150, 175, 200] }, { at: table![1] }); t.check();
    expect(t.model.view().blocks.find((view) => view.block.kind === 'table')?.block.attrs.colSizes).toEqual([150, 175, 200]);
  } finally { t.close(); }
});

test('an invalid final tree cannot publish a partial editor transaction', () => {
  const t = setup();
  try {
    const before = t.model.heads();
    t.editor.tf.withoutNormalizing(() => {
      t.editor.tf.select({ path: [0, 0], offset: 0 });
      t.editor.tf.insertText('Not published ');
      t.editor.tf.setNodes({ type: 'code_line' }, { at: [0] });
    });
    t.adapter.flush();
    expect(t.errors).toHaveLength(1);
    expect(t.model.heads()).toEqual(before);
    expect(t.editor.children).toEqual(projectPlate(t.model.view()));
  } finally { t.close(); }
});


test('typing wiki-link delimiters around a comment creates a chip without replacing its characters', () => {
  const t = setup();
  try {
    t.editor.tf.select({ path: [0, 0], offset: 7 }); t.editor.tf.insertText('[['); t.check();
    t.editor.tf.select({ path: [0, 0], offset: 15 }); t.editor.tf.insertText(']]'); t.check();
    expect(t.editor.children[0].children[1]).toMatchObject({ type: 'wikilink', target: 'TARGET' });
    expect(t.model.markdown()).toContain('Before [[TARGET]] after.');
    t.adapter.refresh();
    expect(t.editor.children[0].children[1]).toMatchObject({ type: 'wikilink', target: 'TARGET' });
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks[0].text).toBe('Before [[TARGET after.');
    t.adapter.history(true); t.check();
    expect(t.model.markdown()).toContain('Before [[TARGET]] after.');
  } finally { t.close(); }
});

test('native Markdown import keeps wiki-links in editable paragraphs', () => {
  const source = '😀 [[Other Note#Section|label]] and ![[Picture]].';
  const model = DocumentModel.fromMarkdown(source);
  try {
    expect(model.view().blocks[0].block.kind).toBe('p');
    expect(model.view().blocks[0].text).toBe(source);
    expect(model.markdown().trim()).toBe(source);
    const value = projectPlate(model.view());
    expect(value[0].children.filter((node) => node.type === 'wikilink')).toHaveLength(2);
  } finally { model.dispose(); }
});


test('inline suggestion editing and comments stay in the native proposal through acceptance and undo', () => {
  const t = setup();
  try {
    t.adapter.command([{ op: 'propose_replacement', id: 'proposal', author: 'Agent', block: t.first,
      at: t.model.point(t.first, 7), ranges: t.model.selection(t.first, 7, 13), text: 'NEW' }]); t.check();
    const proposal = t.editor.api.node({ at: [], match: (node: import('platejs').TElement) => node.quarryProposal === 'proposal' });
    expect(proposal).toBeDefined();
    const path = [...proposal![1], 0];
    t.editor.tf.select({ path, offset: 3 }); t.editor.tf.insertText(' improved'); t.check();
    expect(t.model.view().proposals[0].text).toBe('NEW improved');
    expect(t.model.view().blocks[0].text).toBe('Before TARGET after.');
    t.editor.tf.select({ anchor: { path, offset: 4 }, focus: { path, offset: 12 } });
    const draft = t.adapter.beginComment();
    t.adapter.command([{ op: 'add_comment', id: 'on-proposal', author: 'Reviewer', body: 'Keep these characters', ranges: draft.ranges }], draft.base); t.check();
    expect(t.model.view().comments.find((view) => view.comment.id === 'on-proposal')!.target.attachments[0]).toMatchObject({ owner: { kind: 'proposal', id: 'proposal' }, quote: 'improved' });
    t.adapter.command([{ op: 'accept_proposal', id: 'proposal' }]);
    expect(t.errors).toEqual([]);
    expect(t.model.view().blocks[0].text).toBe('Before NEW improved after.');
    expect(t.model.view().comments.find((view) => view.comment.id === 'on-proposal')!.target.attachments[0]).toMatchObject({ owner: { kind: 'block', id: t.first }, quote: 'improved' });
    t.adapter.history(false); t.check();
    expect(t.model.view().comments.find((view) => view.comment.id === 'on-proposal')!.target.attachments[0].owner.kind).toBe('proposal');
  } finally { t.close(); }
});


test('suggesting mode types into one native proposal while leaving the body and existing comment unchanged', () => {
  const t = setup();
  try {
    t.setMode('suggesting');
    t.editor.tf.select({ anchor: { path: [0, 0], offset: 7 }, focus: { path: [0, 0], offset: 13 } });
    t.editor.tf.insertText('😀'); t.check();
    t.editor.tf.insertText(' better'); t.check();
    expect(t.model.view().blocks[0].text).toBe('Before TARGET after.');
    expect(t.model.view().proposals).toHaveLength(1);
    expect(t.model.view().proposals[0]).toMatchObject({ text: '😀 better', proposal: { author: 'Reviewer', state: 'open' } });
    t.editor.tf.insertBreak(); t.editor.tf.insertText('next line'); t.check();
    expect(t.model.view().proposals[0].text).toBe('😀 better\nnext line');
  } finally { t.close(); }
});


test('a private comment draft follows its exact characters through agent insertion and split', () => {
  const t = setup();
  let agent: DocumentModel | undefined;
  try {
    const heads = t.model.heads();
    t.editor.tf.select({ anchor: { path: [0, 0], offset: 7 }, focus: { path: [0, 0], offset: 13 } });
    const draft = t.adapter.beginComment();
    const captured = t.model.captureTarget(draft.ranges);
    expect(captured.quote).toBe('TARGET'); expect(t.model.heads()).toEqual(heads);
    agent = new DocumentModel(t.model.save());
    agent.edit((change) => {
      change.apply([{ op: 'insert_text', at: change.model.point(t.first, 0), text: 'Agent TARGET prefix. ' }]);
      change.apply([{ op: 'split_block', block: t.first, at: change.model.point(t.first, 30), new_block: 'agent-tail' }]);
    });
    t.model.merge(agent.save()); t.replay.merge(agent.save()); t.adapter.refresh();
    const target = t.model.resolveTarget(captured.fragments);
    expect(target.attachments.map((part) => part.quote).join('')).toBe('TARGET');
    expect(target.attachments).toHaveLength(2);
    expect(t.model.view().comments).toHaveLength(1);
    t.adapter.command([{ op: 'add_comment', id: 'draft', author: 'Reviewer', body: 'Exact target', ranges: draft.ranges }], draft.base); t.check();
    expect(t.model.view().comments.find((view) => view.comment.id === 'draft')!.target).toEqual(target);
  } finally { agent?.dispose(); t.close(); }
});

test('an image upload placeholder stays local and keeps its insertion point across agent changes', () => {
  const t = setup();
  let agent: DocumentModel | undefined;
  try {
    const heads = t.model.heads();
    t.editor.tf.insertNodes({ type: 'placeholder', id: 'upload', mediaType: 'img', children: [{ text: '' }] }, { at: [1] }); t.check();
    expect(t.model.heads()).toEqual(heads);
    agent = new DocumentModel(t.model.save());
    agent.edit((change) => change.apply([{ op: 'insert_block', block: { id: 'prefix', kind: 'p', parent: null, position: 0, attrs: {}, text: 'Agent prefix' } }]));
    t.model.merge(agent.save()); t.replay.merge(agent.save()); t.adapter.refresh();
    expect(t.editor.children[2]).toMatchObject({ id: 'upload', type: 'placeholder' });
    t.editor.tf.withoutNormalizing(() => {
      t.editor.tf.removeNodes({ at: [2] });
      t.editor.tf.insertNodes({ type: 'img', url: 'assets/upload.png', children: [{ text: '' }] }, { at: [2] });
    }); t.check();
    expect(t.model.view().blocks[2].block).toMatchObject({ kind: 'img', attrs: { url: 'assets/upload.png' } });
    expect(t.model.view().blocks[3].text).toBe('Second TARGET.');
    expect(t.model.view().blocks.some((view) => view.block.kind === 'placeholder')).toBe(false);
  } finally { agent?.dispose(); t.close(); }
});


test('one comment can span canonical text, a proposal and another canonical block', () => {
  const t = setup();
  try {
    t.adapter.command([{ op: 'propose_insertion', id: 'mixed', author: 'Agent', block: t.first, at: t.model.point(t.first, 7), text: 'new ' }]);
    t.editor.tf.select({ anchor: { path: [0, 0], offset: 0 }, focus: { path: [1, 0], offset: 6 } });
    const draft = t.adapter.beginComment();
    expect(t.model.captureTarget(draft.ranges).quote).toBe('Before new TARGET after.Second');
    t.adapter.command([{ op: 'add_comment', id: 'across', author: 'Reviewer', body: 'Mixed owners', ranges: draft.ranges }], draft.base);
    const parts = t.model.view().comments.find((view) => view.comment.id === 'across')!.target.attachments;
    expect(parts.some((part) => part.owner.kind === 'proposal' && part.quote === 'new ')).toBe(true);
    t.adapter.command([{ op: 'accept_proposal', id: 'mixed' }]);
    expect(t.model.view().comments.find((view) => view.comment.id === 'across')!.target.attachments.every((part) => part.owner.kind === 'block')).toBe(true);
    t.check();
  } finally { t.close(); }
});


test('the Plate Bold control proposes formatting without copying canonical characters', () => {
  const t = setup();
  try {
    t.setMode('suggesting');
    t.editor.tf.select({ anchor: { path: [0, 0], offset: 7 }, focus: { path: [0, 0], offset: 13 } });
    t.editor.tf.addMark('bold', true); t.check();
    expect(t.model.view().blocks[0].runs.every((run) => !run.marks.bold)).toBe(true);
    const proposal = t.model.view().proposals[0];
    expect(proposal.proposal.action.kind).toBe('format');
    expect(proposal.target.attachments[0].quote).toBe('TARGET');
    t.adapter.command([{ op: 'accept_proposal', id: proposal.proposal.id }]); t.check();
    expect(t.model.view().blocks[0].runs.find((run) => run.text === 'TARGET')!.marks.bold).toBe(true);
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks[0].runs.every((run) => !run.marks.bold)).toBe(true);
  } finally { t.close(); }
});


test('typing after a trailing diagram gets fresh identity after undo and never saves the empty input', () => {
  const t = setup('Before TARGET after.\n\n```mermaid\nflowchart LR\nA --> B\n```');
  try {
    expect(t.model.view().blocks).toHaveLength(2);
    t.editor.tf.select({ path: [2, 0], offset: 0 }); t.editor.tf.insertText('First'); t.check();
    const first = t.model.view().blocks[2].block.id;
    t.adapter.history(false); t.check();
    expect(t.model.view().blocks).toHaveLength(2);
    t.editor.tf.select({ path: [2, 0], offset: 0 }); t.editor.tf.insertText('Second'); t.check();
    expect(t.model.view().blocks[2].text).toBe('Second');
    expect(t.model.view().blocks[2].block.id).not.toBe(first);
  } finally { t.close(); }
});


test('removing the final input leaves one local input and no empty native block', () => {
  const t = setup('Before TARGET after.\n\n```mermaid\nflowchart LR\nA --> B\n```');
  try {
    t.editor.tf.removeNodes({ at: [2] }); t.check();
    expect(t.model.view().blocks).toHaveLength(2);
    expect(t.editor.children).toHaveLength(3);
    t.editor.tf.select({ path: [2, 0], offset: 0 }); t.editor.tf.insertText('Tail'); t.check();
    expect(t.model.view().blocks).toHaveLength(3);
  } finally { t.close(); }
});

test('block conversion and deletion in Suggesting mode require native review decisions', () => {
  const t = setup();
  try {
    t.setMode('suggesting');
    t.adapter.updateBlock(t.first, 'h2', {}); t.check();
    expect(t.model.view().blocks[0].block.kind).toBe('p');
    const update = t.model.view().proposals[0].proposal.id;
    t.adapter.command([{ op: 'accept_proposal', id: update }]); t.check();
    expect(t.model.view().blocks[0].block.kind).toBe('h2');
    t.adapter.deleteBlock(t.first); t.check();
    const deletion = t.model.view().proposals.find((view) => view.proposal.action.kind === 'delete_block')!.proposal.id;
    expect(t.model.view().blocks).toHaveLength(2);
    t.adapter.command([{ op: 'accept_proposal', id: deletion }]);
    expect(t.model.view().comments[0].target.state).toBe('hidden');
    t.adapter.history(false); t.check();
    expect(t.model.view().comments[0].target.state).toBe('attached');
    expect(t.model.view().blocks[0].block.kind).toBe('h2');
  } finally { t.close(); }
});

test('pending Bold formatting applies to newly proposed typing and one undo removes the typing group', () => {
  const t = setup();
  try {
    t.setMode('suggesting');
    t.editor.tf.select({ path: [0, 0], offset: 7 });
    t.editor.tf.addMark('bold', true);
    for (const key of 'NEW') { t.editor.tf.insertText(key); t.adapter.flush(); }
    t.check();
    const proposal = t.model.view().proposals[0];
    expect(proposal.text).toBe('NEW');
    expect(proposal.runs.every((run) => run.marks.bold === true)).toBe(true);
    expect(t.model.view().blocks[0].text).toBe('Before TARGET after.');
    t.adapter.history(false); t.check();
    expect(t.model.view().proposals.filter((view) => view.proposal.state === 'open')).toHaveLength(0);
    t.adapter.history(true); t.check();
    expect(t.model.view().proposals.find((view) => view.proposal.id === proposal.proposal.id)?.text).toBe('NEW');
  } finally { t.close(); }
});

import { assignLegacyApi, assignLegacyTransforms, ElementApi, RangeApi, TextApi, type Descendant, type Operation, type Path, type Point, type SlateEditor, type TElement } from 'platejs';
import { DocumentModel, type Command, type DocumentBatch, type DocumentTransaction, type DocumentDraft, type TextPoint, type TextRange, type Attributes, type EditAction, type EditMode } from './document-model';
import type { EditorMode } from './editor-types';
import { sourceDrafts, type SourceBlock } from './source-drafts';
import { wikiLinkLeaves, type WikiLinkNode } from './wiki-link';
import { blockAt, blockAttrs, blockKinds, blockText, flattenBlocks, inlineText, inputBlockId, isBlock, isTextBlock, nodeAt, projectPlate, selectedText, slatePoint, textOffset, type NativeOffset } from './plate-document-projection';

const marks = ['bold', 'italic', 'strikethrough', 'underline', 'superscript', 'subscript', 'code', 'link', 'wikilink'];
const adapters = new WeakMap<SlateEditor, PlateDocumentAdapter>();
export function documentAdapter(editor: SlateEditor) { return adapters.get(editor); }
export interface PlateAdapterOptions {
  changed: (batch: DocumentBatch) => void;
  pending?: (pending: boolean) => void;
  error: (error: unknown) => void;
  selection?: (points: TextPoint[]) => void;
  proposed?: (id: string) => void;
  readOnly?: () => boolean;
  mode?: () => EditorMode;
  author?: () => string;
}

/** Translate explicit Slate operations. Text uses native cursors; structure
 * uses native block IDs. Markdown and text similarity never supply identity. */
export class PlateDocumentAdapter {
  private transaction?: DocumentTransaction;
  private presenting = 0;
  private disposed = false;
  private failedTurn = false;
  private structureChanged = false;
  private proposedMoves = new Set<string>();
  private proposedDeletes = new Set<string>();
  private proposedStructures = new Set<string>();
  private dirty = new Set<string>();
  private dirtyProposals = new Set<string>();
  private formatProposals = new Map<string, { name: string; value: unknown; ranges: TextRange[] }>();
  private scaffolds = new Set<string>();
  private placeholders = new Map<string, { node: TElement; before?: string; after?: string; position: number }>();
  private nativeIds = new Set<string>();
  private savedSelection: TextPoint[] = [];
  private typing?: { block: string; next: number; group: string };
  private continuation?: { id: string; author: string };
  private inputGroup?: string;
  private inputDepth = 0;
  private inputMode?: EditorMode;
  private group?: string;
  private compositionGroup?: { composition: string };
  private insertingProposal?: string;
  private readonly apply: SlateEditor['tf']['apply'];
  private readonly onChange: SlateEditor['api']['onChange'];
  private readonly undo: SlateEditor['tf']['undo'];
  private readonly redo: SlateEditor['tf']['redo'];
  private readonly insertText: SlateEditor['tf']['insertText'];
  private readonly insertFragment: SlateEditor['tf']['insertFragment'];
  private readonly deleteBackward: SlateEditor['tf']['deleteBackward'];
  private readonly deleteForward: SlateEditor['tf']['deleteForward'];
  private readonly deleteFragment: SlateEditor['tf']['deleteFragment'];
  private readonly insertBreak: SlateEditor['tf']['insertBreak'];
  private readonly addMark: SlateEditor['tf']['addMark'];
  private readonly removeMark: SlateEditor['tf']['removeMark'];
  private readonly normalizeNode: SlateEditor['tf']['normalizeNode'];

  constructor(readonly editor: SlateEditor, readonly model: DocumentModel, readonly options: PlateAdapterOptions) {
    this.apply = editor.tf.apply; this.onChange = editor.api.onChange;
    this.undo = editor.tf.undo; this.redo = editor.tf.redo;
    this.insertText = editor.tf.insertText; this.deleteBackward = editor.tf.deleteBackward;
    this.insertFragment = editor.tf.insertFragment;
    this.deleteForward = editor.tf.deleteForward; this.deleteFragment = editor.tf.deleteFragment; this.insertBreak = editor.tf.insertBreak;
    this.addMark = editor.tf.addMark; this.removeMark = editor.tf.removeMark; this.normalizeNode = editor.tf.normalizeNode;
    this.nativeIds = new Set(model.view().blocks.map((view) => view.block.id));
    this.scaffolds.add(inputBlockId(model.view()));
    adapters.set(editor, this);
    editor.tf.apply = (operation) => {
      if (this.presenting || operation.type === 'set_selection') {
        if (!this.presenting && !this.inputDepth) this.endIntent();
        this.apply(operation); return;
      }
      if (options.readOnly?.() || this.failedTurn) return;
      try {
        // Composition and browser reconciliation may emit text operations
        // directly. Capture their explicit selection before Plate changes it.
        // The native edit still decides whether the text becomes a proposal.
        if (this.options.mode?.() === 'suggesting' && (operation.type === 'insert_text' || operation.type === 'remove_text')) {
          const at = textOffset(editor.children, { path: operation.path, offset: operation.offset });
          if (!at.proposal && this.nativeIds.has(at.block)) {
            this.flush();
            this.input(() => {
              const anchor = { path: operation.path, offset: operation.offset };
              const focus = { ...anchor, offset: anchor.offset + (operation.type === 'remove_text' ? operation.text.length : 0) };
              this.present(() => editor.tf.select({ anchor, focus }));
              this.suggestSelection(operation.type === 'insert_text' ? operation.text : '');
            });
            return;
          }
        }
        if (this.applyPlaceholder(operation)) return;
        this.tx();
        this.promoteInput(operation);
        this.translate(operation);
        editor.tf.withoutSaving(() => this.apply(operation));
      } catch (error) { this.fail(error); }
    };
    editor.api.onChange = (options) => {
      if (!this.presenting) this.flush();
      this.onChange(options);
      if (!this.presenting) this.rememberSelection();
      this.failedTurn = false;
    };
    // Plate history must not reinsert copied text on undo. Native undo keeps
    // the original characters, including comments delivered after the edit.
    editor.tf.undo = () => this.history(false); editor.tf.redo = () => this.history(true);
    editor.tf.insertText = (...args) => this.input(() => {
      if (this.suggestSelection(args[0])) return;
      if (this.replaceAcrossContainers(args[0])) return;
      this.insertText(...args);
    });
    editor.tf.insertFragment = (...args) => this.input(() => {
      this.endIntent();
      const previous = this.insertingProposal, selection = editor.selection;
      if (selection) {
        const anchor = textOffset(editor.children, selection.anchor), focus = textOffset(editor.children, selection.focus);
        if (anchor.proposedBlock && focus.proposedBlock && anchor.block === focus.block
          && selectedText(editor.children, selection).every((part) => part.proposal && part.block === anchor.block)) this.insertingProposal = anchor.block;
      }
      try { this.insertFragment(...args); } finally { this.insertingProposal = previous; this.endIntent(); }
    });
    editor.tf.deleteFragment = (...args) => this.input(() => {
      if (this.suggestSelection('')) return;
      if (this.replaceAcrossContainers('')) return;
      this.deleteFragment(...args);
    });
    editor.tf.deleteBackward = (...args) => this.input(() => {
      if (this.suggestDelete(false, args[0])) return;
      this.deleteBackward(...args);
    });
    editor.tf.deleteForward = (...args) => this.input(() => {
      if (this.suggestDelete(true, args[0])) return;
      this.deleteForward(...args);
    });
    editor.tf.insertBreak = (...args) => this.input(() => {
      this.endIntent();
      const selection = this.editor.selection;
      const block = selection && blockAt(this.editor.children, selection.anchor.path);
      if (selection && block && isTextBlock(block.node) && !block.node.quarryProposedBlock && textOffset(this.editor.children, selection.anchor).proposal) { this.editor.tf.insertText('\n'); return; }
      if (this.suggestSelection('\n')) return;
      this.insertBreak(...args);
    });
    editor.tf.addMark = (...args) => { this.endIntent(); if (!this.suggestFormat(args[0], args[1])) this.addMark(...args); };
    editor.tf.removeMark = (...args) => { this.endIntent(); if (!this.suggestFormat(args[0], null)) this.removeMark(...args); };
    editor.tf.normalizeNode = (...args) => {
      if (!args[0][1].length && editor.children.at(-1)?.type !== 'p') {
        let id = inputBlockId(model.view());
        while (editor.children.some((node) => node.id === id)) id += ':';
        this.scaffolds.add(id);
        this.present(() => editor.tf.insertNodes({ id, type: 'p', children: [{ text: '' }] }, { at: [editor.children.length] }));
        return;
      }
      this.normalizeNode(...args);
    };
    // Slate's own transforms call its legacy methods. Plate maintains both
    // surfaces when installing plugins; an attached adapter must do the same.
    assignLegacyTransforms(editor, { apply: editor.tf.apply, undo: editor.tf.undo, redo: editor.tf.redo,
      insertText: editor.tf.insertText, insertFragment: editor.tf.insertFragment, deleteBackward: editor.tf.deleteBackward, deleteForward: editor.tf.deleteForward, deleteFragment: editor.tf.deleteFragment, insertBreak: editor.tf.insertBreak, addMark: editor.tf.addMark, removeMark: editor.tf.removeMark, normalizeNode: editor.tf.normalizeNode });
    assignLegacyApi(editor, { onChange: editor.api.onChange });
  }

  present<T>(action: () => T): T {
    this.presenting++;
    try {
      let result!: T;
      this.editor.tf.withoutSaving(() => { result = action(); });
      return result;
    } finally { this.presenting--; }
  }
  setComposing(composing: boolean) {
    if (composing === Boolean(this.compositionGroup)) return;
    this.flush();
    this.compositionGroup = composing ? { composition: crypto.randomUUID() } : undefined;
    this.typing = undefined;
  }
  preserveInlineSyntax(at: Point, text: string, action: () => void) {
    const block = blockAt(this.editor.children, at.path);
    if (this.options.mode?.() === 'suggesting' && block && !block.node.quarryProposal && !this.nativeIds.has(String(block.node.id))) {
      this.present(action); this.structureChanged = true; return;
    }
    const owner = this.offset(at.path, at.offset);
    const ranges = this.nativeRanges(owner, text.length);
    this.tx().apply([this.content({ op: 'format', ranges, name: 'wikilink', value: true })]);
    this.markDirty(owner);
    this.present(() => {
      action();
      const document = this.tx().view();
      const entries = document.comments.filter(({ comment }) => !comment.deleted && !comment.parent_id && comment.state === 'open')
        .flatMap(({ comment, target }) => target.attachments.map((part) => ({ ...part, kind: 'comment', id: comment.id })))
        .concat(document.proposals.filter(({ proposal }) => proposal.state === 'open')
          .flatMap(({ proposal, target }) => target.attachments.map((part) => ({ ...part, kind: proposal.action.kind, id: proposal.id }))));
      for (const [node, path] of this.editor.api.nodes({ at: [], match: (node) => node.type === 'wikilink' })) {
        const point = textOffset(this.editor.children, { path: [...path, 0], offset: 0 });
        if (point.block !== owner.block || point.proposal !== owner.proposal) continue;
        const key = JSON.stringify(entries.filter((part) => part.owner.id === point.block && part.owner.kind === (point.proposal ? 'proposal' : 'block')
          && part.start < point.offset + inlineText(node).length && part.end > point.offset).map((part) => [part.kind, part.id, part.start, part.end]));
        this.editor.tf.setNodes({ quarryReviewKey: key }, { at: path });
      }
    });
  }
  dispose() {
    this.flush(); this.disposed = true; adapters.delete(this.editor);
    this.editor.tf.apply = this.apply; this.editor.api.onChange = this.onChange;
    this.editor.tf.undo = this.undo; this.editor.tf.redo = this.redo;
    const transforms = { apply: this.apply, undo: this.undo, redo: this.redo, insertText: this.insertText, insertFragment: this.insertFragment,
      deleteBackward: this.deleteBackward, deleteForward: this.deleteForward, deleteFragment: this.deleteFragment, insertBreak: this.insertBreak, addMark: this.addMark, removeMark: this.removeMark, normalizeNode: this.normalizeNode };
    Object.assign(this.editor.tf, transforms); assignLegacyTransforms(this.editor, transforms);
    assignLegacyApi(this.editor, { onChange: this.onChange });
    this.transaction?.abort(); this.transaction = undefined;
  }
  /** Source forms retain the block version they displayed, independently of
   * subsequent renders. A stale draft never overwrites an observed edit. */
  beginSourceEdit(id: string): SourceBlock {
    this.flush();
    const node = flattenBlocks(this.editor.children).find(({ node }) => node.id === id)?.node;
    return this.currentSourceBlock(String(node?.quarryProposedBlock ?? id), node?.quarryProposal as string | undefined);
  }
  private currentSourceBlock(id: string, proposalId?: string): SourceBlock {
    const document = this.model.view();
    const proposal = proposalId && document.proposals.find((view) => view.proposal.id === proposalId);
    const proposed = proposal && proposal.proposal.state === 'open';
    const blocks = !proposalId || proposal && proposal.proposal.state === 'accepted' ? document.blocks : proposed ? proposal.blocks : [];
    const block = blocks.find((view) => view.block.id === id)?.block;
    if (!block) throw new Error('The source block was removed');
    return proposed ? { ...block, proposal: proposalId } : block;
  }
  saveSourceEdit(previous: SourceBlock, attrs: Attributes) {
    this.flush();
    const current = this.currentSourceBlock(previous.id, previous.proposal);
    if (current.kind !== previous.kind || JSON.stringify(current.attrs) !== JSON.stringify(previous.attrs)) {
      throw new Error('The source changed while you were editing. Your draft is kept here.');
    }
    this.updateSourceBlock(current, previous.kind, attrs);
  }
  updateBlock(block: string, kind: string, attrs: Attributes) {
    this.updateSourceBlock(this.beginSourceEdit(block), kind, attrs);
  }
  private updateSourceBlock(block: SourceBlock, kind: string, attrs: Attributes) {
    this.endIntent();
    const mode = this.editMode();
    this.command([this.content({ op: 'set_block', block: block.id, proposal: block.proposal ?? null, kind, attrs }, mode)]);
    if (!block.proposal && mode.kind === 'suggest') this.options.proposed?.(mode.id);
  }
  deleteBlock(block: string) {
    this.endIntent();
    const entry = flattenBlocks(this.editor.children).find(({ node }) => node.id === block);
    if (entry?.node.quarryProposal) {
      this.editor.tf.removeNodes({ at: entry.path }); this.flush(); return;
    }
    const mode = this.editMode();
    this.command([this.content({ op: 'delete_block', block }, mode)]);
    if (mode.kind === 'suggest') this.options.proposed?.(mode.id);
  }
  /** Input intent is separate from both undo timing and HTTP request identity. */
  endIntent() { this.continuation = undefined; this.inputGroup = undefined; this.typing = undefined; }
  /** Input can precede Slate's throttled selectionchange. Read the browser
   * selection before Slate or composition replaces any of the DOM text. */
  captureInputSelection() {
    const element = this.editor.api.toDOMNode(this.editor);
    const selection = this.editor.api.getWindow()?.getSelection();
    if (!element || this.editor.api.findDocumentOrShadowRoot()?.activeElement !== element || !selection
      || !this.editor.api.hasSelectableTarget(selection.anchorNode) || !this.editor.api.hasTarget(selection.focusNode)) return;
    const range = this.editor.api.toSlateRange(selection, { exactMatch: false, suppressThrow: true });
    if (range) this.present(() => this.editor.tf.select(range));
  }
  private input<T>(action: () => T): T {
    const mode = this.options.mode?.() ?? 'editing';
    if (mode !== this.inputMode) this.endIntent();
    this.inputMode = mode; this.inputDepth++;
    try { return action(); } finally { this.inputDepth--; }
  }
  private editMode(id?: string): EditMode {
    return this.options.mode?.() === 'suggesting' ? { kind: 'suggest', id: id ?? crypto.randomUUID(), author: this.options.author?.() ?? 'user' } : { kind: 'direct' };
  }
  private content(action: EditAction, mode = this.editMode()): Command { return { op: 'edit', mode, action }; }
  private tx() {
    if (!this.transaction) { this.transaction = this.model.beginTransaction(); this.options.pending?.(true); }
    return this.transaction;
  }
  private applyPlaceholder(op: Operation): boolean {
    if (op.type === 'set_selection') return false;
    const node = op.type === 'insert_node' ? op.node : nodeAt(this.editor.children, op.path);
    if (!ElementApi.isElement(node) || node.type !== 'placeholder' || op.path.length !== 1) return false;
    const id = String(node.id || crypto.randomUUID());
    if (op.type === 'insert_node') {
      node.id = id;
      this.placeholders.set(id, { node, position: op.path[0],
        before: this.editor.children[op.path[0]]?.id as string | undefined,
        after: this.editor.children[op.path[0] - 1]?.id as string | undefined });
    } else if (op.type === 'remove_node') this.placeholders.delete(id);
    this.editor.tf.withoutSaving(() => this.apply(op));
    const item = this.placeholders.get(id);
    if (item) {
      const current = this.editor.children.find((node) => node.id === id);
      if (current) item.node = current;
    }
    return true;
  }
  private promoteInput(operation: Operation) {
    if (operation.type === 'insert_node' || operation.type === 'set_selection' || operation.type === 'remove_node') return;
    const block = blockAt(this.editor.children, operation.path);
    if (!block || block.node.quarryProposal || this.nativeIds.has(String(block.node.id)) || !this.scaffolds.has(String(block.node.id))) return;
    // The virtual input is a local convenience. Every promotion gets a fresh
    // durable ID, including typing again after undo hid an earlier paragraph.
    const id = crypto.randomUUID();
    this.present(() => this.editor.tf.apply({ type: 'set_node', path: block.path, properties: { id: block.node.id }, newProperties: { id } }));
  }
  private suggestSelection(text: string): boolean {
    if (this.options.mode?.() !== 'suggesting' || !this.editor.selection || this.presenting) return false;
    const selection = this.editor.selection;
    const anchor = textOffset(this.editor.children, selection.anchor), focus = textOffset(this.editor.children, selection.focus);
    if (anchor.proposal && focus.proposal && anchor.block === focus.block && anchor.proposedBlock === focus.proposedBlock) return false;
    try {
      if (this.options.readOnly?.()) return true;
      this.flush();
      const [from] = RangeApi.edges(selection);
      const start = textOffset(this.editor.children, from);
      const selected = selectedText(this.editor.children, selection);
      if (anchor.proposal || focus.proposal || selected.some((part) => part.proposal)) throw new Error('Accept or reject the intervening suggestion before replacing this selection');
      if (!text && !selected.length) return true;
      const owner = blockAt(this.editor.children, from.path)!;
      const input = !this.nativeIds.has(start.block);
      const author = this.options.author?.() ?? 'user';
      const continuation = !input && this.continuation?.author === author ? this.continuation : undefined;
      const id = continuation?.id ?? crypto.randomUUID();
      const group = continuation ? this.inputGroup : crypto.randomUUID();
      const mode: EditMode = { kind: continuation ? 'continue' : 'suggest', id, author };
      const formatting = { ...this.formatting(from.path), ...this.editor.api.marks() };
      this.command((draft) => {
        if (input) {
          draft.apply([this.content({ op: 'insert_blocks', parent: null, before: null,
            blocks: [{ id: crypto.randomUUID(), kind: owner.node.type, attrs: blockAttrs(owner.node), parent: null, position: 0, text }] }, mode)]);
        } else {
          draft.apply([this.content({ op: 'replace_text',
            at: draft.model.point(start.block, start.offset), ranges: selected.flatMap((part) => draft.model.selection(part.block, part.offset, part.end)), text }, mode)]);
        }
        if (text.length) {
          const ranges = draft.model.selectionFor({ kind: 'proposal', id }, 0, text.length);
          draft.apply(marks.filter((name) => formatting[name] != null).map((name) => this.content({ op: 'format', ranges, name, value: formatting[name] })));
        }
      }, undefined, group);
      this.continuation = { id, author }; this.inputGroup = group;
      this.typing = text && group ? { block: id, next: text.length, group } : undefined;
      const point = text ? slatePoint(this.editor.children, id, text.length, true) : slatePoint(this.editor.children, start.block, start.offset);
      if (point) this.present(() => this.editor.tf.select(point));
      this.options.proposed?.(id);
    } catch (error) { this.endIntent(); this.options.error(error); }
    return true;
  }
  private replaceAcrossContainers(text: string): boolean {
    const selection = this.editor.selection;
    if (this.presenting || !selection || RangeApi.isCollapsed(selection)) return false;
    const [start, end] = RangeApi.edges(selection);
    const first = blockAt(this.editor.children, start.path), last = blockAt(this.editor.children, end.path);
    if (!first || !last || first.path.slice(0, -1).join('.') === last.path.slice(0, -1).join('.')) return false;
    try {
      if (this.options.readOnly?.()) return true;
      this.flush();
      const from = textOffset(this.editor.children, start);
      const selected = selectedText(this.editor.children, selection);
      if (selected.some((part) => part.proposal)) throw new Error('Accept or reject the intervening suggestion before replacing this selection');
      // Delete each selected text span in place. Keep Slate's ordinary input
      // operations so composition can continue in the same DOM text node.
      this.editor.tf.withoutNormalizing(() => {
        for (const part of selected.reverse()) {
          const anchor = slatePoint(this.editor.children, part.block, part.offset)!;
          const focus = slatePoint(this.editor.children, part.block, part.end)!;
          this.editor.tf.delete({ at: { anchor, focus } });
        }
        const point = slatePoint(this.editor.children, from.block, from.offset);
        if (point) this.editor.tf.select(point);
        if (text) this.insertText(text);
      });
    } catch (error) { this.endIntent(); this.options.error(error); }
    return true;
  }
  private suggestFormat(name: string, value: unknown): boolean {
    if (this.options.mode?.() !== 'suggesting' || !this.editor.selection || this.presenting) return false;
    const selected = selectedText(this.editor.children, this.editor.selection);
    if (!selected.some((part) => !part.proposal)) return false;
    try {
      this.flush();
      const ranges = selected.flatMap((part) => this.model.selectionFor({ kind: part.proposal ? 'proposal' : 'block', id: part.block }, part.offset, part.end));
      const id = crypto.randomUUID();
      this.command([this.content({ op: 'format', ranges, name, value }, this.editMode(id))]);
      this.options.proposed?.(id);
    } catch (error) { this.endIntent(); this.options.error(error); }
    return true;
  }
  private suggestDelete(forward: boolean, options: Parameters<SlateEditor['tf']['deleteBackward']>[0]): boolean {
    if (this.options.mode?.() !== 'suggesting' || !this.editor.selection || this.presenting) return false;
    if (this.options.readOnly?.()) return true;
    let { anchor, focus } = this.editor.selection;
    if (anchor.path.join('.') !== focus.path.join('.') || anchor.offset !== focus.offset) return this.suggestSelection('');
    try {
      this.flush();
      let at = textOffset(this.editor.children, anchor);
      if (at.proposal) {
        if (at.proposedBlock) return false;
        const view = this.model.view().proposals.find((view) => view.proposal.id === at.block)!;
        if (forward ? at.offset < view.text.length : at.offset > 0) return false;
        const location = this.model.locate(view.proposal.action.at as TextPoint);
        if (!location || location.owner.kind !== 'block') return true;
        at = { block: location.owner.id, offset: location.offset };
      }
      // Struck-through text remains canonical until acceptance. Repeated
      // delete keys move over it instead of proposing the same character again.
      const deletions = this.deletions(at.block);
      let offset = at.offset;
      for (;;) {
        const part = deletions.find((part) => forward ? part.start <= offset && offset < part.end : part.start < offset && offset <= part.end);
        if (!part) break;
        offset = forward ? part.end : part.start;
      }
      anchor = slatePoint(this.editor.children, at.block, offset)!;
      if (forward) {
        // At an inline boundary the same canonical offset has a point on
        // either side. Forward deletion starts after inserted proposal text.
        const owner = blockAt(this.editor.children, anchor.path)!;
        for (const [node, path] of this.editor.api.nodes({ at: owner.path, match: TextApi.isText })) {
          if (!TextApi.isText(node)) continue;
          const start = textOffset(this.editor.children, { path, offset: 0 });
          if (!start.proposal && start.block === at.block && start.offset <= offset && offset <= start.offset + node.text.length) {
            anchor = { path, offset: offset - start.offset };
          }
        }
      }
      const next = forward ? this.editor.api.after(anchor, { unit: options }) : this.editor.api.before(anchor, { unit: options });
      if (!next) return true;
      this.present(() => this.editor.tf.select({ anchor, focus: next }));
      return this.suggestSelection('');
    } catch (error) { this.options.error(error); return true; }
  }
  private deletions(block: string) {
    return this.model.view().proposals.flatMap((view) => {
      if (view.proposal.state !== 'open' || view.proposal.action.kind !== 'text' || view.text || view.acceptance_error) return [];
      const parts = view.target.attachments;
      if (!parts.length || parts.some((part) => part.owner.kind !== 'block' || part.owner.id !== block)) return [];
      const start = Math.min(...parts.map((part) => part.start)), end = Math.max(...parts.map((part) => part.end));
      if (end - start !== parts.reduce((sum, part) => sum + part.end - part.start, 0)) return [];
      return [{ id: view.proposal.id, author: view.proposal.author, start, end }];
    });
  }
  private position(path: Path) {
    const children = path.length > 1 ? (nodeAt(this.editor.children, path.slice(0, -1)) as TElement).children : this.editor.children;
    return children.slice(0, path.at(-1)!).filter((node) => isBlock(node) && !node.quarryProposal && !(this.scaffolds.has(String(node.id)) && !blockText(node))).length;
  }
  private ensure(block: TElement, path: Path) {
    const id = String(block.id);
    if (this.nativeIds.has(id)) return;
    const parent = path.length > 1 ? blockAt(this.editor.children, path.slice(0, -1)) : undefined;
    if (parent) this.ensure(parent.node, parent.path);
    this.tx().apply([this.content({ op: 'insert_block', block: { id, kind: block.type, attrs: blockAttrs(block),
      parent: parent ? String(parent.node.id) : null, position: this.position(path), text: isTextBlock(block) ? blockText(block) : '' } })]);
    this.nativeIds.add(id); this.scaffolds.delete(id); this.structureChanged = true;
  }
  private offset(path: Path, offset = 0) {
    const at = textOffset(this.editor.children, { path, offset });
    if (at.proposal) return at;
    const owner = blockAt(this.editor.children, path);
    if (!owner) throw new Error('Missing block for editor text');
    this.ensure(owner.node, owner.path);
    return at;
  }
  private nativePoint(at: NativeOffset) { return at.proposedBlock ? this.tx().proposedBlockPoint(at.block, at.proposedBlock, at.blockOffset!) : at.proposal ? this.tx().proposalPoint(at.block, at.offset) : this.tx().point(at.block, at.offset); }
  private nativeRanges(at: NativeOffset, length: number) { return at.proposedBlock ? this.tx().proposedBlockSelection(at.block, at.proposedBlock, at.blockOffset!, at.blockOffset! + length) : at.proposal ? this.tx().proposalSelection(at.block, at.offset, at.offset + length) : this.tx().selection(at.block, at.offset, at.offset + length); }
  private markDirty(at: NativeOffset) { (at.proposal ? this.dirtyProposals : this.dirty).add(at.block); }
  private inlineOffset(path: Path) {
    const node = nodeAt(this.editor.children, path);
    let first = path;
    let current = node;
    while (ElementApi.isElement(current) && current.children.length) { first = [...first, 0]; current = current.children[0]; }
    return this.offset(first);
  }
  private formatting(path: Path): Record<string, unknown> {
    const node = nodeAt(this.editor.children, path);
    const value: Record<string, unknown> = {};
    if (TextApi.isText(node)) for (const name of marks) if (node[name] !== undefined) value[name] = node[name];
    for (let length = path.length - 1; length > 0; length--) {
      const parent = nodeAt(this.editor.children, path.slice(0, length));
      if (ElementApi.isElement(parent) && parent.type === 'a') value.link = parent.url;
    }
    return value;
  }
  private exactMarks(block: string, start: number, length: number, desired: Record<string, unknown>, proposal = false, proposedBlock?: string) {
    if (!length) return;
    let offset = 0;
    const view = proposal ? this.tx().view().proposals.find((view) => view.proposal.id === block)! : undefined;
    const runs = view ? proposedBlock ? view.blocks.find((view) => view.block.id === proposedBlock)!.runs : view.runs : this.tx().block(block).runs;
    const affected = runs.filter((run) => { const at = offset; offset += run.text.length; return at < start + length && offset > start; });
    const changed = marks.filter((name) => affected.some((run) => (run.marks[name] ?? null) !== (desired[name] ?? null)));
    if (changed.length) {
      const ranges = this.nativeRanges({ block, offset: start, proposal, proposedBlock, blockOffset: start }, length);
      if (this.options.mode?.() === 'suggesting' && !proposal) {
        for (const name of changed) {
          const value = desired[name] ?? null, key = JSON.stringify([name, value]);
          const change = this.formatProposals.get(key) ?? { name, value, ranges: [] };
          change.ranges.push(...ranges); this.formatProposals.set(key, change);
        }
        return;
      }
      this.tx().apply(changed.map((name) => (this.content({ op: 'format', ranges, name, value: desired[name] ?? null }))));
    }
  }
  private translate(op: Operation) {
    if (op.type === 'set_selection') return;
    if (op.type === 'merge_node') {
      const right = nodeAt(this.editor.children, op.path);
      const leftPath = [...op.path]; leftPath[leftPath.length - 1]--;
      const left = nodeAt(this.editor.children, leftPath);
      // A paste can join a new block to existing text before its turn ends.
      // Give both blocks native sources before transferring their characters.
      if (isBlock(right) && isBlock(left) && right.quarryProposal && right.quarryProposal === left.quarryProposal
        && Boolean(right.quarryNewProposedBlock) !== Boolean(left.quarryNewProposedBlock)) this.reconcileProposedStructures();
    }
    const pending = op.type === 'insert_node' ? blockAt(this.editor.children, op.path.slice(0, -1)) : blockAt(this.editor.children, op.path);
    if (pending?.node.quarryNewProposedBlock) {
      if (op.type === 'split_node' && isBlock(nodeAt(this.editor.children, op.path))) {
        const id = crypto.randomUUID(); op.properties.id = id; op.properties.quarryProposedBlock = id;
      }
      if (op.type === 'insert_node' && isBlock(op.node)) this.assignProposedTree(op.node, String(pending.node.quarryProposal));
      this.proposedStructures.add(String(pending.node.quarryProposal)); return;
    }
    // Plate may normalize or format a newly inserted subtree before its
    // operation batch ends. Its native proposal is created from that final
    // subtree; no canonical block is created during these local operations.
    if (this.options.mode?.() === 'suggesting' && op.type !== 'insert_node') {
      const owner = blockAt(this.editor.children, op.path);
      if (owner && !owner.node.quarryProposal && !this.nativeIds.has(String(owner.node.id))) {
        if (op.type === 'split_node' && isBlock(nodeAt(this.editor.children, op.path))) op.properties.id = crypto.randomUUID();
        this.structureChanged = true; return;
      }
    }
    if (op.type === 'insert_text' || op.type === 'remove_text') {
      const at = this.offset(op.path, op.offset); this.markDirty(at);
      if (op.type === 'insert_text') {
        this.group = this.typing?.block === at.block && this.typing.next === at.offset ? this.typing.group : crypto.randomUUID();
        this.typing = { block: at.block, next: at.offset + op.text.length, group: this.group };
        this.tx().apply([this.content({ op: 'insert_text', at: this.nativePoint(at), text: op.text })]);
        this.exactMarks(at.block, at.blockOffset ?? at.offset, op.text.length, this.formatting(op.path), at.proposal, at.proposedBlock);
      } else {
        this.typing = undefined; this.group = undefined;
        this.tx().apply([this.content({ op: 'delete_text', ranges: this.nativeRanges(at, op.text.length) })]);
      }
      return;
    }
    this.typing = undefined; this.group = undefined;
    if (op.type === 'insert_node') {
      if (isBlock(op.node)) {
        const assign = (node: Descendant) => {
          if (!isBlock(node)) return;
          if (!node.id || this.nativeIds.has(String(node.id))) node.id = crypto.randomUUID();
          if (blockKinds.get(node.type)!.content === 'container') node.children.forEach(assign);
        };
        const parent = op.path.length > 1 && blockAt(this.editor.children, op.path.slice(0, -1));
        assign(op.node);
        const proposal = parent && parent.node.quarryProposal ? String(parent.node.quarryProposal) : this.insertingProposal;
        if (proposal) {
          this.assignProposedTree(op.node, proposal); this.proposedStructures.add(proposal); return;
        }
        if (this.options.mode?.() === 'suggesting') { this.structureChanged = true; return; }
        this.insertTree(op.node, op.path);
      } else {
        const owner = blockAt(this.editor.children, op.path.slice(0, -1));
        if (this.options.mode?.() === 'suggesting' && owner && !owner.node.quarryProposal && !this.nativeIds.has(String(owner.node.id))) {
          this.structureChanged = true; return;
        }
        const at = this.inlineOffset(op.path.slice(0, -1));
        const parent = nodeAt(this.editor.children, op.path.slice(0, -1));
        if (!ElementApi.isElement(parent)) throw new Error('Inline insertion requires a parent');
        const preceding = parent.children.slice(0, op.path.at(-1)!).reduce((sum, node) => sum + inlineText(node).length, 0);
        at.offset += preceding;
        if (at.blockOffset !== undefined) at.blockOffset += preceding;
        const text = inlineText(op.node);
        if (text) {
          if (this.options.mode?.() === 'suggesting' && !at.proposal) throw new Error('Insert inline content through a text selection in Suggesting mode');
          this.tx().apply([this.content({ op: 'insert_text', at: this.nativePoint(at), text })]);
          this.markDirty(at);
          const desired = TextApi.isText(op.node) ? op.node : op.node.type === 'a' ? { link: op.node.url } : {};
          this.exactMarks(at.block, at.blockOffset ?? at.offset, text.length, desired, at.proposal, at.proposedBlock);
        }
      }
      return;
    }
    const node = nodeAt(this.editor.children, op.path);
    if (isBlock(node) && node.quarryProposal) {
      if (op.type === 'remove_node' || op.type === 'move_node') { this.proposedStructures.add(String(node.quarryProposal)); return; }
      if (op.type === 'split_node') {
        const id = crypto.randomUUID();
        op.properties.id = id; op.properties.quarryProposedBlock = id;
        if (isTextBlock(node)) {
          const offset = node.children.slice(0, op.position).reduce((sum, child) => sum + inlineText(child).length, 0);
          const proposal = String(node.quarryProposal), block = String(node.quarryProposedBlock);
          this.tx().apply([this.content({ op: 'split_block', proposal, block, at: this.tx().proposedBlockPoint(proposal, block, offset), new_block: id })]);
        } else op.properties.quarryNewProposedBlock = true;
        this.proposedStructures.add(String(node.quarryProposal)); return;
      }
      if (op.type === 'merge_node') {
        const leftPath = [...op.path]; leftPath[leftPath.length - 1]--;
        const left = nodeAt(this.editor.children, leftPath);
        if (!isBlock(left) || left.quarryProposal !== node.quarryProposal) throw new Error('Join blocks within their suggestion');
        if (isTextBlock(node) && isTextBlock(left)) {
          this.tx().apply([this.content({ op: 'join_blocks', proposal: String(node.quarryProposal), left: String(left.quarryProposedBlock), right: String(node.quarryProposedBlock) })]);
        } else if (isTextBlock(node) || isTextBlock(left)) throw new Error('Join proposed blocks with compatible content');
        this.proposedStructures.add(String(node.quarryProposal)); return;
      }
      if (op.type !== 'set_node') throw new Error('Change the proposed text before accepting its structure');
      if (op.newProperties.id && op.newProperties.id !== node.id) op.newProperties.id = node.id;
      const next = { ...node, ...op.newProperties };
      this.tx().apply([this.content({ op: 'set_block', proposal: String(node.quarryProposal), block: String(node.quarryProposedBlock), kind: next.type, attrs: blockAttrs(next) })]);
      return;
    }
    if (this.options.mode?.() === 'suggesting' && isBlock(node) && op.type === 'move_node') {
      this.proposedMoves.add(String(node.id)); this.structureChanged = true; return;
    }
    if (op.type === 'remove_node') {
      if (isBlock(node)) {
        if (this.options.mode?.() === 'suggesting') this.proposedDeletes.add(String(node.id));
        this.structureChanged = true; return;
      }
      const at = this.inlineOffset(op.path); const text = ElementApi.isElement(node) && node.type === 'quarry_proposal' ? node.children.map(inlineText).join('') : inlineText(node);
      if (text && this.options.mode?.() === 'suggesting' && !at.proposal) throw new Error('Remove inline content through a text selection in Suggesting mode');
      if (text) this.tx().apply([this.content({ op: 'delete_text', ranges: this.nativeRanges(at, text.length) })]);
      this.markDirty(at); return;
    }
    if (op.type === 'set_node') {
      if (TextApi.isText(node) && op.path.length > 1) {
        const parentPath = op.path.slice(0, -1), parent = nodeAt(this.editor.children, parentPath);
        if (ElementApi.isElement(parent) && parent.type === 'wikilink') {
          const changes = Object.fromEntries(Object.entries(op.newProperties).filter(([name]) => marks.includes(name)));
          if (Object.keys(changes).length) {
            const leaves = wikiLinkLeaves(parent as WikiLinkNode).map((leaf) => ({ ...leaf, ...changes }));
            this.present(() => this.editor.tf.setNodes({ quarryLeaves: leaves }, { at: parentPath }));
            this.markDirty(this.inlineOffset(parentPath));
          }
          return;
        }
      }
      if (isBlock(node)) {
        if (op.newProperties.id && op.newProperties.id !== node.id) op.newProperties.id = node.id;
        this.structureChanged = true; return;
      }
      const at = this.inlineOffset(op.path); const length = inlineText(node).length;
      if (!length) return;
      if (this.options.mode?.() === 'suggesting' && !at.proposal) { this.markDirty(at); return; }
      const desired = { ...this.formatting(op.path), ...op.newProperties };
      if (ElementApi.isElement(node) && node.type === 'a') desired.link = op.newProperties.url ?? null;
      this.exactMarks(at.block, at.blockOffset ?? at.offset, length, desired, at.proposal, at.proposedBlock); this.markDirty(at); return;
    }
    if (op.type === 'split_node') {
      if (!isBlock(node)) return;
      this.ensure(node, op.path);
      const id = crypto.randomUUID(); op.properties.id = id;
      if (isTextBlock(node)) {
        const offset = node.children.slice(0, op.position).reduce((sum, child) => sum + inlineText(child).length, 0);
        this.tx().apply([this.content({ op: 'split_block', block: String(node.id), at: this.tx().point(String(node.id), offset), new_block: id })]);
        this.nativeIds.add(id); this.dirty.add(String(node.id)); this.dirty.add(id);
      } else {
        const parent = this.tx().block(String(node.id)).block.parent;
        this.tx().apply([this.content({ op: 'insert_block', block: { id, kind: node.type, attrs: blockAttrs(node), parent, position: op.path.at(-1)! + 1, text: '' } }),
          ...node.children.slice(op.position).filter(isBlock).map((child): Command => (this.content({ op: 'move_block', block: String(child.id), parent: id, before: null })))]);
        this.nativeIds.add(id);
      }
      this.structureChanged = true; return;
    }
    if (op.type === 'merge_node') {
      if (!isBlock(node)) return;
      const leftPath = [...op.path]; leftPath[leftPath.length - 1]--;
      const left = nodeAt(this.editor.children, leftPath);
      if (!isBlock(left)) throw new Error('Block merge requires blocks');
      this.ensure(left, leftPath); this.ensure(node, op.path);
      if (isTextBlock(node) && isTextBlock(left)) {
        this.tx().apply([this.content({ op: 'join_blocks', left: String(left.id), right: String(node.id) })]);
        this.dirty.add(String(left.id)); this.nativeIds.delete(String(node.id));
      } else this.tx().apply([...node.children.filter(isBlock).map((child): Command => (this.content({ op: 'move_block', block: String(child.id), parent: String(left.id), before: null }))), this.content({ op: 'delete_block', block: String(node.id) })]);
      this.structureChanged = true; return;
    }
    if (op.type === 'move_node') {
      if (isBlock(node)) { this.structureChanged = true; return; }
      // Wrapping/unwrapping a link moves leaves without moving characters.
      // Cross-block leaf moves are explicit ownership transfers.
      const from = this.inlineOffset(op.path);
      const parentPath = op.newPath.slice(0, -1); const parent = nodeAt(this.editor.children, parentPath);
      if (!ElementApi.isElement(parent)) throw new Error('Invalid inline move');
      const to = this.inlineOffset(parentPath);
      to.offset += parent.children.slice(0, op.newPath.at(-1)!).reduce((sum, child) => sum + inlineText(child).length, 0);
      const length = inlineText(node).length;
      this.markDirty(from); this.markDirty(to);
      if (from.block === to.block && (to.offset === from.offset || to.offset === from.offset + length)) return;
      if (length) {
        if (from.proposal || to.proposal) throw new Error('Suggestion text cannot move to another text owner');
        this.moveText(from, to, length);
      }
    }
  }
  private assignProposedTree(node: TElement, proposal: string) {
    const id = crypto.randomUUID(); node.id = id; node.quarryProposedBlock = id;
    node.quarryProposal = proposal; node.quarryNewProposedBlock = true;
    if (blockKinds.get(node.type)!.content === 'container') for (const child of node.children) if (isBlock(child)) this.assignProposedTree(child, proposal);
  }
  private insertTree(node: TElement, path: Path, parentId?: string) {
    const parent = parentId ?? (path.length > 1 ? String(blockAt(this.editor.children, path.slice(0, -1))?.node.id) : null);
    this.tx().apply([this.content({ op: 'insert_block', block: { id: String(node.id), kind: node.type, attrs: blockAttrs(node), parent,
      position: parentId ? path.at(-1)! : this.position(path), text: isTextBlock(node) ? blockText(node) : '' } })]);
    this.nativeIds.add(String(node.id)); this.structureChanged = true;
    if (blockKinds.get(node.type)!.content === 'container') node.children.forEach((child, index) => { if (isBlock(child)) this.insertTree(child, [...path, index], String(node.id)); });
    else if (isTextBlock(node)) {
      this.dirty.add(String(node.id));
      let offset = 0;
      const format = (child: Descendant, link?: unknown) => {
        if (TextApi.isText(child)) { this.exactMarks(String(node.id), offset, child.text.length, { ...child, ...(link ? { link } : {}) }); offset += child.text.length; }
        else if (child.type === 'quarry_proposal') return;
        else if (child.type === 'wikilink') {
          for (const leaf of wikiLinkLeaves(child as WikiLinkNode)) {
            this.exactMarks(String(node.id), offset, leaf.text.length, { ...leaf, wikilink: true }); offset += leaf.text.length;
          }
        } else child.children.forEach((leaf) => format(leaf, child.type === 'a' ? child.url : link));
      };
      node.children.forEach((child) => format(child));
    }
  }
  private moveText(from: { block: string; offset: number }, to: { block: string; offset: number }, length: number) {
    const tx = this.tx(); const span = crypto.randomUUID(), tail = crypto.randomUUID(), destinationTail = crypto.randomUUID();
    const destination = tx.point(to.block, to.offset);
    tx.apply([this.content({ op: 'split_block', block: from.block, at: tx.point(from.block, from.offset), new_block: span })]);
    tx.apply([this.content({ op: 'split_block', block: span, at: tx.point(span, length), new_block: tail }), this.content({ op: 'join_blocks', left: from.block, right: tail })]);
    tx.apply([this.content({ op: 'split_block', block: to.block, at: destination, new_block: destinationTail }),
      this.content({ op: 'move_block', block: span, parent: tx.block(to.block).block.parent, before: destinationTail }),
      this.content({ op: 'join_blocks', left: to.block, right: span }), this.content({ op: 'join_blocks', left: to.block, right: destinationTail })]);
    this.dirty.add(from.block); this.dirty.add(to.block); this.structureChanged = true;
  }
  flush() {
    const tx = this.transaction;
    if (!tx || this.presenting || this.disposed) return;
    try {
      const updatedProposals = this.reconcileProposedStructures();
      const proposed = this.structureChanged ? this.reconcileStructure() : [];
      for (const { node } of flattenBlocks(this.editor.children)) {
        if (!this.dirty.has(String(node.id)) || !isTextBlock(node)) continue;
        if (tx.block(String(node.id)).text !== blockText(node)) throw new Error('Slate text does not match its native commands');
        // Link wrapping moves existing leaves. Reconcile their final marks
        // without deleting or reinserting the native characters.
        let offset = 0;
        const format = (child: Descendant, link?: unknown) => {
          if (TextApi.isText(child)) {
            this.exactMarks(String(node.id), offset, child.text.length, { ...child, ...(link ? { link } : {}) });
            offset += child.text.length;
          } else if (child.type === 'quarry_proposal') return;
          else if (child.type === 'wikilink') {
            for (const leaf of wikiLinkLeaves(child as WikiLinkNode)) {
              this.exactMarks(String(node.id), offset, leaf.text.length, { ...leaf, wikilink: true }); offset += leaf.text.length;
            }
          } else child.children.forEach((leaf) => format(leaf, child.type === 'a' ? child.url : link));
        };
        node.children.forEach((child) => format(child));
      }
      this.flushProposals();
      for (const change of this.formatProposals.values()) {
        const id = crypto.randomUUID(); proposed.push(id);
        tx.apply([this.content({ op: 'format', ...change }, this.editMode(id))]);
      }
      this.formatProposals.clear();
      this.transaction = undefined; this.dirty.clear(); this.dirtyProposals.clear(); this.structureChanged = false; this.proposedMoves.clear(); this.proposedDeletes.clear(); this.proposedStructures.clear();
      if (!tx.request.commands.length) { tx.abort(); return; }
      const batch = tx.finish(this.compositionGroup ?? this.group); this.options.changed(batch);
      this.nativeIds = new Set(this.model.view().blocks.map((view) => view.block.id));
      if (proposed.length || updatedProposals) {
        this.rememberSelection();
        this.refresh();
        if (proposed.length) this.options.proposed?.(proposed.at(-1)!);
      }
    } catch (error) { this.transaction = tx; this.fail(error); }
    finally { this.options.pending?.(false); }
  }
  private reconcileProposedStructures(): boolean {
    if (!this.proposedStructures.size) return false;
    const all = flattenBlocks(this.editor.children), byId = new Map(all.map((item) => [String(item.node.id), item]));
    for (const id of this.proposedStructures) {
      const view = this.tx().view().proposals.find((view) => view.proposal.id === id)!;
      if (view.proposal.action.kind !== 'insert_blocks') throw new Error('This suggestion does not contain blocks');
      const tree = all.filter(({ node }) => node.quarryProposal === id);
      if (!tree.length) { this.tx().apply([{ op: 'reject_proposal', id }]); this.dirtyProposals.delete(id); continue; }
      const existing = new Set(view.blocks.map(({ block }) => block.id));
      const positions = new Map<string | null, number>();
      const placements = tree.map(({ node, parent, path }) => {
        const container = parent && byId.get(parent)?.node;
        const internalParent = container && container.quarryProposal === id ? String(container.quarryProposedBlock) : null;
        if (!internalParent) {
          if (parent !== view.proposal.action.parent) throw new Error('Move proposed blocks within their suggestion');
          const siblings = path.length > 1 ? (nodeAt(this.editor.children, path.slice(0, -1)) as TElement).children : this.editor.children;
          const next = siblings.slice(path.at(-1)! + 1).find((node) => isBlock(node) && !node.quarryProposal && !this.scaffolds.has(String(node.id)));
          if ((next?.id ?? null) !== view.proposal.action.before) throw new Error('Move proposed blocks within their suggestion');
        }
        const position = positions.get(internalParent) ?? 0; positions.set(internalParent, position + 1);
        const block = String(node.quarryProposedBlock);
        return existing.has(block) ? { kind: 'existing' as const, block, parent: internalParent, position }
          : { kind: 'new' as const, block: { id: block, parent: internalParent, position, kind: node.type, attrs: blockAttrs(node), text: isTextBlock(node) ? blockText(node) : '' } };
      });
      this.tx().apply([this.content({ op: 'set_proposed_structure', proposal: id, blocks: placements })]);
      const updated = this.tx().view().proposals.find((view) => view.proposal.id === id)!;
      this.present(() => tree.forEach(({ node, path }) => {
        const index = updated.blocks.findIndex(({ block }) => block.id === node.quarryProposedBlock);
        this.editor.tf.setNodes({ quarryNewProposedBlock: null, quarryProposalOrder: index, quarrySources: updated.blocks[index].block.segments.map((segment) => segment.source) }, { at: path });
      }));
      this.dirtyProposals.add(id);
    }
    return true;
  }
  private flushProposals() {
    if (!this.dirtyProposals.size) return;
    const visit = (nodes: Descendant[]) => {
      for (const node of nodes) {
        if (!ElementApi.isElement(node)) continue;
        if ((node.type === 'quarry_proposal' || node.quarryProposedBlock && isTextBlock(node)) && this.dirtyProposals.has(String(node.quarryProposal))) {
          const id = String(node.quarryProposal);
          const native = this.tx().view().proposals.find((view) => view.proposal.id === id)!;
          const text = node.quarryProposedBlock ? native.blocks.find((view) => view.block.id === node.quarryProposedBlock)!.text : native.text;
          if (text !== node.children.map(inlineText).join('')) throw new Error('Suggestion text does not match its native commands');
          let offset = 0;
          if (node.quarryProposedBlock) for (const view of native.blocks) {
            if (view.block.id === node.quarryProposedBlock) break;
            offset += view.text.length;
          }
          const format = (children: Descendant[], link?: unknown) => children.forEach((child) => {
            if (TextApi.isText(child)) {
              this.exactMarks(id, offset, child.text.length, { ...child, ...(link ? { link } : {}) }, true); offset += child.text.length;
            } else if (child.type === 'wikilink') {
              for (const leaf of wikiLinkLeaves(child as WikiLinkNode)) {
                this.exactMarks(id, offset, leaf.text.length, { ...leaf, wikilink: true }, true); offset += leaf.text.length;
              }
            } else format(child.children, child.type === 'a' ? child.url : link);
          });
          format(node.children);
        } else visit(node.children);
      }
    };
    visit(this.editor.children);
  }
  private reconcileStructure(): string[] {
    const tx = this.tx();
    const blocks = flattenBlocks(this.editor.children, true).filter(({ node }) => !(this.scaffolds.has(String(node.id)) && !blockText(node)));
    if (this.options.mode?.() === 'suggesting') {
      const originals = tx.view().blocks, originalIds = new Set(originals.map((view) => view.block.id));
      const removed = (id: string): boolean => {
        const block = originals.find((view) => view.block.id === id)?.block;
        return this.proposedDeletes.has(id) || Boolean(block?.parent && removed(block.parent));
      };
      const retained = blocks.filter(({ node }) => originalIds.has(String(node.id)));
      const stationary = retained.filter(({ node }) => !this.proposedMoves.has(String(node.id)));
      const expected = originals.filter(({ block }) => !this.proposedMoves.has(block.id) && !removed(block.id));
      const order = (items: Array<{ id: string; parent: string | null }>) => {
        const parents = new Map<string, string[]>();
        for (const { id, parent } of items) { const key = parent ?? ''; const ids = parents.get(key) ?? []; ids.push(id); parents.set(key, ids); }
        return JSON.stringify([...parents].sort(([a], [b]) => a.localeCompare(b)));
      };
      if (originals.filter(({ block }) => !removed(block.id)).length !== retained.length || stationary.some(({ node, parent }) => originals.find((view) => view.block.id === node.id)?.block.parent !== parent)
        || order(stationary.map(({ node, parent }) => ({ id: String(node.id), parent }))) !== order(expected.map(({ block }) => block))) {
        throw new Error('This structure change requires a block proposal');
      }
      const ids: string[] = [];
      for (const block of this.proposedDeletes) {
        const parent = originals.find((view) => view.block.id === block)?.block.parent;
        if (parent && removed(parent)) continue;
        const id = crypto.randomUUID(); ids.push(id);
        tx.apply([this.content({ op: 'delete_block', block }, this.editMode(id))]);
      }
      for (const { node } of retained) {
        const current = tx.block(String(node.id)).block, attrs = blockAttrs(node);
        if (current.kind === node.type && JSON.stringify(current.attrs) === JSON.stringify(attrs)) continue;
        const id = crypto.randomUUID(); ids.push(id);
        tx.apply([this.content({ op: 'set_block', block: String(node.id), kind: node.type, attrs }, this.editMode(id))]);
      }
      for (const block of this.proposedMoves) {
        const item = retained.find(({ node }) => node.id === block);
        if (!item || item.parent && !originalIds.has(item.parent)) throw new Error('The proposed move destination must be a document block');
        const next = retained.find(({ parent, position }) => parent === item.parent && position > item.position);
        const before = next ? String(next.node.id) : null;
        const original = originals.find((view) => view.block.id === block)!.block;
        const originalNext = originals.find(({ block }) => block.parent === original.parent && block.position > original.position)?.block.id ?? null;
        if (original.parent === item.parent && originalNext === before) continue;
        const id = crypto.randomUUID(); ids.push(id);
        tx.apply([this.content({ op: 'move_block', block, parent: item.parent, before }, this.editMode(id))]);
      }
      const added = blocks.filter(({ node }) => !originalIds.has(String(node.id))), addedIds = new Set(added.map(({ node }) => String(node.id)));
      const groups = new Map<string, { parent: string | null; before: string | null; roots: string[] }>();
      for (const item of added.filter(({ parent }) => parent === null || !addedIds.has(parent))) {
        const next = retained.find(({ parent, position }) => parent === item.parent && position > item.position);
        const before = next ? String(next.node.id) : null, key = JSON.stringify([item.parent, before]);
        const group = groups.get(key) ?? { parent: item.parent, before, roots: [] };
        group.roots.push(String(item.node.id)); groups.set(key, group);
      }
      for (const { parent, before, roots } of groups.values()) {
        const members = new Set(roots);
        const tree = added.filter(({ node, parent }) => {
          if (!members.has(String(node.id)) && (parent === null || !members.has(parent))) return false;
          members.add(String(node.id)); return true;
        });
        const id = crypto.randomUUID(); ids.push(id);
        tx.apply([this.content({ op: 'insert_blocks', parent, before,
          blocks: tree.map(({ node, parent, position }) => ({ id: String(node.id), kind: node.type, attrs: blockAttrs(node),
            parent: parent && members.has(parent) ? parent : null, position: roots.includes(String(node.id)) ? roots.indexOf(String(node.id)) : position,
            text: isTextBlock(node) ? blockText(node) : '' })) }, this.editMode(id))]);
        let offset = 0;
        const format = (node: Descendant, link?: unknown) => {
          if (TextApi.isText(node)) {
            this.exactMarks(id, offset, node.text.length, { ...node, ...(link ? { link } : {}) }, true); offset += node.text.length;
          } else if (node.type === 'wikilink') wikiLinkLeaves(node as WikiLinkNode).forEach((leaf) => format({ ...leaf, wikilink: true }));
          else node.children.forEach((child) => format(child, node.type === 'a' ? node.url : link));
        };
        for (const { node } of tree) if (isTextBlock(node)) node.children.forEach((child) => format(child));
        const native = tx.view().proposals.find((view) => view.proposal.id === id)!;
        this.present(() => tree.forEach(({ node, path }, index) => this.editor.tf.setNodes({ quarryProposal: id, quarryProposedBlock: String(node.id), quarryProposalOrder: index,
          quarrySources: native.blocks[index].block.segments.map((segment) => segment.source) }, { at: path })));
      }
      return ids;
    }
    const ids = new Set(blocks.map(({ node }) => String(node.id)));
    for (const { node, path } of blocks) this.ensure(node, path);
    for (const { node } of blocks) {
      const current = tx.block(String(node.id)).block; const attrs = blockAttrs(node);
      if (current.kind !== node.type || JSON.stringify(current.attrs) !== JSON.stringify(attrs)) tx.apply([this.content({ op: 'set_block', block: String(node.id), kind: node.type, attrs })]);
    }
    // Parents precede children in the final tree. Reparent first, then order.
    for (const { node, parent } of blocks) if (tx.block(String(node.id)).block.parent !== parent) tx.apply([this.content({ op: 'move_block', block: String(node.id), parent, before: null })]);
    const before = new Map<string | null, string>();
    for (const { node, parent } of [...blocks].reverse()) {
      const id = String(node.id); const next = before.get(parent) ?? null;
      const siblings = tx.view().blocks.filter((view) => view.block.parent === parent);
      const index = siblings.findIndex((view) => view.block.id === id);
      if ((siblings[index + 1]?.block.id ?? null) !== next) tx.apply([this.content({ op: 'move_block', block: id, parent, before: next })]);
      before.set(parent, id);
    }
    for (const view of tx.view().blocks) if (!ids.has(view.block.id)) {
      if (!tx.block(view.block.id).block.deleted) tx.apply([this.content({ op: 'delete_block', block: view.block.id })]);
    }
    return [];
  }
  private fail(error: unknown) {
    this.failedTurn = true; this.endIntent();
    this.transaction?.abort(); this.transaction = undefined; this.dirty.clear(); this.dirtyProposals.clear(); this.formatProposals.clear(); this.structureChanged = false; this.proposedMoves.clear(); this.proposedDeletes.clear(); this.proposedStructures.clear();
    this.nativeIds = new Set(this.model.view().blocks.map((view) => view.block.id));
    this.options.pending?.(false);
    this.refresh(); this.options.error(error);
  }
  refresh() {
    this.flush();
    sourceDrafts(this.editor).reconcile(this.model.view());
    const desired = projectPlate(this.model.view(), true, (point) => this.model.locate(point)); this.scaffolds.add(inputBlockId(this.model.view()));
    for (const item of this.placeholders.values()) {
      const before = desired.findIndex((node) => node.id === item.before), after = desired.findIndex((node) => node.id === item.after);
      const position = after >= 0 ? after + 1 : before >= 0 ? before : Math.min(item.position, desired.length);
      desired.splice(position, 0, item.node);
    }
    this.present(() => this.editor.tf.withoutNormalizing(() => {
      this.presentChildren([], desired);
      const points = this.savedSelection.map((point) => {
        // Explicit recovery can discard the unsaved source containing the caret.
        try { return this.model.locate(point); } catch { return null; }
      }).map((point, index) => point ? slatePoint(this.editor.children, point.owner.id, point.offset, point.owner.kind === 'proposal', this.savedSelection[index].source, point.proposed_block) : undefined);
      if (points[0] && points[1]) this.editor.tf.select({ anchor: points[0], focus: points[1] });
    }));
    this.nativeIds = new Set(this.model.view().blocks.map((view) => view.block.id));
  }
  private presentChildren(parent: Path, desired: Descendant[]) {
    const children = () => parent.length ? (nodeAt(this.editor.children, parent) as TElement).children : this.editor.children;
    for (let index = 0; index < desired.length; index++) {
      const next = desired[index], path = [...parent, index];
      let existing = children()[index];
      if (ElementApi.isElement(next) && next.id && existing?.id !== next.id) {
        const from = children().findIndex((node, at) => at > index && node.id === next.id);
        if (from >= 0) { this.editor.tf.apply({ type: 'move_node', path: [...parent, from], newPath: path }); existing = children()[index]; }
        else { this.editor.tf.apply({ type: 'insert_node', path, node: next }); continue; }
      }
      if (!existing) { this.editor.tf.apply({ type: 'insert_node', path, node: next }); continue; }
      if (JSON.stringify(existing) === JSON.stringify(next)) continue;
      if (TextApi.isText(existing) !== TextApi.isText(next)) {
        this.editor.tf.apply({ type: 'remove_node', path, node: existing });
        this.editor.tf.apply({ type: 'insert_node', path, node: next }); continue;
      }
      // set_node retains Slate's React key. A remote attribute edit must not
      // remount a source editor and erase its private form draft.
      const properties: Record<string, unknown> = {}, newProperties: Record<string, unknown> = {};
      for (const key of new Set([...Object.keys(existing), ...Object.keys(next)])) {
        if (key === 'children' || key === 'text' || JSON.stringify(existing[key]) === JSON.stringify(next[key])) continue;
        properties[key] = existing[key]; newProperties[key] = next[key] ?? null;
      }
      if (Object.keys(newProperties).length) this.editor.tf.apply({ type: 'set_node', path, properties, newProperties });
      if (TextApi.isText(existing) && TextApi.isText(next)) {
        if (existing.text !== next.text) {
          if (existing.text) this.editor.tf.apply({ type: 'remove_text', path, offset: 0, text: existing.text });
          if (next.text) this.editor.tf.apply({ type: 'insert_text', path, offset: 0, text: next.text });
        }
      } else if (ElementApi.isElement(next)) this.presentChildren(path, next.children);
    }
    while (children().length > desired.length) this.editor.tf.apply({ type: 'remove_node', path: [...parent, desired.length], node: children()[desired.length] });
  }
  rememberSelection(fromDOM = false) {
    if (this.transaction) return;
    let selection = this.editor.selection;
    if (fromDOM && !this.editor.api.isComposing()) {
      const element = this.editor.api.toDOMNode(this.editor);
      const root = this.editor.api.findDocumentOrShadowRoot();
      const domSelection = this.editor.api.getWindow()?.getSelection();
      // A remote update can arrive before Slate's throttled selectionchange
      // handler. Capture the browser range against the displayed tree first.
      // Inputs and comment composers retain their own selection and focus.
      if (element && root?.activeElement === element && domSelection
        && this.editor.api.hasSelectableTarget(domSelection.anchorNode) && this.editor.api.hasTarget(domSelection.focusNode)) {
        selection = this.editor.api.toSlateRange(domSelection, { exactMatch: false, suppressThrow: true }) ?? selection;
      }
    }
    if (!selection) return;
    try {
      this.savedSelection = [selection.anchor, selection.focus].map((point) => {
        const at = textOffset(this.editor.children, point); return at.proposedBlock ? this.model.proposedBlockPoint(at.block, at.proposedBlock, at.blockOffset!) : this.model.pointFor({ kind: at.proposal ? 'proposal' : 'block', id: at.block }, at.offset);
      });
      this.options.selection?.(this.savedSelection);
    } catch { this.savedSelection = []; }
  }
  history(redo: boolean) {
    if (this.options.readOnly?.() || this.options.mode?.() === 'viewing') return;
    this.endIntent(); this.flush(); const batch = redo ? this.model.redo() : this.model.undo();
    if (batch) { this.refresh(); this.options.changed(batch); }
  }
  command(commands: Command[] | ((draft: DocumentDraft) => void), base?: string[], group?: string) {
    if (!this.inputDepth) this.endIntent();
    if (this.options.readOnly?.()) throw new Error('The document is not editable');
    this.flush();
    const batch = this.model.edit((draft) => typeof commands === 'function' ? commands(draft) : draft.apply(commands, base), undefined, this.compositionGroup ?? group);
    this.refresh(); this.options.changed(batch);
  }
  beginComment(): { base: string[]; ranges: TextRange[] } {
    this.flush(); const selection = this.editor.selection;
    if (!selection) throw new Error('Select text to comment on');
    const ranges = selectedText(this.editor.children, selection).flatMap((part) => part.proposedBlock
      ? this.model.proposedBlockSelection(part.block, part.proposedBlock, part.blockOffset!, part.blockOffset! + part.end - part.offset)
      : this.model.selectionFor({ kind: part.proposal ? 'proposal' : 'block', id: part.block }, part.offset, part.end));
    if (!ranges.length) throw new Error('Select text to comment on');
    return { base: this.model.heads(), ranges };
  }
}

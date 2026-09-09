import init, { NativeDocument, NativeCommandBuilder } from '../../generated/document/quarry_document';
import type { Command, CommandRequest } from '../../api/generated/schema/types.gen';
export type { BlockConversion, Command, CommandRequest, EditAction, EditMode } from '../../api/generated/schema/types.gen';

export interface TextPoint { source: string; cursor: string }
export interface TextRange { source: string; start: string; end: string }
export interface TargetFragment { source: string; first: string; last: string; last_width: number }
export interface SegmentRef { source: string; segment: string }
export type Attributes = Record<string, unknown>;
export interface SeedBlock {
  id: string; kind: string; parent: string | null; position: number;
  attrs: Attributes; text: string;
}
export interface Block extends Omit<SeedBlock, 'text'> { segments: SegmentRef[]; deleted: boolean }
export interface TextRun { text: string; marks: Attributes; source: string; source_start: number }
export interface BlockView { block: Block; text: string; runs: TextRun[] }
export interface Owner { kind: 'block' | 'proposal'; id: string }
export interface ResolvedPoint { owner: Owner; offset: number; proposed_block?: { block: string; offset: number } }
export interface Attachment { owner: Owner; start: number; end: number; quote: string }
export interface ReviewMetadata { created_at: string; updated_at: string; unattached_reason: string | null; legacy_record?: unknown }
export interface ReviewTarget { state: 'attached' | 'hidden' | 'deleted' | 'unattached'; attachments: Attachment[] }
export interface CommentView {
  comment: { id: string; author: string; body: string; original_quote: string; state: 'open' | 'resolved'; parent_id: string | null; deleted: boolean; metadata: ReviewMetadata };
  target: ReviewTarget;
}
export interface ProposalView {
  proposal: { id: string; author: string; body: string; action: { kind: 'text' | 'format' | 'update_block' | 'convert_block' | 'move_block' | 'split_block' | 'paste_blocks' | 'join_blocks' | 'delete_block' | 'insert_blocks' | 'unavailable'; [key: string]: unknown }; state: 'open' | 'accepted' | 'rejected' | 'closed'; metadata: ReviewMetadata };
  blocks: BlockView[]; text: string; runs: TextRun[]; target: ReviewTarget; acceptance_error: string | null;
}
export interface DocumentView {
  document_id: string; schema_version: number; heads: string[];
  blocks: BlockView[]; comments: CommentView[]; proposals: ProposalView[];
  conflicts: Array<{ id: string; after: string | null; base: string; incoming: string; canonical: string; resolved: boolean; author: string; metadata: ReviewMetadata }>;
}
export interface DocumentBatch { request_id: string; requests: CommandRequest[] }
export type HistoryGroup = string | { composition: string };

let ready: Promise<unknown> | undefined;
export function loadDocumentEngine() {
  return ready ??= init().catch((error) => { ready = undefined; throw error; });
}

/** Owns native document bytes. Markdown and editor nodes are projections. */
export class DocumentModel {
  private native: NativeDocument;
  private projection: DocumentView | undefined;
  private encoded: { bytes: Uint8Array; heads: string[] } | undefined;
  private undoStack: Array<{ before: string[]; after: string[] }> = [];
  private group: { key: string; at: number; heads: string } | undefined;
  private redoStack: Array<{ before: string[]; after: string[] }> = [];
  constructor(input: Uint8Array | NativeDocument) {
    this.native = input instanceof Uint8Array ? NativeDocument.load(input) : input;
    if (input instanceof Uint8Array) this.encoded = { bytes: input.slice(), heads: this.heads() };
  }
  static create(id: string) {
    return new DocumentModel(new NativeDocument(id));
  }
  static fromMarkdown(markdown: string, id = crypto.randomUUID()) {
    return new DocumentModel(NativeDocument.from_markdown(id, markdown));
  }
  markdown() { return this.native.markdown(); }
  archive(metadata: Record<string, unknown> = {}) { return JSON.stringify({ format: 'quarry-document', version: 1, metadata, bytes: Array.from(this.save()) }); }
  static fromArchive(text: string) {
    const archive = JSON.parse(text);
    if (archive.format !== 'quarry-document' || archive.version !== 1 || !Array.isArray(archive.bytes)
        || !archive.bytes.every((byte: unknown) => Number.isInteger(byte) && Number(byte) >= 0 && Number(byte) <= 255)) throw new Error('Unsupported Quarry archive');
    return new DocumentModel(Uint8Array.from(archive.bytes));
  }
  dispose() { this.native.free(); }
  save() {
    const heads = this.heads();
    if (!this.encoded) this.encoded = { bytes: this.native.save(), heads };
    else if (JSON.stringify(heads) !== JSON.stringify(this.encoded.heads)) {
      const changes = this.native.save_after(JSON.stringify(this.encoded.heads));
      const bytes = new Uint8Array(this.encoded.bytes.length + changes.length);
      bytes.set(this.encoded.bytes); bytes.set(changes, this.encoded.bytes.length);
      this.encoded = { bytes, heads };
    }
    return this.encoded.bytes.slice();
  }
  heads(): string[] { return this.projection?.heads ?? JSON.parse(this.native.heads()); }
  containsHistory(heads: string[]) { return this.native.contains_history(JSON.stringify(heads)); }
  changesSince(heads: string[]) { return this.native.save_after(JSON.stringify(heads)); }
  view(): DocumentView { return this.projection ??= JSON.parse(this.native.view()); }
  proposal(id: string): ProposalView { return JSON.parse(this.native.proposal_view(id)); }
  proposalsForBlock(block: string): ProposalView[] { return JSON.parse(this.native.proposal_views_for_block(block)); }
  reviewMarkers(owner: Owner): Array<[string, string, number, number]> {
    return JSON.parse(this.native.review_markers(owner.kind, owner.id));
  }
  point(block: string, offset: number): TextPoint { return JSON.parse(this.native.point(block, offset)); }
  selection(block: string, start: number, end: number): TextRange[] {
    return JSON.parse(this.native.selection(block, start, end));
  }
  pointFor(owner: Owner, offset: number): TextPoint {
    return owner.kind === 'block' ? this.point(owner.id, offset) : JSON.parse(this.native.proposal_point(owner.id, offset));
  }
  proposedBlockPoint(proposal: string, block: string, offset: number): TextPoint {
    return JSON.parse(this.native.proposed_block_point(proposal, block, offset));
  }
  proposedBlockSelection(proposal: string, block: string, start: number, end: number): TextRange[] {
    return JSON.parse(this.native.proposed_block_selection(proposal, block, start, end));
  }
  selectionFor(owner: Owner, start: number, end: number): TextRange[] {
    return owner.kind === 'block' ? this.selection(owner.id, start, end) : JSON.parse(this.native.proposal_selection(owner.id, start, end));
  }
  locate(point: TextPoint): ResolvedPoint | null {
    return JSON.parse(this.native.locate_point(JSON.stringify(point)));
  }
  captureTarget(ranges: TextRange[]): { fragments: TargetFragment[]; quote: string } {
    const [fragments, quote] = JSON.parse(this.native.capture_target(JSON.stringify(ranges)));
    return { fragments, quote };
  }
  resolveTarget(fragments: TargetFragment[]): ReviewTarget { return JSON.parse(this.native.resolve_target(JSON.stringify(fragments))); }
  apply(request: CommandRequest) { this.native.apply(JSON.stringify(request)); this.projection = undefined; }
  applyBatch(batch: DocumentBatch) {
    const candidate = this.fork();
    try {
      for (const request of batch.requests) candidate.apply(request);
      this.adopt(candidate);
    } catch (error) { candidate.dispose(); throw error; }
  }
  merge(bytes: Uint8Array, base?: string[] | null, heads?: string[]) {
    if (base) {
      if (!heads) throw new Error('Incremental state is missing its resulting heads');
      this.native.merge_changes(bytes, JSON.stringify(base), JSON.stringify(heads));
    } else this.native.merge(bytes);
    this.projection = undefined;
  }
  useSavedVersion(bytes: Uint8Array) {
    const candidate = new DocumentModel(bytes);
    if (candidate.view().document_id !== this.view().document_id) { candidate.dispose(); throw new Error('This is a different document'); }
    this.adopt(candidate);
    this.undoStack = []; this.redoStack = []; this.group = undefined;
  }

  /** Several editor steps commit as one batch. Failed translation changes nothing. */
  edit(edit: (draft: DocumentDraft) => void, requestId = crypto.randomUUID(), group?: HistoryGroup): DocumentBatch {
    const before = this.heads();
    const candidate = this.fork();
    const draft = new DocumentDraft(candidate, requestId);
    try {
      edit(draft); this.adopt(candidate);
      if (draft.batch.requests.length) {
        const after = this.heads(); const now = Date.now();
        const key = typeof group === 'string' ? group : group?.composition;
        if (key && this.group?.key === key && (typeof group === 'object' || now - this.group.at < 750) && this.group.heads === JSON.stringify(before)) this.undoStack.at(-1)!.after = after;
        else this.undoStack.push({ before, after });
        this.group = key ? { key, at: now, heads: JSON.stringify(after) } : undefined;
        this.redoStack = [];
      }
      return draft.batch;
    }
    catch (error) { candidate.dispose(); throw error; }
  }
  get canUndo() { return this.undoStack.length > 0; }
  get canRedo() { return this.redoStack.length > 0; }
  undo() { return this.reverse(this.undoStack, this.redoStack); }
  redo() { return this.reverse(this.redoStack, this.undoStack); }
  private reverse(from: Array<{ before: string[]; after: string[] }>, to: Array<{ before: string[]; after: string[] }>): DocumentBatch | undefined {
    this.group = undefined;
    const transition = from.at(-1);
    if (!transition) return;
    const before = this.heads();
    const candidate = this.fork();
    const draft = new DocumentDraft(candidate, crypto.randomUUID());
    try {
      draft.apply([{ op: 'revert', ...transition }]);
      this.adopt(candidate);
      from.pop(); to.push({ before, after: this.heads() });
      return draft.batch;
    } catch (error) { candidate.dispose(); throw error; }
  }
  private adopt(candidate: DocumentModel) {
    this.native.free(); this.native = candidate.native; this.projection = candidate.projection; this.encoded = candidate.encoded;
  }
  private fork() {
    const candidate = new DocumentModel(this.native.fork()); candidate.projection = this.projection; candidate.encoded = this.encoded; return candidate;
  }

  beginTransaction() {
    const request: CommandRequest = { request_id: crypto.randomUUID(), base: this.heads(), commands: [], at: new Date().toISOString() };
    return new DocumentTransaction(this, this.native.command_builder(request.request_id, request.at!), request);
  }

  finishTransaction(native: NativeDocument, request: CommandRequest, group?: HistoryGroup): DocumentBatch {
    if (JSON.stringify(this.heads()) !== JSON.stringify(request.base)) {
      native.free(); throw new Error('Finish the local editor transaction before receiving remote changes');
    }
    this.native.free(); this.native = native; this.projection = undefined;
    const after = this.heads(); const now = Date.now();
    const key = typeof group === 'string' ? group : group?.composition;
    if (key && this.group?.key === key && (typeof group === 'object' || now - this.group.at < 750) && this.group.heads === JSON.stringify(request.base)) this.undoStack.at(-1)!.after = after;
    else this.undoStack.push({ before: request.base, after });
    this.group = key ? { key, at: now, heads: JSON.stringify(after) } : undefined;
    this.redoStack = [];
    return { request_id: crypto.randomUUID(), requests: [request] };
  }
}

/** Private, staged commands for one editor operation group. Native validation
 * runs before adoption; no intermediate tree can be saved or sent. */
export class DocumentTransaction {
  private projection?: DocumentView;
  private closed = false;
  constructor(private readonly owner: DocumentModel, private readonly native: NativeCommandBuilder, readonly request: CommandRequest) {}
  view(): DocumentView { return this.projection ??= JSON.parse(this.native.view()); }
  proposal(id: string): ProposalView { return JSON.parse(this.native.proposal_view(id)); }
  proposalsForBlock(block: string): ProposalView[] { return JSON.parse(this.native.proposal_views_for_block(block)); }
  reviewMarkers(owner: Owner): Array<[string, string, number, number]> {
    return JSON.parse(this.native.review_markers(owner.kind, owner.id));
  }
  locate(point: TextPoint): ResolvedPoint | null { return JSON.parse(this.native.locate_point(JSON.stringify(point))); }
  block(id: string): BlockView { return JSON.parse(this.native.block_view(id)); }
  point(block: string, offset: number): TextPoint { return JSON.parse(this.native.point(block, offset)); }
  selection(block: string, start: number, end: number): TextRange[] { return JSON.parse(this.native.selection(block, start, end)); }
  proposalPoint(id: string, offset: number): TextPoint { return JSON.parse(this.native.proposal_point(id, offset)); }
  proposalSelection(id: string, start: number, end: number): TextRange[] { return JSON.parse(this.native.proposal_selection(id, start, end)); }
  proposedBlockPoint(proposal: string, block: string, offset: number): TextPoint { return JSON.parse(this.native.proposed_block_point(proposal, block, offset)); }
  proposedBlockSelection(proposal: string, block: string, start: number, end: number): TextRange[] { return JSON.parse(this.native.proposed_block_selection(proposal, block, start, end)); }
  apply(commands: Command[]) {
    if (this.closed) throw new Error('Editor transaction is closed');
    this.native.push(JSON.stringify(commands)); this.request.commands.push(...commands); this.projection = undefined;
  }
  finish(group?: HistoryGroup) {
    if (this.closed) throw new Error('Editor transaction is closed');
    this.closed = true;
    return this.owner.finishTransaction(this.native.finish(), this.request, group);
  }
  abort() { if (!this.closed) { this.closed = true; this.native.free(); } }
}

export class DocumentDraft {
  readonly batch: DocumentBatch;
  constructor(readonly model: DocumentModel, id: string) { this.batch = { request_id: id, requests: [] }; }
  apply(commands: Command[], base = this.model.heads()) {
    if (!commands.length) return;
    const request: CommandRequest = {
      request_id: `${this.batch.request_id}_${this.batch.requests.length}`,
      base, commands, at: new Date().toISOString(),
    };
    this.model.apply(request);
    this.batch.requests.push(request);
  }
}

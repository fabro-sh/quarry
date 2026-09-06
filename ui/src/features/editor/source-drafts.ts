import type { SlateEditor } from 'platejs';
import type { Block, DocumentView } from './document-model';

export interface SourceBlock extends Block { proposal?: string }
export interface SourceDraft {
  id: string;
  base: SourceBlock;
  field: 'code' | 'markdown';
  value: string;
  error?: string;
  unavailable?: 'removed' | 'converted';
}

/** Private form state belongs to the open editor, so removing a rendered block
 * cannot discard the user's unsaved source. It is never document content. */
class SourceDraftStore {
  private drafts = new Map<string, SourceDraft>();
  private snapshot: SourceDraft[] = [];
  private listeners = new Set<() => void>();
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  all = () => this.snapshot;
  get = (id: string) => this.drafts.get(id);
  open(id: string, base: SourceBlock, field: SourceDraft['field'], value: string) {
    this.drafts.set(id, { id, base, field, value }); this.emit();
  }
  update(id: string, change: Partial<Pick<SourceDraft, 'value' | 'error'>>) {
    const current = this.drafts.get(id);
    if (current) { this.drafts.set(id, { ...current, ...change }); this.emit(); }
  }
  close(id: string) { if (this.drafts.delete(id)) this.emit(); }
  reconcile(document: DocumentView) {
    if (!this.drafts.size) return;
    let changed = false;
    for (const [id, draft] of [...this.drafts]) {
      const proposal = draft.base.proposal && document.proposals.find((view) => view.proposal.id === draft.base.proposal);
      const accepted = proposal && proposal.proposal.state === 'accepted';
      const blocks = !draft.base.proposal || accepted ? document.blocks : proposal && proposal.proposal.state === 'open' ? proposal.blocks : [];
      const current = blocks.find((view) => view.block.id === draft.base.id)?.block;
      const unavailable = !current ? 'removed' : current.kind !== draft.base.kind ? 'converted' : undefined;
      if (accepted && current) {
        this.drafts.delete(id);
        this.drafts.set(draft.base.id, { ...draft, id: draft.base.id, base: { ...draft.base, proposal: undefined }, unavailable });
        changed = true;
      } else if (draft.unavailable !== unavailable) {
        this.drafts.set(id, { ...draft, unavailable }); changed = true;
      }
    }
    if (changed) this.emit();
  }
  private emit() {
    this.snapshot = [...this.drafts.values()];
    this.listeners.forEach((listener) => listener());
  }
}

const stores = new WeakMap<SlateEditor, SourceDraftStore>();
export function sourceDrafts(editor: SlateEditor) {
  let store = stores.get(editor);
  if (!store) { store = new SourceDraftStore(); stores.set(editor, store); }
  return store;
}

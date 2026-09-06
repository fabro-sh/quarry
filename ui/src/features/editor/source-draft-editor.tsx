import { useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';
import type { SlateEditor } from 'platejs';
import { documentAdapter } from './plate-document-adapter';
import { sourceDrafts, type SourceDraft } from './source-drafts';

export function useSourceDraft(editor: SlateEditor, id: string, field: SourceDraft['field'], value: string) {
  const store = sourceDrafts(editor);
  const draft = useSyncExternalStore(store.subscribe, () => store.get(id));
  return {
    draft,
    begin() {
      const adapter = documentAdapter(editor);
      if (!adapter) return;
      try { store.open(id, adapter.beginSourceEdit(id), field, value); }
      catch (error) { adapter.options.error(error); }
    },
    update(value: string) { store.update(id, { value }); },
    cancel() { store.close(id); },
    save() {
      if (!draft) return false;
      if (draft.value === String(draft.base.attrs[field] ?? '')) { store.close(id); return true; }
      try {
        const adapter = documentAdapter(editor);
        if (!adapter) throw new Error('The editor is still opening');
        adapter.saveSourceEdit(draft.base, { ...draft.base.attrs, [field]: draft.value });
        store.close(id); return true;
      } catch (error) { store.update(id, { error: error instanceof Error ? error.message : String(error) }); return false; }
    },
  };
}

export function SourceDraftRecovery({ editor }: { editor: SlateEditor }) {
  const store = sourceDrafts(editor);
  const drafts = useSyncExternalStore(store.subscribe, store.all).filter((draft) => draft.unavailable);
  if (!drafts.length) return null;
  return createPortal(<section aria-label="Unsaved source drafts" className="fixed bottom-4 right-4 z-50 max-h-[60vh] w-[min(32rem,calc(100vw-2rem))] overflow-auto rounded-lg border border-line bg-raised p-4 shadow-xl">
    <h2 className="text-sm font-medium text-ink">Unsaved source drafts</h2>
    {drafts.map((draft) => <div key={draft.id} className="mt-3 space-y-2">
      <p className="text-sm text-muted">{draft.unavailable === 'removed' ? 'The block was removed.' : 'The block type changed.'} Your source draft is kept here.</p>
      <textarea aria-label={draft.field === 'code' ? 'Mermaid source' : 'Markdown source'} className="min-h-32 w-full rounded border border-line bg-well p-2 font-mono text-sm text-ink" value={draft.value} onChange={(event) => store.update(draft.id, { value: event.target.value })} />
      <div className="flex gap-3 text-sm text-muted">
        <button type="button" onClick={() => void navigator.clipboard.writeText(draft.value).catch(() => store.update(draft.id, { error: 'Select the source text and copy it with your keyboard.' }))}>Copy draft</button>
        <button type="button" onClick={() => store.close(draft.id)}>Discard draft</button>
      </div>
      {draft.error && <p role="alert" className="text-sm text-danger">{draft.error}</p>}
    </div>)}
  </section>, document.body);
}

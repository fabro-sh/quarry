import * as Y from 'yjs';

import { isRecord } from '../../lib/utils';
import { type ReviewMeta, type ReviewMetaEntry } from './rfm-types';
import { useReviewStore } from './review-store';

export const REVIEW_ROOT = 'review';
const REVIEW_ORIGIN = 'quarry-review-doc';

type ReviewSection = 'comments' | 'suggestions';

interface ReviewDocBinding {
  lastMetaKey?: string;
  onMeta: (meta: ReviewMeta) => void;
  ready: boolean;
  root: Y.Map<unknown>;
}

export interface ReviewDocBindOptions {
  isSynced: boolean;
  onMeta: (meta: ReviewMeta) => void;
}

let activeBinding: ReviewDocBinding | null = null;

// Review metadata is shared as one plain JSON object per id. That gives us
// per-entry last-writer-wins semantics, which matches the current UI: comment
// bodies are write-once, replies get their own ids, and resolve/reopen is a
// single status flip. If rich concurrent comment editing lands, switch entries
// to nested Y.Maps for field-level merging.
export function metaFromReviewMap(root: Y.Map<unknown>): ReviewMeta {
  return {
    comments: metaSectionFromMap(root.get('comments')),
    suggestions: metaSectionFromMap(root.get('suggestions')),
  };
}

export function reconcileReviewMap(root: Y.Map<unknown>, meta: ReviewMeta): void {
  const doc = root.doc;
  const apply = () => {
    replaceSection(ensureSectionMap(root, 'comments'), meta.comments);
    replaceSection(ensureSectionMap(root, 'suggestions'), meta.suggestions);
  };
  if (doc) {
    doc.transact(apply, REVIEW_ORIGIN);
  } else {
    apply();
  }
}

export function bindReviewDoc(doc: Y.Doc, options: ReviewDocBindOptions): () => void {
  const root = doc.getMap<unknown>(REVIEW_ROOT);
  const binding: ReviewDocBinding = {
    onMeta: options.onMeta,
    ready: false,
    root,
  };
  activeBinding = binding;

  // The review map is seeded server-side from canonical review rows at
  // session seed (see `session.rs`), so a synced map is authoritative —
  // including an EMPTY one. (The legacy flusher-only bootstrap that seeded
  // the map from endmatter-parsed store metadata died with the Phase 5
  // autosave machinery.)
  const tryBecomeReady = () => {
    if (!options.isSynced) return;
    binding.ready = true;
    emitMeta(binding, metaFromReviewMap(root));
  };

  const handleChange = () => {
    if (activeBinding !== binding) return;
    if (!binding.ready) {
      tryBecomeReady();
      return;
    }
    emitMeta(binding, metaFromReviewMap(root));
  };

  root.observeDeep(handleChange);
  tryBecomeReady();

  return () => {
    root.unobserveDeep(handleChange);
    if (activeBinding === binding) activeBinding = null;
  };
}

export function applyReviewMutation(reducer: (meta: ReviewMeta) => ReviewMeta): void {
  const binding = activeBinding;
  if (binding?.ready) {
    const current = metaFromReviewMap(binding.root);
    const next = reducer(current);
    if (reviewMetaEqual(current, next)) return;
    reconcileReviewMap(binding.root, next);
    emitMeta(binding, next);
    return;
  }

  const store = useReviewStore.getState();
  const current = store.getMeta();
  const next = reducer(current);
  if (reviewMetaEqual(current, next)) return;
  store.setMeta(next);
}

function metaSectionFromMap(value: unknown): Record<string, ReviewMetaEntry> {
  if (!(value instanceof Y.Map)) return {};
  const entries: Record<string, ReviewMetaEntry> = {};
  for (const [id, entry] of value.entries()) {
    const parsed = reviewMetaEntryFromValue(entry);
    if (parsed) entries[id] = parsed;
  }
  return entries;
}

function reviewMetaEntryFromValue(value: unknown): ReviewMetaEntry | null {
  if (!isRecord(value)) return null;
  if (typeof value.by !== 'string' || typeof value.at !== 'string') return null;
  const entry: ReviewMetaEntry = { by: value.by, at: value.at };
  if (typeof value.editedAt === 'string') entry.editedAt = value.editedAt;
  if (typeof value.body === 'string') entry.body = value.body;
  if (typeof value.re === 'string') entry.re = value.re;
  if (value.status === 'resolved') entry.status = 'resolved';
  if (typeof value.resolved === 'string') entry.resolved = value.resolved;
  return entry;
}

function ensureSectionMap(root: Y.Map<unknown>, section: ReviewSection): Y.Map<unknown> {
  const existing = root.get(section);
  if (existing instanceof Y.Map) return existing;
  const next = new Y.Map<unknown>();
  root.set(section, next);
  return next;
}

function replaceSection(sectionMap: Y.Map<unknown>, entries: Record<string, ReviewMetaEntry>): void {
  for (const id of Array.from(sectionMap.keys())) {
    if (!(id in entries)) sectionMap.delete(id);
  }
  for (const [id, entry] of Object.entries(entries)) {
    const current = reviewMetaEntryFromValue(sectionMap.get(id));
    if (current && reviewMetaEntryEqual(current, entry)) continue;
    sectionMap.set(id, { ...entry });
  }
}

function reviewMetaEqual(left: ReviewMeta, right: ReviewMeta): boolean {
  return reviewMetaKey(left) === reviewMetaKey(right);
}

function reviewMetaEntryEqual(left: ReviewMetaEntry, right: ReviewMetaEntry): boolean {
  return JSON.stringify(stableEntry(left)) === JSON.stringify(stableEntry(right));
}

function emitMeta(binding: ReviewDocBinding, meta: ReviewMeta): void {
  const key = reviewMetaKey(meta);
  if (binding.lastMetaKey === key) return;
  binding.lastMetaKey = key;
  binding.onMeta(meta);
}

function reviewMetaKey(meta: ReviewMeta): string {
  return JSON.stringify(stableMeta(meta));
}

function stableMeta(meta: ReviewMeta): ReviewMeta {
  return {
    comments: stableEntries(meta.comments),
    suggestions: stableEntries(meta.suggestions),
  };
}

function stableEntries(entries: Record<string, ReviewMetaEntry>): Record<string, ReviewMetaEntry> {
  const sorted: Record<string, ReviewMetaEntry> = {};
  for (const key of Object.keys(entries).sort()) {
    sorted[key] = stableEntry(entries[key]);
  }
  return sorted;
}

function stableEntry(entry: ReviewMetaEntry): ReviewMetaEntry {
  const stable: ReviewMetaEntry = { by: entry.by, at: entry.at };
  if (entry.editedAt !== undefined) stable.editedAt = entry.editedAt;
  if (entry.body !== undefined) stable.body = entry.body;
  if (entry.re !== undefined) stable.re = entry.re;
  if (entry.status === 'resolved') stable.status = 'resolved';
  if (entry.resolved !== undefined) stable.resolved = entry.resolved;
  return stable;
}

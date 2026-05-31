import { create } from 'zustand';
import type { Descendant } from 'platejs';
import { emptyReviewMeta, type ReviewMeta, type ReviewMetaEntry } from './rfm-types';

function cloneMeta(meta: ReviewMeta): ReviewMeta {
  return { comments: { ...meta.comments }, suggestions: { ...meta.suggestions } };
}

export function addComment(meta: ReviewMeta, id: string, fields: { by: string; at: string; body?: string }): ReviewMeta {
  const next = cloneMeta(meta);
  const entry: ReviewMetaEntry = { by: fields.by, at: fields.at };
  if (fields.body) entry.body = fields.body;
  next.comments[id] = entry;
  return next;
}

export function addReply(meta: ReviewMeta, id: string, fields: { parentId: string; body: string; by: string; at: string }): ReviewMeta {
  const next = cloneMeta(meta);
  next.comments[id] = { by: fields.by, at: fields.at, body: fields.body, re: fields.parentId };
  return next;
}

export function editComment(meta: ReviewMeta, id: string, body: string): ReviewMeta {
  const existing = meta.comments[id];
  if (!existing) return meta;
  const next = cloneMeta(meta);
  next.comments[id] = { ...existing, body };
  return next;
}

export function resolveComment(meta: ReviewMeta, id: string, summary?: string): ReviewMeta {
  const existing = meta.comments[id];
  if (!existing) return meta;
  const next = cloneMeta(meta);
  const entry: ReviewMetaEntry = { ...existing, status: 'resolved' };
  if (summary) entry.resolved = summary;
  next.comments[id] = entry;
  return next;
}

export function deleteComment(meta: ReviewMeta, id: string): ReviewMeta {
  const next = cloneMeta(meta);
  delete next.comments[id];
  for (const [key, entry] of Object.entries(next.comments)) {
    if (entry.re === id) delete next.comments[key];
  }
  return next;
}

export interface ReviewThread {
  id: string;
  entry: ReviewMetaEntry;
  replies: Array<{ id: string; entry: ReviewMetaEntry }>;
}

export function buildThreads(meta: ReviewMeta): ReviewThread[] {
  const roots: ReviewThread[] = [];
  for (const [id, entry] of Object.entries(meta.comments)) {
    if (!entry.re) roots.push({ id, entry, replies: [] });
  }
  for (const [id, entry] of Object.entries(meta.comments)) {
    if (entry.re) {
      const root = roots.find((r) => r.id === entry.re);
      if (root) root.replies.push({ id, entry });
    }
  }
  for (const root of roots) root.replies.sort((a, b) => a.entry.at.localeCompare(b.entry.at));
  roots.sort((a, b) => a.entry.at.localeCompare(b.entry.at));
  return roots;
}

export function syncSuggestionsFromValue(meta: ReviewMeta, value: Descendant[]): ReviewMeta {
  const next = cloneMeta(meta);
  const visit = (node: Record<string, unknown>) => {
    for (const key of Object.keys(node)) {
      if (!key.startsWith('suggestion_')) continue;
      const raw = node[key];
      if (typeof raw !== 'object' || raw === null) continue;
      const data: Record<string, unknown> = { ...raw };
      const id = data.id;
      const userId = data.userId;
      const createdAt = data.createdAt;
      if (typeof id === 'string' && !next.suggestions[id]) {
        const at = typeof createdAt === 'number' && createdAt > 0 ? new Date(createdAt).toISOString() : new Date().toISOString();
        next.suggestions[id] = { by: typeof userId === 'string' ? userId : 'user', at };
      }
    }
    const children = node.children;
    if (Array.isArray(children)) {
      for (const child of children) {
        if (typeof child === 'object' && child !== null) visit({ ...child });
      }
    }
  };
  for (const node of value) visit(node);
  return next;
}

interface ReviewStoreState {
  meta: ReviewMeta;
  hydrate: (meta: ReviewMeta) => void;
  setMeta: (meta: ReviewMeta) => void;
  getMeta: () => ReviewMeta;
}

export const useReviewStore = create<ReviewStoreState>((set, get) => ({
  meta: emptyReviewMeta(),
  hydrate: (meta) => set({ meta }),
  setMeta: (meta) => set({ meta }),
  getMeta: () => get().meta,
}));

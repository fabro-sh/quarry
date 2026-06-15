import { MarkdownPlugin } from '@platejs/markdown';
import { createSlateEditor, type Descendant } from 'platejs';
import remarkGfm from 'remark-gfm';

import { baseMarkdownPlugins } from '../editor/markdown-codec';
import { remarkInlineMarks } from '../editor/remark-inline-marks';
import { mermaidMdRules } from '../editor/mermaid';
import { tableMdRules } from '../editor/table';
import { wikiLinkMdRules } from '../editor/wiki-link';
import type { ReviewMeta, ReviewMetaEntry } from './rfm-types';
import { readSuggestionMark } from './suggestion-mark';

const SERIALIZER_CACHE_LIMIT = 16;
const serializerEditors = new Map<string, ReturnType<typeof createSlateEditor>>();

/** Read the comment id off a leaf (key `comment_<id>`, excluding the draft key). */
function commentId(leaf: Record<string, unknown>): string | null {
  for (const key of Object.keys(leaf)) {
    if (key.startsWith('comment_') && key !== 'comment_draft' && leaf[key] === true) {
      return key.slice('comment_'.length);
    }
  }
  return null;
}

/** Build the Plate MdRules that serialize review marks to CriticMarkup. */
export function reviewMdRules(meta: ReviewMeta) {
  return {
    ...wikiLinkMdRules,
    ...mermaidMdRules,
    ...tableMdRules,
    suggestion: {
      mark: true,
      serialize: (leaf: Record<string, unknown> & { text: string }) => {
        const data = readSuggestionMark(leaf);
        if (!data || data.type === 'update') return { type: 'text', value: leaf.text };
        const open = data.type === 'remove' ? '{--' : '{++';
        const close = data.type === 'remove' ? '--}' : '++}';
        return { type: 'text', value: `${open}${leaf.text}${close}{#${data.id}}` };
      },
    },
    comment: {
      mark: true,
      serialize: (leaf: Record<string, unknown> & { text: string }) => {
        const id = commentId(leaf);
        if (!id) return { type: 'text', value: leaf.text };
        const body = meta.comments[id]?.body ?? '';
        const bodyPart = body ? `{>>${body}<<}` : '';
        return { type: 'text', value: `{==${leaf.text}==}${bodyPart}{#${id}}` };
      },
    },
  };
}

function serializerEditor(meta: ReviewMeta) {
  const stable = stableMeta(meta);
  const key = JSON.stringify(stable);
  const cached = serializerEditors.get(key);
  if (cached) return cached;

  const editor = createSlateEditor({
    plugins: [
      ...baseMarkdownPlugins,
      MarkdownPlugin.configure({
        options: { remarkPlugins: [remarkGfm, remarkInlineMarks], rules: reviewMdRules(stable) },
      }),
    ],
  });
  serializerEditors.set(key, editor);
  if (serializerEditors.size > SERIALIZER_CACHE_LIMIT) {
    const oldest = serializerEditors.keys().next().value;
    if (oldest) serializerEditors.delete(oldest);
  }
  return editor;
}

/** Serialize a Plate value's body to Markdown with review marks emitted as CriticMarkup. */
export function serializeReviewBody(value: Descendant[], meta: ReviewMeta): string {
  return serializerEditor(meta).api.markdown.serialize({ value }).replace(/\n+$/, '');
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
  if (entry.status === 'resolved') stable.status = entry.status;
  if (entry.resolved !== undefined) stable.resolved = entry.resolved;
  return stable;
}

import { MarkdownPlugin } from '@platejs/markdown';
import { createSlateEditor, type Descendant } from 'platejs';
import remarkGfm from 'remark-gfm';

import { baseMarkdownPlugins } from '../editor/markdown-codec';
import { remarkInlineMarks } from '../editor/remark-inline-marks';
import { wikiLinkMdRules } from '../editor/wiki-link';
import type { ReviewMeta } from './rfm-types';
import { readSuggestionMark } from './suggestion-mark';

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
  return createSlateEditor({
    plugins: [
      ...baseMarkdownPlugins,
      MarkdownPlugin.configure({
        options: { remarkPlugins: [remarkGfm, remarkInlineMarks], rules: reviewMdRules(meta) },
      }),
    ],
  });
}

/** Serialize a Plate value's body to Markdown with review marks emitted as CriticMarkup. */
export function serializeReviewBody(value: Descendant[], meta: ReviewMeta): string {
  return serializerEditor(meta).api.markdown.serialize({ value }).replace(/\n+$/, '');
}

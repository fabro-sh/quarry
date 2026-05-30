import {
  BaseBlockquotePlugin,
  BaseBoldPlugin,
  BaseCodePlugin,
  BaseH1Plugin,
  BaseH2Plugin,
  BaseH3Plugin,
  BaseH4Plugin,
  BaseH5Plugin,
  BaseH6Plugin,
  BaseHighlightPlugin,
  BaseHorizontalRulePlugin,
  BaseItalicPlugin,
  BaseStrikethroughPlugin,
  BaseSubscriptPlugin,
  BaseSuperscriptPlugin,
  BaseUnderlinePlugin,
} from '@platejs/basic-nodes';
import { BaseCodeBlockPlugin, BaseCodeLinePlugin, BaseCodeSyntaxPlugin } from '@platejs/code-block';
import { BaseLinkPlugin } from '@platejs/link';
import { BaseListPlugin } from '@platejs/list';
import { MarkdownPlugin } from '@platejs/markdown';
import { BaseParagraphPlugin, createSlateEditor, type Descendant } from 'platejs';
import remarkGfm from 'remark-gfm';

import { remarkInlineMarks } from '../editor/remark-inline-marks';
import type { ReviewMeta } from './rfm-types';

/** Read the suggestion data object off a leaf (key `suggestion_<id>`). */
function suggestionData(leaf: Record<string, unknown>): { id: string; type: 'insert' | 'remove' } | null {
  for (const key of Object.keys(leaf)) {
    if (!key.startsWith('suggestion_')) continue;
    const raw = leaf[key];
    if (typeof raw !== 'object' || raw === null) continue;
    const data: Record<string, unknown> = { ...raw };
    const id = data.id;
    const type = data.type;
    if (typeof id === 'string' && (type === 'insert' || type === 'remove')) {
      return { id, type };
    }
  }
  return null;
}

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
    highlight: {
      mark: true,
      serialize: (leaf: { text: string }) => ({ type: 'text', value: `{==${leaf.text}==}` }),
    },
    suggestion: {
      mark: true,
      serialize: (leaf: Record<string, unknown> & { text: string }) => {
        const data = suggestionData(leaf);
        if (!data) return { type: 'text', value: leaf.text };
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
      BaseParagraphPlugin,
      BaseH1Plugin,
      BaseH2Plugin,
      BaseH3Plugin,
      BaseH4Plugin,
      BaseH5Plugin,
      BaseH6Plugin,
      BaseBlockquotePlugin,
      BaseHorizontalRulePlugin,
      BaseCodeBlockPlugin,
      BaseCodeLinePlugin,
      BaseCodeSyntaxPlugin,
      BaseBoldPlugin,
      BaseItalicPlugin,
      BaseCodePlugin,
      BaseStrikethroughPlugin,
      BaseUnderlinePlugin,
      BaseSubscriptPlugin,
      BaseSuperscriptPlugin,
      BaseHighlightPlugin,
      BaseListPlugin,
      BaseLinkPlugin,
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

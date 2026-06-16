import { MarkdownPlugin } from '@platejs/markdown';
import { createSlateEditor, type Descendant } from 'platejs';
import remarkGfm from 'remark-gfm';

import { baseMarkdownPlugins, stripTrailingEmptyParagraphs } from '../editor/markdown-codec';
import { remarkInlineMarks } from '../editor/remark-inline-marks';
import { stripPlaceholders } from '../editor/image';
import { applyMermaid } from '../editor/mermaid';
import { normalizeTablesInValue, tableMdRules } from '../editor/table';
import { applyWikiLinks } from '../editor/wiki-link';
import { applyCriticMarkup } from './apply-critic-markup';
import { collapseSubstitutions, expandSubstitutions } from './collapse-substitutions';
import { serializeReviewMeta, splitEndmatter } from './endmatter';
import { emptyReviewMeta, type ReviewMeta, type ReviewMetaEntry } from './rfm-types';
import { serializeReviewBody } from './review-md-rules';
import { readSuggestionMark } from './suggestion-mark';

interface ReviewDocument {
  value: Descendant[];
  meta: ReviewMeta;
}

interface LiveIds {
  comments: Set<string>;
  suggestions: Set<string>;
}

function deserializeEditor() {
  return createSlateEditor({
    plugins: [
      ...baseMarkdownPlugins,
      MarkdownPlugin.configure({
        options: { remarkPlugins: [remarkGfm, remarkInlineMarks], rules: tableMdRules },
      }),
    ],
  });
}

/** Markdown (RFM) → Plate value with review marks + parsed metadata. */
export function markdownToReview(markdown: string): ReviewDocument {
  const { body, meta } = splitEndmatter(markdown);
  const rawValue = deserializeEditor().api.markdown.deserialize(expandSubstitutions(body));
  const reviewed = applyCriticMarkup(rawValue, meta ?? emptyReviewMeta());
  const value = normalizeTablesInValue(applyMermaid(applyWikiLinks(reviewed.value)) as never);
  return {
    value,
    meta: pruneMeta(reviewed.meta, liveIds(value)),
  };
}

/** Collect the comment/suggestion ids still present as marks in the value. */
function liveIds(value: Descendant[]): LiveIds {
  const live: LiveIds = { comments: new Set(), suggestions: new Set() };
  collectIds(value, live);
  return live;
}

function collectIds(nodes: Descendant[], live: LiveIds): void {
  for (const node of nodes) {
    for (const key of Object.keys(node)) {
      if (key === 'comment_draft') continue;
      if (key.startsWith('comment_') && node[key] === true) {
        live.comments.add(key.slice('comment_'.length));
      }
    }
    const mark = readSuggestionMark(node);
    if (mark) live.suggestions.add(mark.id);
    const children = node.children;
    if (Array.isArray(children)) collectIds(children, live);
  }
}

/** Drop metadata whose anchor mark no longer exists (replies survive while their parent does). */
function pruneMeta(meta: ReviewMeta, live: LiveIds): ReviewMeta {
  const comments: Record<string, ReviewMetaEntry> = {};
  for (const [id, entry] of Object.entries(meta.comments)) {
    const parentLive =
      entry.re !== undefined && (live.comments.has(entry.re) || live.suggestions.has(entry.re));
    if (live.comments.has(id) || parentLive) comments[id] = entry;
  }
  const suggestions: Record<string, ReviewMetaEntry> = {};
  for (const [id, entry] of Object.entries(meta.suggestions)) {
    if (live.suggestions.has(id)) suggestions[id] = entry;
  }
  return { comments, suggestions };
}

/**
 * Strip the `body` of root comments (they are emitted inline by
 * `serializeReviewBody`); replies keep their body. Constructs new entries so
 * the input is left untouched.
 */
function endmatterMeta(pruned: ReviewMeta): ReviewMeta {
  const comments: Record<string, ReviewMetaEntry> = {};
  for (const [id, entry] of Object.entries(pruned.comments)) {
    if (entry.re === undefined) {
      const { body: _body, ...rest } = entry;
      comments[id] = rest;
    } else {
      comments[id] = entry;
    }
  }
  return { comments, suggestions: pruned.suggestions };
}

/** Plate value + metadata → Markdown (RFM). */
export function reviewToMarkdown(value: Descendant[], meta: ReviewMeta): string {
  // Convert any stray `[[...]]` text to wiki-link nodes first, so a link the user
  // typed (but the editor hasn't turned into a chip yet) still serializes as
  // `[[...]]` rather than being escaped to `\[\[...]]`. Drop in-flight upload
  // placeholders — they aren't part of the saved document.
  const wikied = normalizeTablesInValue(
    stripTrailingEmptyParagraphs(stripPlaceholders(applyWikiLinks(value)))
  );
  const live = liveIds(wikied);
  const pruned = pruneMeta(meta, live);
  const body = collapseSubstitutions(serializeReviewBody(wikied, pruned));
  const endmatter = serializeReviewMeta(endmatterMeta(pruned));
  return endmatter ? `${body}\n\n---\n${endmatter}` : `${body}\n`;
}

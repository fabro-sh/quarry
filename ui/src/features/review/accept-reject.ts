import { acceptSuggestion, rejectSuggestion } from '@platejs/suggestion';
import { SuggestionPlugin } from '@platejs/suggestion/react';
import { KEYS } from 'platejs';
import type { PlateEditor } from 'platejs/react';
import { resolveSuggestions } from './resolve-suggestions';
import { serializeReviewMeta, splitEndmatter } from './endmatter';
import { applyReviewMutation } from './review-doc';
import { removeSuggestion } from './review-store';

export type SuggestionResolution = 'accept' | 'reject';

// Apply (accept) the suggestion with the given id: an insertion keeps its text
// and a deletion removes its text; either way the suggestion mark is dropped.
// The edit is wrapped in `withoutSuggestions` so it isn't itself recorded as a
// new suggestion while suggesting mode is on.
export function acceptSuggestionById(editor: PlateEditor, id: string): void {
  const desc = resolveSuggestions(editor.children).find((s) => s.suggestionId === id);
  if (!desc) return;
  editor.getApi(SuggestionPlugin).suggestion.withoutSuggestions(() => {
    editor.tf.withoutNormalizing(() => {
      acceptSuggestion(editor, desc);
      if (editor.children.length === 0) {
        editor.tf.insertNodes({ type: KEYS.p, children: [{ text: '' }] }, { at: [0] });
      }
    });
  });
  applyReviewMutation((meta) => removeSuggestion(meta, id));
}

// Revert (reject) the suggestion with the given id: an insertion's text is
// removed and a deletion's text is kept; either way the suggestion mark is
// dropped.
export function rejectSuggestionById(editor: PlateEditor, id: string): void {
  const desc = resolveSuggestions(editor.children).find((s) => s.suggestionId === id);
  if (!desc) return;
  editor.getApi(SuggestionPlugin).suggestion.withoutSuggestions(() => rejectSuggestion(editor, desc));
  applyReviewMutation((meta) => removeSuggestion(meta, id));
}

export function resolveSuggestionInMarkdown(
  markdown: string,
  id: string,
  resolution: SuggestionResolution
): string | null {
  const { body, meta } = splitEndmatter(markdown);
  if (meta?.suggestions[id]?.kind === 'block_delete') return null;
  const escapedId = escapeRegExp(id);
  const replacements: Array<[RegExp, (match: RegExpExecArray) => string]> = [
    [
      new RegExp(String.raw`\{~~([\s\S]*?)~>([\s\S]*?)~~\}\{#${escapedId}\}`),
      (match) => (resolution === 'accept' ? match[2] : match[1]),
    ],
    [
      new RegExp(String.raw`\{\+\+([\s\S]*?)\+\+\}\{#${escapedId}\}`),
      (match) => (resolution === 'accept' ? match[1] : ''),
    ],
    [
      new RegExp(String.raw`\{--([\s\S]*?)--\}\{#${escapedId}\}`),
      (match) => (resolution === 'accept' ? '' : match[1]),
    ],
  ];

  let nextBody: string | null = null;
  for (const [pattern, replacement] of replacements) {
    const match = pattern.exec(body);
    if (!match) continue;
    nextBody = body.replace(pattern, replacement(match));
    break;
  }
  if (nextBody === null) return null;

  if (!meta) return nextBody.endsWith('\n') ? nextBody : `${nextBody}\n`;
  const nextMeta = removeSuggestion(meta, id);
  const endmatter = serializeReviewMeta(nextMeta);
  return endmatter ? `${nextBody}\n\n---\n${endmatter}` : `${nextBody}\n`;
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

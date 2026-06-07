import { NodeApi, type Value } from 'platejs';
import { ParagraphPlugin, createPlateEditor, type PlateEditor } from 'platejs/react';
import { describe, expect, it } from 'vitest';
import { reviewKit } from '../editor/review-kit';
import {
  acceptSuggestionById,
  rejectSuggestionById,
  resolveSuggestionInMarkdown,
} from './accept-reject';
import { markdownToReview } from './rfm-codec';
import { useReviewStore } from './review-store';
import { emptyReviewMeta } from './rfm-types';

// One paragraph: "keep " then an inserted "ins" (suggestion s1, type insert),
// then "mid " then a removed "del" (suggestion s2, type remove), then a
// replacement pair for s3.
function buildEditor() {
  return createPlateEditor({
    plugins: [ParagraphPlugin, ...reviewKit],
    value: [
      {
        type: 'p',
        children: [
          { text: 'keep ' },
          {
            text: 'ins',
            suggestion: true,
            suggestion_s1: { id: 's1', type: 'insert', userId: 'user', createdAt: 0 },
          } as never,
          { text: 'mid ' },
          {
            text: 'del',
            suggestion: true,
            suggestion_s2: { id: 's2', type: 'remove', userId: 'user', createdAt: 0 },
          } as never,
          { text: 'swap ' },
          {
            text: 'old',
            suggestion: true,
            suggestion_s3: { id: 's3', type: 'remove', userId: 'user', createdAt: 0 },
          } as never,
          {
            text: 'new',
            suggestion: true,
            suggestion_s3: { id: 's3', type: 'insert', userId: 'user', createdAt: 0 },
          } as never,
        ],
      },
    ],
  });
}

function docText(editor: PlateEditor): string {
  return editor.children.map((node) => NodeApi.string(node)).join('');
}

function suggestionKeys(editor: PlateEditor): string[] {
  const keys: string[] = [];
  for (const [node] of editor.api.nodes({ at: [] })) {
    for (const key of Object.keys(node)) {
      if (key.startsWith('suggestion_')) keys.push(key);
    }
  }
  return keys;
}

describe('accept/reject suggestions', () => {
  it('accept on an insert keeps the inserted text and drops the mark', () => {
    useReviewStore.getState().hydrate({ comments: {}, suggestions: { s1: { by: 'AI', at: '2026-01-01T00:00:00.000Z' } } });
    const editor = buildEditor();
    expect(suggestionKeys(editor)).toContain('suggestion_s1');

    acceptSuggestionById(editor, 's1');

    expect(docText(editor)).toContain('ins');
    expect(suggestionKeys(editor)).not.toContain('suggestion_s1');
    expect(useReviewStore.getState().getMeta().suggestions.s1).toBeUndefined();
    useReviewStore.getState().hydrate(emptyReviewMeta());
  });

  it('reject on an insert removes the inserted text and drops the mark', () => {
    const editor = buildEditor();

    rejectSuggestionById(editor, 's1');

    expect(docText(editor)).not.toContain('ins');
    expect(suggestionKeys(editor)).not.toContain('suggestion_s1');
  });

  it('accept on a remove deletes the removed text and drops the mark', () => {
    const editor = buildEditor();
    expect(docText(editor)).toContain('del');

    acceptSuggestionById(editor, 's2');

    expect(docText(editor)).not.toContain('del');
    expect(suggestionKeys(editor)).not.toContain('suggestion_s2');
  });

  it('reject on a remove keeps the text and drops the mark', () => {
    const editor = buildEditor();

    rejectSuggestionById(editor, 's2');

    expect(docText(editor)).toContain('del');
    expect(suggestionKeys(editor)).not.toContain('suggestion_s2');
  });

  it('is a no-op for an unknown suggestion id', () => {
    const editor = buildEditor();
    const before = docText(editor);

    acceptSuggestionById(editor, 'nope');
    rejectSuggestionById(editor, 'nope');

    expect(docText(editor)).toBe(before);
  });

  it('accept on a replacement keeps new text and drops both marks', () => {
    const editor = buildEditor();

    acceptSuggestionById(editor, 's3');

    expect(docText(editor)).toContain('swap new');
    expect(docText(editor)).not.toContain('swap oldnew');
    expect(suggestionKeys(editor)).not.toContain('suggestion_s3');
  });

  it('accepts a replacement parsed from CriticMarkup', () => {
    const { value } = markdownToReview(
      'Review {~~suggestion target~>agent suggestion replacement~~}{#agent-suggestion-smoke} here.\n\n---\nsuggestions:\n  agent-suggestion-smoke:\n    by: Codex\n    at: "2026-01-01T00:00:00.000Z"\n'
    );
    const editor = createPlateEditor({
      plugins: [ParagraphPlugin, ...reviewKit],
      value: value as Value,
    });

    acceptSuggestionById(editor, 'agent-suggestion-smoke');

    expect(docText(editor)).toContain('Review agent suggestion replacement here.');
    expect(docText(editor)).not.toContain('suggestion target');
    expect(suggestionKeys(editor)).not.toContain('suggestion_agent-suggestion-smoke');
  });

  it('reject on a replacement keeps old text and drops both marks', () => {
    const editor = buildEditor();

    rejectSuggestionById(editor, 's3');

    expect(docText(editor)).toContain('swap old');
    expect(docText(editor)).not.toContain('swap oldnew');
    expect(suggestionKeys(editor)).not.toContain('suggestion_s3');
  });

  it('accepts a substitution in serialized Markdown and drops its metadata', () => {
    const markdown =
      'Use {~~rough~>specific~~}{#s1} wording.\n\n---\nsuggestions:\n  s1:\n    by: AI\n    at: "2026-01-01T00:00:00.000Z"\n  s2:\n    by: user\n    at: "2026-01-02T00:00:00.000Z"\n';

    const resolved = resolveSuggestionInMarkdown(markdown, 's1', 'accept');

    expect(resolved).toContain('Use specific wording.');
    expect(resolved).not.toContain('s1:');
    expect(resolved).toContain('s2:');
  });

  it('rejects an insertion in serialized Markdown', () => {
    const markdown = 'Keep this{++ extra++}{#s1}.\n\n---\nsuggestions:\n  s1:\n    by: AI\n    at: "2026-01-01T00:00:00.000Z"\n';

    expect(resolveSuggestionInMarkdown(markdown, 's1', 'reject')).toBe('Keep this.\n');
  });

  it('rejects a deletion in serialized Markdown', () => {
    const markdown = 'Keep {--this --}{#s1}text.\n';

    expect(resolveSuggestionInMarkdown(markdown, 's1', 'reject')).toBe('Keep this text.\n');
  });
});

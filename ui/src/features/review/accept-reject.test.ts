import { NodeApi } from 'platejs';
import { ParagraphPlugin, createPlateEditor } from 'platejs/react';
import { describe, expect, it } from 'vitest';
import { reviewKit } from '../editor/review-kit';
import { acceptSuggestionById, rejectSuggestionById } from './accept-reject';

// One paragraph: "keep " then an inserted "ins" (suggestion s1, type insert),
// then "mid " then a removed "del" (suggestion s2, type remove).
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
        ],
      },
    ],
  });
}

function docText(editor: ReturnType<typeof buildEditor>): string {
  return editor.children.map((node) => NodeApi.string(node)).join('');
}

function suggestionKeys(editor: ReturnType<typeof buildEditor>): string[] {
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
    const editor = buildEditor();
    expect(suggestionKeys(editor)).toContain('suggestion_s1');

    acceptSuggestionById(editor, 's1');

    expect(docText(editor)).toContain('ins');
    expect(suggestionKeys(editor)).not.toContain('suggestion_s1');
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
});

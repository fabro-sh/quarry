import { acceptSuggestion, rejectSuggestion } from '@platejs/suggestion';
import { SuggestionPlugin } from '@platejs/suggestion/react';
import type { PlateEditor } from 'platejs/react';
import { resolveSuggestions } from './resolve-suggestions';

// Apply (accept) the suggestion with the given id: an insertion keeps its text
// and a deletion removes its text; either way the suggestion mark is dropped.
// The edit is wrapped in `withoutSuggestions` so it isn't itself recorded as a
// new suggestion while suggesting mode is on.
export function acceptSuggestionById(editor: PlateEditor, id: string): void {
  const desc = resolveSuggestions(editor.children).find((s) => s.suggestionId === id);
  if (!desc) return;
  editor.getApi(SuggestionPlugin).suggestion.withoutSuggestions(() => acceptSuggestion(editor, desc));
}

// Revert (reject) the suggestion with the given id: an insertion's text is
// removed and a deletion's text is kept; either way the suggestion mark is
// dropped.
export function rejectSuggestionById(editor: PlateEditor, id: string): void {
  const desc = resolveSuggestions(editor.children).find((s) => s.suggestionId === id);
  if (!desc) return;
  editor.getApi(SuggestionPlugin).suggestion.withoutSuggestions(() => rejectSuggestion(editor, desc));
}

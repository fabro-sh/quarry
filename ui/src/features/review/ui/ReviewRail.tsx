import { useEditorSelector, type PlateEditor } from 'platejs/react';
import { useMemo } from 'react';

import { acceptSuggestionById, rejectSuggestionById } from '../accept-reject';
import { resolveSuggestions } from '../resolve-suggestions';
import { buildThreads, useReviewStore } from '../review-store';
import { CommentThreadCard } from './CommentThreadCard';
import { SuggestionCard } from './SuggestionCard';

// The review rail lists every open comment thread and suggestion for the
// document. It reads comments from the store and recomputes suggestions from
// the live editor value (via useEditorSelector) so accept/reject and new
// suggestions stay in sync. When there's nothing to review the rail is hidden.
//
// Ordering: comments first, then suggestions. Anchoring each card to its
// position in the document is a deferred follow-up.
export function ReviewRail({ editor }: { editor: PlateEditor }) {
  const meta = useReviewStore((s) => s.meta);
  const threads = useMemo(() => buildThreads(meta), [meta]);
  const suggestions = useEditorSelector((ed) => resolveSuggestions(ed.children), []);

  if (threads.length === 0 && suggestions.length === 0) return null;

  return (
    <aside
      aria-label="Review"
      className="flex h-full w-80 shrink-0 flex-col gap-2 overflow-auto border-l border-line bg-surface p-3"
      data-testid="review-rail"
    >
      {threads.map((thread) => (
        <CommentThreadCard key={thread.id} thread={thread} />
      ))}
      {suggestions.map((suggestion) => (
        <SuggestionCard
          key={suggestion.suggestionId}
          onAccept={(id) => acceptSuggestionById(editor, id)}
          onReject={(id) => rejectSuggestionById(editor, id)}
          suggestion={suggestion}
        />
      ))}
    </aside>
  );
}

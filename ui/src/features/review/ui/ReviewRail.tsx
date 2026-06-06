import { useEditorSelector, type PlateEditor } from 'platejs/react';
import { useMemo } from 'react';

import {
  acceptSuggestionById,
  rejectSuggestionById,
  type SuggestionResolution,
} from '../accept-reject';
import { draftAnchorText, hasCommentDraft } from '../comment-draft';
import { resolveSuggestions } from '../resolve-suggestions';
import { buildThreads, useReviewStore } from '../review-store';
import { CommentThreadCard } from './CommentThreadCard';
import { DraftCommentComposer } from './DraftCommentComposer';
import { SuggestionCard } from './SuggestionCard';

// The review rail lists every open comment thread and suggestion for the
// document. It reads comments from the store and recomputes suggestions from
// the live editor value (via useEditorSelector) so accept/reject and new
// suggestions stay in sync. When there's nothing to review the rail is hidden.
//
// Ordering: comments first, then suggestions. Anchoring each card to its
// position in the document is a deferred follow-up.
export function ReviewRail({
  editor,
  onSuggestionResolved,
}: {
  editor: PlateEditor;
  onSuggestionResolved?: (id: string, resolution: SuggestionResolution) => void;
}) {
  const meta = useReviewStore((s) => s.meta);
  // The rail is the working surface for open threads only; resolved threads live
  // in the right pane's Comments tab (where they can be reopened).
  const threads = useMemo(
    () => buildThreads(meta).filter((thread) => thread.entry.status !== 'resolved'),
    [meta]
  );
  const suggestions = useEditorSelector((ed) => resolveSuggestions(ed.children), []);
  const hasDraft = useEditorSelector((ed) => hasCommentDraft(ed), []);
  const draftText = useEditorSelector((ed) => draftAnchorText(ed), []);

  if (threads.length === 0 && suggestions.length === 0 && !hasDraft) return null;

  return (
    <aside
      aria-label="Review"
      className="mr-6 flex h-full w-80 shrink-0 flex-col gap-2 overflow-auto bg-surface p-3"
      data-testid="review-rail"
    >
      {hasDraft ? <DraftCommentComposer editor={editor} anchorText={draftText} /> : null}
      {threads.map((thread) => (
        <CommentThreadCard key={thread.id} thread={thread} editor={editor} />
      ))}
      {suggestions.map((suggestion) => (
        <SuggestionCard
          key={suggestion.suggestionId}
          onAccept={(id) => {
            acceptSuggestionById(editor, id);
            onSuggestionResolved?.(id, 'accept');
          }}
          onReject={(id) => {
            rejectSuggestionById(editor, id);
            onSuggestionResolved?.(id, 'reject');
          }}
          suggestion={suggestion}
        />
      ))}
    </aside>
  );
}

import { useState } from 'react';
import type { ReactNode } from 'react';

import type {
  AgentReviewComment,
  AgentReviewConflict,
  AgentReviewResponse,
  AgentReviewSuggestion,
} from '../../../api/generated/types';
import { cn } from '../../../lib/utils';
import { reopenComment, useReviewStore } from '../review-store';
import { applyReviewMutation } from '../review-doc';
import { ReviewAuthorHeader } from './ReviewAuthorHeader';

// The Comments tab in the right pane is the document's complete review
// record, read from the rows-backed `GET .../review` projection: comment
// threads (including resolved and orphaned ones), suggestions that left the
// rail, structural suggestions, and diff3 conflict review items from
// whole-file merges. Unlike the rail (which only lists open items bound to
// live marks), this panel shows row states with badges. Structural suggestions
// are resolved here because they deliberately have no inline text mark;
// resolved threads can also be reopened, which returns them to the rail.
// Hovering a thread highlights its in-text mark via the shared review store.

type StatusFilter = 'all' | 'open' | 'resolved';

const filters: Array<{ key: StatusFilter; label: string }> = [
  { key: 'all', label: 'All' },
  { key: 'open', label: 'Open' },
  { key: 'resolved', label: 'Resolved' },
];

function matchesFilter(status: string, filter: StatusFilter): boolean {
  if (filter === 'open') return status === 'open';
  if (filter === 'resolved') return status === 'resolved';
  return true;
}

function emptyLabel(filter: StatusFilter): string {
  if (filter === 'open') return 'No open comments';
  if (filter === 'resolved') return 'No resolved comments';
  return 'No comments';
}

// Orphaned (comments) and invalidated (suggestions) states survive in the
// rows after their anchored text disappears; the badge keeps them honest.
// Resolved already reads from the author header, so it carries no badge.
function StatusBadge({ status }: { status: string }) {
  if (status === 'open' || status === 'resolved') return null;
  return (
    <span
      className="shrink-0 rounded bg-warn-tint px-1.5 py-0.5 text-[0.625rem] font-semibold uppercase tracking-wide text-warn-ink"
      data-testid="review-status-badge"
    >
      {status}
    </span>
  );
}

function CommentItem({ comment }: { comment: AgentReviewComment }) {
  const resolved = comment.status === 'resolved';

  function reopen() {
    applyReviewMutation((meta) => reopenComment(meta, comment.id));
  }

  return (
    <li
      className="group rounded-lg bg-well/40 p-3 transition-colors hover:bg-well/70"
      data-resolved={resolved ? 'true' : 'false'}
      data-status={comment.status}
      data-testid="comments-panel-item"
      onMouseEnter={() => useReviewStore.getState().setHoverId(comment.id)}
      onMouseLeave={() => useReviewStore.getState().setHoverId(null)}
    >
      <div className="flex items-start justify-between gap-2">
        <ReviewAuthorHeader
          at={comment.at}
          by={comment.by}
          editedAt={comment.editedAt}
          resolved={resolved}
        />
        <span className="flex items-center gap-1">
          <StatusBadge status={comment.status} />
          {resolved ? (
            <button
              className="shrink-0 rounded px-2 py-1 text-xs font-medium text-muted opacity-0 transition-opacity hover:bg-well hover:text-body group-hover:opacity-100 focus-visible:opacity-100"
              data-testid="reopen-comment"
              onClick={reopen}
              type="button"
            >
              Reopen
            </button>
          ) : null}
        </span>
      </div>

      {comment.body ? (
        <p className="mt-2 text-sm whitespace-pre-wrap text-body">{comment.body}</p>
      ) : null}

      {comment.replies.length > 0 ? (
        <div className="mt-5 flex flex-col gap-5">
          {comment.replies.map((reply) => (
            <div key={reply.id}>
              <ReviewAuthorHeader at={reply.at} by={reply.by} editedAt={reply.editedAt} />
              <p className="mt-1 text-sm whitespace-pre-wrap text-body">{reply.body}</p>
            </div>
          ))}
        </div>
      ) : null}
    </li>
  );
}

type SuggestionResolution = 'accept' | 'reject';

function SuggestionItem({
  suggestion,
  onResolve,
}: {
  suggestion: AgentReviewSuggestion;
  onResolve?: (suggestionId: string, resolution: SuggestionResolution) => Promise<void>;
}) {
  const [resolving, setResolving] = useState<SuggestionResolution | null>(null);
  const blockDelete = suggestion.kind === 'block_delete';
  const markdownInsert = suggestion.kind === 'markdown_insert';
  const structural = blockDelete || markdownInsert;
  const structuralAction = blockDelete ? 'block-delete' : 'markdown-insert';

  async function resolve(resolution: SuggestionResolution) {
    if (!onResolve || resolving) return;
    setResolving(resolution);
    try {
      await onResolve(suggestion.id, resolution);
    } finally {
      setResolving(null);
    }
  }

  return (
    <li
      className="rounded-lg bg-well/40 p-3"
      data-status={suggestion.status}
      data-testid="comments-panel-suggestion"
    >
      <div className="flex items-start justify-between gap-2">
        <ReviewAuthorHeader
          at={suggestion.at}
          by={suggestion.by}
          resolved={suggestion.status === 'resolved'}
        />
        <StatusBadge status={suggestion.status} />
      </div>
      {markdownInsert ? (
        <div className="mt-2 text-sm text-body">
          <span className="text-muted">Insert Markdown:</span>
          <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap rounded bg-raised p-2 font-mono text-xs text-body">
            {suggestion.preview.after}
          </pre>
        </div>
      ) : blockDelete ? (
        <p className="mt-2 text-sm text-body">
          <span className="text-muted">Delete block:</span>{' '}
          <del className="text-danger/80">{suggestion.preview.before}</del>
        </p>
      ) : (
        <p className="mt-2 text-sm text-body">
          <span className="text-muted">Replace:</span>{' '}
          <del className="text-danger/80">{suggestion.preview.before}</del>{' '}
          <ins className="no-underline text-success">{suggestion.preview.after}</ins>
        </p>
      )}

      {suggestion.body ? (
        <p className="mt-2 text-sm whitespace-pre-wrap text-body">{suggestion.body}</p>
      ) : null}

      {suggestion.replies.length > 0 ? (
        <div className="mt-5 flex flex-col gap-5">
          {suggestion.replies.map((reply) => (
            <div key={reply.id}>
              <ReviewAuthorHeader at={reply.at} by={reply.by} editedAt={reply.editedAt} />
              <p className="mt-1 text-sm whitespace-pre-wrap text-body">{reply.body}</p>
            </div>
          ))}
        </div>
      ) : null}

      {structural && suggestion.status === 'open' && onResolve ? (
        <div className="mt-3 flex justify-end gap-1">
          <button
            className="rounded px-2 py-1 text-xs font-medium text-muted transition-colors hover:bg-well hover:text-body disabled:opacity-50"
            data-testid={`reject-${structuralAction}-suggestion`}
            disabled={resolving !== null}
            onClick={() => void resolve('reject')}
            type="button"
          >
            Reject
          </button>
          <button
            className={cn(
              'rounded px-2 py-1 text-xs font-medium transition-colors disabled:opacity-50',
              blockDelete
                ? 'bg-danger text-white hover:opacity-90'
                : 'bg-accent text-on-accent hover:bg-accent-strong'
            )}
            data-testid={`accept-${structuralAction}-suggestion`}
            disabled={resolving !== null}
            onClick={() => void resolve('accept')}
            type="button"
          >
            {blockDelete ? 'Delete block' : 'Insert'}
          </button>
        </div>
      ) : null}
    </li>
  );
}

interface ConflictItemProps {
  readonly conflict: AgentReviewConflict;
  readonly onDismiss?: (conflictId: string) => Promise<void>;
}

function ConflictItem({ conflict, onDismiss }: ConflictItemProps): ReactNode {
  const [copied, setCopied] = useState(false);
  const [dismissing, setDismissing] = useState(false);

  function copyIncoming() {
    void navigator.clipboard.writeText(conflict.incomingMarkdown).then(() => setCopied(true));
  }

  async function dismiss() {
    if (!onDismiss || dismissing) return;
    setDismissing(true);
    try {
      await onDismiss(conflict.id);
    } finally {
      setDismissing(false);
    }
  }

  return (
    <li
      className="rounded-lg border border-warn-line bg-warn-tint/40 p-3"
      data-status={conflict.status}
      data-testid="comments-panel-conflict"
    >
      <div className="flex items-start justify-between gap-2">
        <ReviewAuthorHeader at={conflict.at} by={conflict.by} />
        <StatusBadge status={conflict.status === 'open' ? 'conflict' : conflict.status} />
      </div>
      <p className="mt-2 text-xs text-muted">
        A file write conflicted with newer edits. The document kept the
        current version; the incoming text is preserved here.
      </p>
      <div className="mt-2 grid gap-2">
        <div>
          <p className="text-[0.625rem] font-semibold uppercase tracking-wide text-muted">Kept</p>
          <pre className="mt-1 overflow-x-auto whitespace-pre-wrap rounded bg-raised p-2 font-mono text-xs text-body">
            {conflict.canonicalMarkdown || '(deleted)'}
          </pre>
        </div>
        <div>
          <p className="text-[0.625rem] font-semibold uppercase tracking-wide text-muted">
            Incoming
          </p>
          <pre className="mt-1 overflow-x-auto whitespace-pre-wrap rounded bg-raised p-2 font-mono text-xs text-body">
            {conflict.incomingMarkdown || '(deleted)'}
          </pre>
        </div>
      </div>
      {conflict.status === 'open' ? (
        <div className="mt-2 flex justify-end gap-1">
          <button
            className="rounded px-2 py-1 text-xs font-medium text-muted transition-colors hover:bg-well hover:text-body"
            data-testid="copy-conflict-incoming"
            onClick={copyIncoming}
            type="button"
          >
            {copied ? 'Copied' : 'Copy incoming'}
          </button>
          {onDismiss ? (
            <button
              className="rounded px-2 py-1 text-xs font-medium text-muted transition-colors hover:bg-well hover:text-body disabled:opacity-50"
              data-testid="dismiss-conflict"
              disabled={dismissing}
              onClick={() => void dismiss()}
              type="button"
            >
              Dismiss
            </button>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}

interface CommentsPanelProps {
  readonly review?: AgentReviewResponse;
  readonly onDismissConflict?: (conflictId: string) => Promise<void>;
  readonly onResolveSuggestion?: (
    suggestionId: string,
    resolution: SuggestionResolution
  ) => Promise<void>;
}

export function CommentsPanel({
  review,
  onDismissConflict,
  onResolveSuggestion,
}: CommentsPanelProps): ReactNode {
  const [filter, setFilter] = useState<StatusFilter>('all');

  const comments = (review?.comments ?? []).filter((comment) =>
    matchesFilter(comment.status, filter)
  );
  // Open inline suggestions live in the rail next to their marks. Structural
  // suggestions have no text mark, so they remain actionable in this panel.
  const suggestions = (review?.suggestions ?? []).filter(
    (suggestion) =>
      (suggestion.kind === 'block_delete' ||
        suggestion.kind === 'markdown_insert' ||
        suggestion.status !== 'open') &&
      matchesFilter(suggestion.status, filter)
  );
  const conflicts = (review?.conflicts ?? []).filter((conflict) =>
    matchesFilter(conflict.status, filter)
  );

  return (
    <div data-testid="comments-panel">
      <div aria-label="Filter comments by status" className="mb-3 inline-flex rounded-md bg-well p-0.5" role="group">
        {filters.map((option) => (
          <button
            aria-pressed={filter === option.key}
            className={cn(
              'rounded px-2.5 py-1 text-xs font-medium transition-colors',
              filter === option.key ? 'bg-raised text-ink shadow-sm' : 'text-muted hover:text-body'
            )}
            data-testid={`comments-filter-${option.key}`}
            key={option.key}
            onClick={() => setFilter(option.key)}
            type="button"
          >
            {option.label}
          </button>
        ))}
      </div>

      {conflicts.length > 0 ? (
        <ul className="mb-3 flex flex-col gap-2">
          {conflicts.map((conflict) => (
            <ConflictItem conflict={conflict} key={conflict.id} onDismiss={onDismissConflict} />
          ))}
        </ul>
      ) : null}

      {comments.length === 0 && conflicts.length === 0 && suggestions.length === 0 ? (
        <p className="text-xs text-muted">{emptyLabel(filter)}</p>
      ) : (
        <ul className="flex flex-col gap-2">
          {comments.map((comment) => (
            <CommentItem comment={comment} key={comment.id} />
          ))}
          {suggestions.map((suggestion) => (
            <SuggestionItem
              key={suggestion.id}
              onResolve={onResolveSuggestion}
              suggestion={suggestion}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

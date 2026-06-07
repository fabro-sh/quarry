import { useState } from 'react';

import { cn } from '../../../lib/utils';
import { AgentAvatar } from '../../agents/AgentAvatar';
import { agentKind } from '../../agents/agents';
import { firstWord, formatRelativeTime, initials } from '../format';
import type { ReviewMetaEntry } from '../rfm-types';
import { buildThreads, reopenComment, useReviewStore, type ReviewThread } from '../review-store';
import { applyReviewMutation } from '../review-doc';

// The Comments tab in the right pane is the document's complete comment record:
// unlike the rail (which only lists open threads), it shows resolved threads too
// and filters by status. It's a read-only overview — the rail stays the place to
// reply or resolve — except that resolved threads can be reopened, which returns
// them to the rail. Hovering a thread highlights its in-text mark, matching the
// rail, via the shared review store.

type StatusFilter = 'all' | 'open' | 'resolved';

const filters: Array<{ key: StatusFilter; label: string }> = [
  { key: 'all', label: 'All' },
  { key: 'open', label: 'Open' },
  { key: 'resolved', label: 'Resolved' },
];

function isResolved(thread: ReviewThread): boolean {
  return thread.entry.status === 'resolved';
}

function matchesFilter(thread: ReviewThread, filter: StatusFilter): boolean {
  if (filter === 'open') return !isResolved(thread);
  if (filter === 'resolved') return isResolved(thread);
  return true;
}

function emptyLabel(filter: StatusFilter): string {
  if (filter === 'open') return 'No open comments';
  if (filter === 'resolved') return 'No resolved comments';
  return 'No comments';
}

function Header({ entry, resolved }: { entry: ReviewMetaEntry; resolved?: boolean }) {
  return (
    <div className="flex items-center gap-2.5">
      <AgentAvatar
        className="bg-surface text-xs font-medium text-muted ring-1 ring-inset ring-line"
        fallback={initials(entry.by)}
        kind={agentKind(entry.by)}
      />
      <div className="flex min-w-0 flex-col">
        <span className="truncate text-sm font-medium leading-tight text-ink" title={entry.by}>{firstWord(entry.by)}</span>
        <span className="text-[11px] leading-tight text-faint">{formatRelativeTime(entry.at)}</span>
      </div>
      {resolved ? <span className="ml-1 text-[11px] font-medium text-muted">Resolved</span> : null}
    </div>
  );
}

function CommentItem({ thread }: { thread: ReviewThread }) {
  const resolved = isResolved(thread);

  function reopen() {
    applyReviewMutation((meta) => reopenComment(meta, thread.id));
  }

  return (
    <li
      className="group rounded-lg bg-well/40 p-3 transition-colors hover:bg-well/70"
      data-resolved={resolved ? 'true' : 'false'}
      data-testid="comments-panel-item"
      onMouseEnter={() => useReviewStore.getState().setHoverId(thread.id)}
      onMouseLeave={() => useReviewStore.getState().setHoverId(null)}
    >
      <div className="flex items-start justify-between gap-2">
        <Header entry={thread.entry} resolved={resolved} />
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
      </div>

      {thread.entry.body ? <p className="mt-2 text-sm whitespace-pre-wrap text-body">{thread.entry.body}</p> : null}

      {thread.replies.length > 0 ? (
        <div className="mt-3 flex flex-col gap-3 border-l border-line pl-3">
          {thread.replies.map((reply) => (
            <div key={reply.id}>
              <Header entry={reply.entry} />
              <p className="mt-1 text-sm whitespace-pre-wrap text-body">{reply.entry.body ?? ''}</p>
            </div>
          ))}
        </div>
      ) : null}
    </li>
  );
}

export function CommentsPanel() {
  const meta = useReviewStore((s) => s.meta);
  const [filter, setFilter] = useState<StatusFilter>('all');

  const visible = buildThreads(meta).filter((thread) => matchesFilter(thread, filter));

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

      {visible.length === 0 ? (
        <p className="text-xs text-muted">{emptyLabel(filter)}</p>
      ) : (
        <ul className="flex flex-col gap-2">
          {visible.map((thread) => (
            <CommentItem key={thread.id} thread={thread} />
          ))}
        </ul>
      )}
    </div>
  );
}

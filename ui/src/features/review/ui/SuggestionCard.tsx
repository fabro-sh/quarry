import { Check, X } from 'lucide-react';
import { nanoid } from 'nanoid';
import { useEffect, useMemo, useRef, useState } from 'react';
import type { TResolvedSuggestion } from '@platejs/suggestion';

import { cn } from '../../../lib/utils';
import { currentAuthor } from '../identity';
import { applyReviewMutation } from '../review-doc';
import { addReply, useReviewStore } from '../review-store';
import { ReviewAuthorHeader } from './ReviewAuthorHeader';

interface SuggestionCardProps {
  suggestion: TResolvedSuggestion;
  onAccept: (id: string) => void;
  onReject: (id: string) => void;
}

const iconButton =
  'inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body';

const labels: Record<TResolvedSuggestion['type'], string> = {
  insert: 'Add',
  remove: 'Delete',
  replace: 'Replace',
  update: 'Format change',
};

function Summary({ suggestion }: { suggestion: TResolvedSuggestion }) {
  const label = <span className="font-medium text-muted">{labels[suggestion.type]}:</span>;

  if (suggestion.type === 'insert') {
    return (
      <p className="text-sm text-body">
        {label} <span className="text-accent-ink">{suggestion.newText}</span>
      </p>
    );
  }

  if (suggestion.type === 'remove') {
    return (
      <p className="text-sm text-body">
        {label} <span className="text-danger line-through">{suggestion.text}</span>
      </p>
    );
  }

  if (suggestion.type === 'replace') {
    return (
      <p className="text-sm text-body">
        {label} <span className="text-danger line-through">{suggestion.text}</span>{' '}
        <span aria-hidden="true">→</span> <span className="text-accent-ink">{suggestion.newText}</span>
      </p>
    );
  }

  return (
    <p className="text-sm text-body">
      {suggestion.newText ? (
        <>
          {label} <span className="text-accent-ink">{suggestion.newText}</span>
        </>
      ) : (
        <span className="font-medium text-muted">{labels[suggestion.type]}</span>
      )}
    </p>
  );
}

export function SuggestionCard({ suggestion, onAccept, onReject }: SuggestionCardProps) {
  const id = suggestion.suggestionId;
  const activeId = useReviewStore((state) => state.activeId);
  const hoverId = useReviewStore((state) => state.hoverId);
  const comments = useReviewStore((state) => state.meta.comments);
  const setActiveId = useReviewStore((state) => state.setActiveId);
  const setHoverId = useReviewStore((state) => state.setHoverId);
  const replies = useMemo(
    () =>
      Object.entries(comments)
        .filter(([, entry]) => entry.re === id && entry.status !== 'resolved')
        .sort(([, a], [, b]) => a.at.localeCompare(b.at)),
    [comments, id]
  );
  const [draft, setDraft] = useState('');
  const [replyFocused, setReplyFocused] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  const isActive = activeId === id;
  const isHover = hoverId === id;

  useEffect(() => {
    if (isActive) ref.current?.scrollIntoView({ block: 'nearest' });
  }, [isActive]);

  function submitReply() {
    const body = draft.trim();
    if (!body) return;
    applyReviewMutation((meta) =>
      addReply(meta, nanoid(), {
        parentId: id,
        body,
        by: currentAuthor(),
        at: new Date().toISOString(),
      })
    );
    setDraft('');
  }

  return (
    <div
      className={cn(
        'group rounded-lg bg-well/40 p-3 transition-colors',
        isHover && !isActive && 'bg-well/70',
        isActive && 'bg-well/70 ring-2 ring-accent-ring'
      )}
      data-active={isActive ? 'true' : 'false'}
      data-hover={isHover ? 'true' : 'false'}
      data-testid="suggestion-card"
      onClick={() => setActiveId(id)}
      onMouseEnter={() => setHoverId(id)}
      onMouseLeave={() => setHoverId(null)}
      ref={ref}
    >
      <div className="flex items-start justify-between gap-2">
        <ReviewAuthorHeader by={suggestion.userId} at={suggestion.createdAt.toISOString()} />
        <div className="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
          <button
            aria-label="Accept suggestion"
            className={cn(iconButton, 'bg-accent-tint text-accent-ink hover:bg-accent-line hover:text-accent-ink')}
            data-testid="rail-accept"
            onClick={(event) => {
              event.stopPropagation();
              onAccept(suggestion.suggestionId);
            }}
            type="button"
          >
            <Check size={16} />
          </button>
          <button
            aria-label="Reject suggestion"
            className={cn(iconButton, 'hover:text-danger')}
            data-testid="rail-reject"
            onClick={(event) => {
              event.stopPropagation();
              onReject(suggestion.suggestionId);
            }}
            type="button"
          >
            <X size={16} />
          </button>
        </div>
      </div>

      <div className="mt-2">
        <Summary suggestion={suggestion} />
      </div>

      {replies.length > 0 ? (
        <div className="mt-5 flex flex-col gap-5">
          {replies.map(([replyId, entry]) => (
            <div key={replyId}>
              <ReviewAuthorHeader by={entry.by} at={entry.at} editedAt={entry.editedAt} />
              <p className="mt-1 text-sm whitespace-pre-wrap text-body">{entry.body ?? ''}</p>
            </div>
          ))}
        </div>
      ) : null}

      {isActive ? (
        <div className="mt-3 flex flex-col gap-2">
          <input
            aria-label="Reply"
            className="w-full rounded-md border border-line bg-raised px-2.5 py-1.5 text-sm text-ink outline-none focus:border-accent"
            data-testid="reply-input"
            onBlur={() => setReplyFocused(false)}
            onChange={(event) => setDraft(event.target.value)}
            onClick={(event) => event.stopPropagation()}
            onFocus={() => setReplyFocused(true)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                submitReply();
              }
            }}
            placeholder="Reply…"
            type="text"
            value={draft}
          />
          {replyFocused || draft.trim().length > 0 ? (
            <div className="flex items-center justify-end gap-2">
              <button
                aria-label="Cancel reply"
                className="rounded-md px-2 py-1.5 text-sm font-medium text-muted transition-colors hover:text-body"
                data-testid="reply-cancel"
                onClick={(event) => {
                  event.stopPropagation();
                  setDraft('');
                  setActiveId(null);
                }}
                onMouseDown={(event) => event.preventDefault()}
                type="button"
              >
                Cancel
              </button>
              <button
                aria-label="Submit reply"
                className="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-on-accent transition-colors hover:bg-accent-strong disabled:cursor-not-allowed disabled:opacity-50"
                data-testid="reply-submit"
                disabled={draft.trim().length === 0}
                onClick={(event) => {
                  event.stopPropagation();
                  submitReply();
                }}
                type="button"
              >
                Reply
              </button>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

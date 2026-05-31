import { Check, X } from 'lucide-react';
import { useEffect, useRef } from 'react';
import type { TResolvedSuggestion } from '@platejs/suggestion';

import { cn } from '../../../lib/utils';
import { formatRelativeTime, initials } from '../format';
import { useReviewStore } from '../review-store';

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
  const label = <span className="font-medium text-muted">{labels[suggestion.type]}</span>;

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
      {label}
      {suggestion.newText ? <> <span className="text-accent-ink">{suggestion.newText}</span></> : null}
    </p>
  );
}

export function SuggestionCard({ suggestion, onAccept, onReject }: SuggestionCardProps) {
  const activeId = useReviewStore((state) => state.activeId);
  const hoverId = useReviewStore((state) => state.hoverId);
  const setActiveId = useReviewStore((state) => state.setActiveId);
  const setHoverId = useReviewStore((state) => state.setHoverId);
  const ref = useRef<HTMLDivElement>(null);

  const id = suggestion.suggestionId;
  const isActive = activeId === id;
  const isHover = hoverId === id;

  useEffect(() => {
    if (isActive) ref.current?.scrollIntoView({ block: 'nearest' });
  }, [isActive]);

  return (
    <div
      className={cn(
        'rounded-lg border border-line bg-raised p-3 transition-colors',
        isHover && !isActive && 'bg-well',
        isActive && 'ring-2 ring-accent-ring'
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
        <div className="flex items-center gap-2">
          <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-well text-xs font-medium text-muted">
            {initials(suggestion.userId)}
          </span>
          <span className="text-sm font-medium text-ink">{suggestion.userId}</span>
          <span className="text-xs text-faint">{formatRelativeTime(suggestion.createdAt.toISOString())}</span>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <button
            aria-label="Accept suggestion"
            className={cn(iconButton, 'text-accent-ink bg-accent-tint')}
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
    </div>
  );
}

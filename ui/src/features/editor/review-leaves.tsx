import { getDraftCommentKey } from '@platejs/comment';
import { PlateLeaf, type PlateLeafProps } from 'platejs/react';
import { useReviewStore } from '../review/review-store';
import { readSuggestionMark } from '../review/suggestion-mark';
import { cn } from '../../lib/utils';

function commentIdOf(leaf: Record<string, unknown>): string | null {
  for (const key of Object.keys(leaf)) {
    if (key.startsWith('comment_') && key !== 'comment_draft' && leaf[key] === true) return key.slice('comment_'.length);
  }
  return null;
}

function isCommentDraft(leaf: Record<string, unknown>): boolean {
  return leaf[getDraftCommentKey()] === true;
}

export function CommentLeaf(props: PlateLeafProps) {
  const id = commentIdOf(props.leaf);
  const isDraft = !id && isCommentDraft(props.leaf);
  const activeId = useReviewStore((s) => s.activeId);
  const hoverId = useReviewStore((s) => s.hoverId);
  const setActiveId = useReviewStore((s) => s.setActiveId);
  const setHoverId = useReviewStore((s) => s.setHoverId);
  const isActive = !!id && activeId === id;
  const isHover = !!id && hoverId === id;
  // A draft is the as-yet-uncommitted range the composer is targeting: show it
  // with a distinct dashed underline so the user sees what they're commenting on.
  const className = isDraft
    ? 'border-b-2 border-dashed border-warn-line bg-warn-tint/60'
    : cn('border-b-2 border-warn-line bg-warn-tint transition-colors', (isActive || isHover) && 'border-warn-ink');
  return (
    <PlateLeaf
      {...props}
      attributes={{
        ...props.attributes,
        'data-comment-id': id ?? undefined,
        'data-comment-draft': isDraft ? 'true' : undefined,
        'data-active': isActive ? 'true' : 'false',
        'data-hover': isHover ? 'true' : 'false',
        onClick: () => {
          if (id) setActiveId(id);
        },
        onMouseEnter: () => {
          if (id) setHoverId(id);
        },
        onMouseLeave: () => setHoverId(null),
      }}
      className={className}
    />
  );
}

export function SuggestionLeaf(props: PlateLeafProps) {
  const data = readSuggestionMark(props.leaf);
  const activeId = useReviewStore((s) => s.activeId);
  const hoverId = useReviewStore((s) => s.hoverId);
  const setActiveId = useReviewStore((s) => s.setActiveId);
  const setHoverId = useReviewStore((s) => s.setHoverId);
  const id = data?.id ?? null;
  const isActive = !!id && activeId === id;
  const isHover = !!id && hoverId === id;
  const className =
    data?.type === 'remove'
      ? cn('text-danger line-through decoration-danger/60', (isActive || isHover) && 'bg-danger/10')
      : cn('text-accent-ink underline decoration-accent-line', (isActive || isHover) && 'bg-accent-tint');
  return (
    <PlateLeaf
      {...props}
      attributes={{
        ...props.attributes,
        'data-suggestion-id': id ?? undefined,
        'data-active': isActive ? 'true' : 'false',
        'data-hover': isHover ? 'true' : 'false',
        onClick: () => {
          if (id) setActiveId(id);
        },
        onMouseEnter: () => {
          if (id) setHoverId(id);
        },
        onMouseLeave: () => setHoverId(null),
      }}
      className={className}
    />
  );
}

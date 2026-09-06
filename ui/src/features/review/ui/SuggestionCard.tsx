import { ReviewDiscussion } from './ReviewDiscussion';
import { CommentEditForm } from './CommentThreadCard';
import type { ReactNode } from 'react';
import type { ProposalView } from '../../editor/document-model';
import { useNativeReview } from '../native-review-context';
import { Check, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';

import { cn } from '../../../lib/utils';
import { ReviewAuthorHeader } from './ReviewAuthorHeader';

interface SuggestionCardProps {
  view: ProposalView;
  children?: ReactNode;
  onAccept: (id: string) => void;
  onReject: (id: string) => void;
}

const iconButton =
  'inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body';

const labels: Record<string, string> = {
  insert: 'Add',
  remove: 'Delete',
  replace: 'Replace',
  update: 'Format change',
  move: 'Move',
};

function Summary({ suggestion }: { suggestion: { type: string; text: string; newText: string } }) {
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

function blockLabel(kind: unknown, attrs: Record<string, unknown>): string {
  if (attrs.listStyleType === 'todo') return attrs.checked ? 'Completed task' : 'Task';
  if (attrs.listStyleType === 'decimal') return 'Numbered list';
  if (attrs.listStyleType) return 'Bulleted list';
  const labels: Record<string, string> = { p: 'Paragraph', blockquote: 'Quote', code_block: 'Code block', code_line: 'Code line',
    table: 'Table', tr: 'Table row', td: 'Table cell', th: 'Table header', mermaid: 'Diagram', img: 'Image', hr: 'Divider', raw_markdown: 'Markdown source' };
  return /^h[1-6]$/.test(String(kind)) ? `Heading ${String(kind).slice(1)}` : labels[String(kind)] ?? 'Block';
}

function BlockUpdatePreview({ view }: { view: ProposalView }) {
  const action = view.proposal.action;
  if (action.kind !== 'update_block') return null;
  const before = action.expected_attrs as Record<string, unknown>;
  const after = action.attrs as Record<string, unknown>;
  const source = action.block_kind === 'mermaid' ? 'code' : action.block_kind === 'raw_markdown' ? 'markdown' : undefined;
  if (source) return <div className="mt-2 space-y-2 text-xs">
    <p className="text-muted">Original source</p><pre aria-label="Original source" className="overflow-auto whitespace-pre-wrap rounded bg-well p-2">{String(before[source] ?? '')}</pre>
    <p className="text-muted">Proposed source</p><pre aria-label="Proposed source" className="overflow-auto whitespace-pre-wrap rounded bg-accent-tint p-2">{String(after[source] ?? '')}</pre>
  </div>;
  const properties: Record<string, string> = { indent: 'Indent', listStart: 'List starts at', lang: 'Language', align: 'Alignment', url: 'Address', alt: 'Image description', width: 'Width', colSizes: 'Column widths' };
  return <div className="mt-2 space-y-1 text-xs text-muted">
    <p>{blockLabel(action.expected_kind, before)} → {blockLabel(action.block_kind, after)}</p>
    {Object.entries(properties).filter(([key]) => JSON.stringify(before[key]) !== JSON.stringify(after[key])).map(([key, label]) =>
      <p key={key}>{label}: {String(before[key] ?? 'Default')} → {String(after[key] ?? 'Default')}</p>)}
  </div>;
}

export function SuggestionCard({ view, children, onAccept, onReject }: SuggestionCardProps) {
  const review = useNativeReview();
  const { proposal } = view;
  const id = proposal.id;
  const { activeId, hoverId, setActiveId, setHoverId } = review;
  const body = proposal.body;
  const original = view.target.attachments.map((part) => part.quote).join('');
  const blockTitle = (id: unknown) => {
    const view = review.document.blocks.find(({ block }) => block.id === id);
    return view?.text || (view ? blockLabel(view.block.kind, view.block.attrs) : 'block');
  };
  const moveSummary = proposal.action.kind === 'move_block'
    ? `${blockTitle(proposal.action.block)} → ${proposal.action.before ? `before ${blockTitle(proposal.action.before)}` : proposal.action.parent ? `end of ${blockTitle(proposal.action.parent)}` : 'end of document'}` : '';
  const suggestion = {
    suggestionId: id, userId: proposal.author, createdAt: proposal.metadata.created_at,
    type: proposal.action.kind === 'move_block' ? 'move' : proposal.action.kind === 'format' || proposal.action.kind === 'update_block' ? 'update' : proposal.action.kind === 'delete_block' || !view.text ? 'remove' : original ? 'replace' : 'insert',
    text: original, newText: moveSummary || (proposal.action.kind === 'format' ? `${proposal.action.value === null || proposal.action.value === false ? 'Remove' : 'Apply'} ${proposal.action.name}` : proposal.action.kind === 'update_block' ? `Change ${blockLabel(proposal.action.block_kind, proposal.action.attrs as Record<string, unknown>).toLowerCase()}` : view.text),
  };
  const [editingBody, setEditingBody] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  const isActive = activeId === id;
  const isHover = hoverId === id;

  useEffect(() => {
    if (isActive) ref.current?.scrollIntoView({ block: 'nearest' });
  }, [isActive]);

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
      aria-label={`Suggestion by ${proposal.author}`}
      onClick={() => setActiveId(id)}
      onMouseEnter={() => setHoverId(id)}
      onMouseLeave={() => setHoverId(null)}
      ref={ref}
    >
      <div className="flex items-start justify-between gap-2">
        <ReviewAuthorHeader by={suggestion.userId} at={suggestion.createdAt} />
        <div className="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
          <button
            disabled={review.readOnly || !!view.acceptance_error || proposal.state !== 'open'}
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
            disabled={review.readOnly || proposal.state !== 'open'}
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
        {proposal.action.kind === 'format' && proposal.action.name === 'link' && typeof proposal.action.value === 'string' &&
          <p className="mt-1 break-all text-xs text-muted">Link address: {proposal.action.value}</p>}
        <BlockUpdatePreview view={view} />
        {view.acceptance_error && <p role="status" className="mt-2 text-sm text-danger">{view.acceptance_error}</p>}
        {children}
      </div>

      {body ? (
        <p className="mt-2 text-sm whitespace-pre-wrap text-body" data-testid="suggestion-body">
          {body}
        </p>
      ) : null}

      {editingBody ? <CommentEditForm label="Suggestion details" original={body} onCancel={() => setEditingBody(false)}
        onSave={(body) => { if (review.command({ op: 'edit_proposal', id, body })) setEditingBody(false); }} />
        : isActive && <button disabled={review.readOnly} className="mt-2 text-xs text-muted hover:text-body" onClick={(event) => { event.stopPropagation(); setEditingBody(true); }}>Edit suggestion details</button>}
      {proposal.state !== 'open' && <p className="mt-2 text-xs capitalize text-muted">{proposal.state}</p>}
      <ReviewDiscussion parent={id} active={isActive} />
    </div>
  );
}

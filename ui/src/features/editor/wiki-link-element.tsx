import { createContext, useContext, type ReactNode } from 'react';
import { TextApi } from 'platejs';
import { PlateElement, useReadOnly, type PlateElementProps } from 'platejs/react';

import { useReviewHighlights } from '../review/native-review-context';
import { textOffset, type ReviewMarker } from './plate-document-projection';
import { cn } from '../../lib/utils';
import { documentAdapter } from './plate-document-adapter';
import { BaseWikiLinkPlugin, convertWikiLinkInText, wikiLinkDisplayLeaves, type WikiLinkNode } from './wiki-link';

export interface WikiResolution {
  resolved: boolean;
  targetPath: string | null;
}

export interface WikiLinkApi {
  // Resolve a wiki-link target to a document (via the backend's outgoing links).
  resolve?: (target: string) => WikiResolution | undefined;
  // Open the resolved document (optionally at an anchor).
  open?: (path: string, anchor?: string) => void;
}

const WikiLinkContext = createContext<WikiLinkApi>({});

export function WikiLinkProvider({ value, children }: { value: WikiLinkApi; children: ReactNode }) {
  return <WikiLinkContext.Provider value={value}>{children}</WikiLinkContext.Provider>;
}

const useWikiLink = () => useContext(WikiLinkContext);

// Atomic, void inline chip for `[[target]]`. Resolved links read as accent
// links; unresolved (broken) links are muted with a dashed underline. Clicking a
// resolved link opens the target document.
export function WikiLinkElement(props: PlateElementProps<WikiLinkNode>) {
  const { element } = props;
  const { resolve, open } = useWikiLink();
  useReadOnly();
  const resolution = resolve?.(element.target);
  const resolved = resolution?.resolved ?? false;
  const path = resolution?.targetPath ?? null;
  const review = useReviewHighlights();
  const entries = JSON.parse(String(element.quarryReviewKey ?? '[]')) as ReviewMarker[];
  const comments = entries.filter(([kind]) => kind === 'comment').map(([, id]) => id);
  const suggestions = entries.filter(([kind]) => kind !== 'comment').map(([, id]) => id);
  const ids = [...comments, ...suggestions];
  const activeId = ids.includes(review.activeId ?? '') ? review.activeId : ids[0];
  let draft = false;
  if (review.draftTarget) {
    const path = props.editor.api.findPath(element);
    if (path) {
      const point = textOffset(props.editor.children, { path: [...path, 0], offset: 0 });
      draft = review.draftTarget.attachments.some((part) => part.owner.id === point.block && part.owner.kind === (point.proposal ? 'proposal' : 'block')
        && part.start < point.offset + (element.quarrySource?.length ?? 0) && part.end > point.offset);
    }
  }

  return (
    <PlateElement {...props} as="span" attributes={{ ...props.attributes, contentEditable: false }}>
      <span
        className={cn(
          'rounded-sm underline decoration-1 underline-offset-2 transition-colors',
          resolved
            ? 'cursor-pointer font-medium text-accent-ink hover:bg-accent-tint'
            : 'cursor-default text-muted decoration-dashed',
          comments.length > 0 && 'bg-warn-tint decoration-warn-line decoration-2',
          suggestions.length > 0 && (entries.some(([kind]) => kind === 'text' || kind === 'delete') ? 'text-danger line-through' : 'bg-accent-tint decoration-accent-line'),
          draft && 'bg-warn-tint ring-1 ring-warn-line',
          ids.some((id) => id === review.activeId || id === review.hoverId) && 'ring-1 ring-accent-ring'
        )}
        data-testid="wikilink"
        data-resolved={resolved}
        data-comment-id={comments[0]}
        data-suggestion-id={suggestions[0]}
        data-comment-draft={draft || undefined}
        onMouseEnter={() => { if (activeId) review.setHoverId(activeId); }}
        onMouseLeave={() => review.setHoverId(null)}
        onClick={() => {
          if (activeId) review.setActiveId(activeId);
          if (path) open?.(path, element.anchor);
        }}
        title={resolved ? (path ?? element.target) : `Unresolved: ${element.target}`}
      >
        {element.embed ? '!' : ''}
        {wikiLinkDisplayLeaves(element).map((leaf, index) => {
          let label: ReactNode = leaf.text;
          if (leaf.bold) label = <strong>{label}</strong>;
          if (leaf.italic) label = <em>{label}</em>;
          if (leaf.strikethrough) label = <s>{label}</s>;
          if (leaf.underline) label = <u>{label}</u>;
          if (leaf.code) label = <code>{label}</code>;
          if (leaf.superscript) label = <sup>{label}</sup>;
          if (leaf.subscript) label = <sub>{label}</sub>;
          return <span key={index}>{label}</span>;
        })}
      </span>
      {props.children}
    </PlateElement>
  );
}

// Turn a completed `[[...]]` into a chip as you type, via normalization.
export const WikiLinkPlugin = BaseWikiLinkPlugin.withComponent(WikiLinkElement).overrideEditor(
  ({ editor, tf: { normalizeNode } }) => ({
    transforms: {
      normalizeNode(entry) {
        const [node, path] = entry;
        const adapter = documentAdapter(editor);
        if (TextApi.isText(node) && convertWikiLinkInText(editor, node, path, adapter ? (at, text, action) => adapter.preserveInlineSyntax(at, text, action) : undefined)) return;
        normalizeNode(entry);
      },
    },
  })
);

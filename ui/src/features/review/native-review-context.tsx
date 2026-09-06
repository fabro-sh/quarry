import { createContext, useContext } from 'react';
import type { Command, DocumentView, ReviewTarget, TargetFragment, TextRange } from '../editor/document-model';

export interface CommentDraft { base: string[]; ranges: TextRange[]; quote: string; fragments: TargetFragment[] }
export interface NativeReviewState {
  document: DocumentView;
  author: string;
  readOnly: boolean;
  activeId: string | null;
  hoverId: string | null;
  draftTarget?: ReviewTarget;
  setActiveId(id: string | null): void;
  setHoverId(id: string | null): void;
  command(...commands: Command[]): boolean;
  focusTarget(target: ReviewTarget): void;
}

export const NativeReviewContext = createContext<NativeReviewState | undefined>(undefined);
export function useNativeReview() {
  const value = useContext(NativeReviewContext);
  if (!value) throw new Error('Review controls require an open document');
  return value;
}

/** Inline highlights need only interaction state. Body edits must not
 * propagate a new review context through every block in a large document. */
export type ReviewHighlights = Pick<NativeReviewState, 'activeId' | 'hoverId' | 'draftTarget' | 'setActiveId' | 'setHoverId'>;
export const ReviewHighlightsContext = createContext<ReviewHighlights | undefined>(undefined);
export function useReviewHighlights() {
  const value = useContext(ReviewHighlightsContext);
  if (!value) throw new Error('Review highlights require an open document');
  return value;
}

/** Presentation data only. All changes go through native commands. */
export interface ReviewThread {
  id: string;
  entry: { body: string; by: string; at: string; editedAt?: string; status: string };
  replies: Array<{ id: string; entry: ReviewThread['entry'] }>;
}

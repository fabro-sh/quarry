import type { Descendant } from 'platejs';

import { reviewToMarkdown } from '../review/rfm-codec';
import { syncSuggestionsFromValue } from '../review/review-store';
import type { ReviewMeta } from '../review/rfm-types';

/**
 * The mirror serialization: Plate value + review metadata → Markdown (RFM),
 * with any suggestion marks Plate created mirrored into the metadata so they
 * survive the round-trip. Pure data in, string out — runs identically on the
 * main thread and inside the mirror worker.
 */
export function serializeMirror(value: Descendant[], meta: ReviewMeta): string {
  return reviewToMarkdown(value, syncSuggestionsFromValue(meta, value));
}

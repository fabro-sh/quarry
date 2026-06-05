/** Metadata for one review item, stored in YAML endmatter, keyed by id. */
export interface ReviewMetaEntry {
  /** Free-form author label: "user", "AI", or an agent name. */
  by: string;
  /** ISO 8601 timestamp. */
  at: string;
  /** Markdown body — used by replies and by comments not stored inline. */
  body?: string;
  /** Parent id (this entry is a reply to that id). */
  re?: string;
  /** Review state. */
  status?: 'resolved';
  /** Optional resolution summary. */
  resolved?: string;
}

/** The parsed review endmatter: two id-keyed maps. */
export interface ReviewMeta {
  comments: Record<string, ReviewMetaEntry>;
  suggestions: Record<string, ReviewMetaEntry>;
}

export interface ReviewMetaPatch {
  comments?: Record<string, ReviewMetaEntry>;
  suggestions?: Record<string, ReviewMetaEntry>;
}

export function emptyReviewMeta(): ReviewMeta {
  return { comments: {}, suggestions: {} };
}

export function cloneMeta(meta: ReviewMeta): ReviewMeta {
  return { comments: { ...meta.comments }, suggestions: { ...meta.suggestions } };
}

export function isEmptyReviewMeta(meta: ReviewMeta): boolean {
  return Object.keys(meta.comments).length === 0 && Object.keys(meta.suggestions).length === 0;
}

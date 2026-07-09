// Scope-resolved addressing for document-scoped operations. Library documents
// address by library + path, tmp documents by capability secret, but the
// document semantics behind both are identical (same block store, review
// projection, version history). Client functions and SWR cache keys take a
// DocumentRef so a feature wired once works for both scopes — the pattern
// that prevents "wired for library, forgotten for tmp" gaps.
export type DocumentRef =
  | { readonly scope: 'library'; readonly library: string; readonly path: string }
  | { readonly scope: 'tmp'; readonly secret: string };

export function libraryDocumentRef(library: string, path: string): DocumentRef {
  return { scope: 'library', library, path };
}

export function tmpDocumentRef(secret: string): DocumentRef {
  return { scope: 'tmp', secret };
}

/** The document's REST URL, plus an optional subresource suffix like `/review`. */
export function documentRefUrl(ref: DocumentRef, suffix = ''): string {
  const base =
    ref.scope === 'tmp'
      ? `/v1/tmp/documents/${segment(ref.secret)}`
      : `/v1/libraries/${segment(ref.library)}/documents/${pathSegments(ref.path)}`;
  return `${base}${suffix}`;
}

/**
 * The SWR cache key for one document-scoped operation. Replaces the twin
 * `'/v1/x'` / `'/v1/tmp-x'` string keys; callers append extra parts (e.g. a
 * version id) by spreading: `[...documentRefKey('version-content', ref), id]`.
 */
export function documentRefKey(operation: string, ref: DocumentRef): string[] {
  return ref.scope === 'tmp'
    ? ['doc', operation, 'tmp', ref.secret]
    : ['doc', operation, ref.library, ref.path];
}

/** The workspace's path-like identifier: the library path or the tmp secret. */
export function documentRefPath(ref: DocumentRef): string {
  return ref.scope === 'tmp' ? ref.secret : ref.path;
}

export function segment(value: string): string {
  return encodeURIComponent(value);
}

export function pathSegments(path: string): string {
  return path.split('/').map(segment).join('/');
}

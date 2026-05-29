export interface DraftRecord {
  library: string;
  path: string;
  etag: string;
  content: string;
  savedAt: string;
}

export function draftKey(library: string, path: string, etag: string) {
  return `quarry:draft:${library}:${encodeURIComponent(path)}:${encodeURIComponent(etag)}`;
}

export function saveDraft(library: string, path: string, etag: string, content: string) {
  const draft: DraftRecord = {
    library,
    path,
    etag,
    content,
    savedAt: new Date().toISOString(),
  };
  localStorage.setItem(draftKey(library, path, etag), JSON.stringify(draft));
}

export function loadDraft(library: string, path: string, etag: string): DraftRecord | null {
  const raw = localStorage.getItem(draftKey(library, path, etag));
  if (!raw) return null;
  try {
    return JSON.parse(raw) as DraftRecord;
  } catch {
    return null;
  }
}

export function clearDraft(library: string, path: string, etag: string) {
  localStorage.removeItem(draftKey(library, path, etag));
}

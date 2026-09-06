import { useSyncExternalStore, type ReactNode } from 'react';

let current: { owner: string; panel: ReactNode } | undefined;
const listeners = new Set<() => void>();
export function publishReviewPanel(owner: string, panel: ReactNode) {
  current = { owner, panel };
  for (const listener of listeners) listener();
}
export function clearReviewPanel(owner: string) {
  if (current?.owner !== owner) return;
  current = undefined;
  for (const listener of listeners) listener();
}
export function DocumentReviewPanel() {
  const value = useSyncExternalStore((listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; }, () => current);
  return value?.panel ?? <p className="text-sm text-muted">Open a document to review it.</p>;
}

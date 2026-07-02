/**
 * Prompts before the tab closes while local edits are not yet durable —
 * covering both a checkpoint still in flight (the debounce window) and a
 * persistently failing one ("Save failed"). `isDirty` is read at close time
 * so callers can hand in a live ref instead of re-registering per change.
 */
export function registerUnloadGuard(target: Window, isDirty: () => boolean): () => void {
  const handler = (event: BeforeUnloadEvent) => {
    if (!isDirty()) return;
    event.preventDefault();
    // Legacy engines require a set returnValue for the prompt to appear.
    event.returnValue = '';
  };
  target.addEventListener('beforeunload', handler);
  return () => target.removeEventListener('beforeunload', handler);
}

interface Actions { acceptAllSuggestions(): void; markdown?: () => string }
const documents = new Map<string, Actions>();

export function registerDocumentActions(id: string, actions: Actions) {
  documents.set(id, actions);
  return () => { if (documents.get(id) === actions) documents.delete(id); };
}

/** Application controls use the same native commands and outbox as the editor. */
export function acceptAllDocumentSuggestions(id: string) {
  const actions = documents.get(id);
  if (!actions) throw new Error('The document is still opening');
  actions.acceptAllSuggestions();
}

export function currentDocumentMarkdown(id: string) { return documents.get(id)?.markdown?.(); }

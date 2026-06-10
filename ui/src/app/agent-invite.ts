interface TokenizedDocumentUrlParams {
  origin: string;
  library: string;
  path: string;
  token: string;
}

interface AddAgentPromptParams {
  origin: string;
  library: string;
  path: string;
  tokenizedDocUrl: string;
}

export function workspaceRouteForDocument(library: string, path: string) {
  const libraryPath = `/lib/${encodeURIComponent(library)}`;
  if (!path) return libraryPath;
  return `${libraryPath}/documents/${pathSegments(path)}`;
}

export function buildTokenizedDocumentUrl({
  origin,
  library,
  path,
  token,
}: TokenizedDocumentUrlParams) {
  const url = new URL(workspaceRouteForDocument(library, path), normalizedOrigin(origin));
  url.searchParams.set('token', token);
  return url.toString();
}

export function buildAddAgentPrompt({
  origin,
  library,
  path,
  tokenizedDocUrl,
}: AddAgentPromptParams) {
  const apiBase = `${normalizedOrigin(origin)}/v1`;
  const librarySegment = encodeURIComponent(library);
  const documentPath = pathSegments(path);
  const documentApi = `${apiBase}/libraries/${librarySegment}/documents/${documentPath}`;
  const libraryApi = `${apiBase}/libraries/${librarySegment}`;
  const discoveryOrigin = normalizedOrigin(origin);

  return `Quarry is a local-first collaborative Markdown editor with presence, comments, suggestions, and block edit APIs.

Join this Quarry document using this locator URL:
${tokenizedDocUrl}

Quarry local REST APIs are trusted-localhost for now. The token in the URL identifies the shared document for browser/collab join, but REST agent endpoints on this host do not currently enforce bearer-token auth.

API base: ${apiBase}
Library: ${library}
Document path: ${path}

1. Register presence first.
   Choose an agent id like ai:codex:<short-id> or ai:claude:<short-id>.
   POST ${documentApi}/presence
   Headers:
   - Content-Type: application/json
   - X-Agent-Id: <agent-id>
   Body:
   {"status":"reading","by":"<agent name>"}

2. Read the current document.
   Prefer GET ${documentApi}/blocks (stable block_ids plus the current document_clock)
   Fallback GET ${documentApi}

3. After reading, reply to the user with exactly this shape:
   Connected in Quarry and ready.
   <one-sentence summary of the document>
   I can edit directly, or leave comments and suggestions for you to review. What would you like me to do?

4. While working, monitor document activity.
   Prefer GET ${documentApi}/events/stream
   If you cannot keep a stream open, poll GET ${libraryApi}/events/pending?after=<last-seen-id>.
   When an event arrives, re-read the block tree before replying or editing.

5. Do not edit until the user gives further instructions.
   For every edit and review operation, POST ${documentApi}/transactions with {"client_tx_id":"<unique-id>","base_clock":"<document_clock>","actor":{"kind":"agent","id":"<agent-id>"},"ops":[...]}.
   Ops: insert_block, delete_block, move_block, replace_block_content, set_block_attrs, mark/link ops, comment.add, comment.reply, comment.resolve, comment.delete, suggestion.add, suggestion.accept, suggestion.reject.
   To read existing comments, suggestions, and merge conflicts, GET ${documentApi}/review.
   Errors are typed {code, retryable, message}; when retryable, refresh GET ${documentApi}/blocks and resubmit with the new document_clock.

6. If you need setup details for deeper interaction, fetch:
   Skill: ${discoveryOrigin}/quarry.SKILL.md
   Docs: ${discoveryOrigin}/agent-docs
   Discovery: ${discoveryOrigin}/.well-known/agent.json`;
}

function normalizedOrigin(origin: string) {
  return origin.replace(/\/+$/, '');
}

function pathSegments(path: string) {
  return path.split('/').map(encodeURIComponent).join('/');
}

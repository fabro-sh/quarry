export type ActorKind = "human" | "agent" | "git_import" | "system" | "integration";

export interface Actor {
  id: string;
  display_name: string;
  kind: ActorKind;
  avatar_url?: string | null;
}

export interface TreeEntry {
  path: string;
  object_kind: "blob" | "structured_doc" | "binary_object";
  object_id: string;
  size?: number | null;
  media_type?: string | null;
}

export interface RefRecord {
  name: string;
  entries: TreeEntry[];
  updated_at: string;
}

export interface WorkspaceStatus {
  workspace_id: string;
  data_dir: string;
  refs: string[];
  blob_count: number;
  transaction_count: number;
  annotation_count: number;
  event_count: number;
  snapshot_count: number;
  document_count: number;
  schema_version: number;
  git_sync: string;
}

export interface WriteTextRequest {
  content: string;
  message?: string;
  actor?: Actor;
}

export interface TreeEntryResponse {
  entry: TreeEntry;
  content_text?: string | null;
}

export interface EventRecord {
  id: string;
  kind: string;
  actor?: Actor | null;
  target?: string | null;
  transaction_id?: string | null;
  payload: unknown;
  created_at: string;
}

export interface RefSnapshotRecord {
  id: string;
  ref_name: string;
  entries: TreeEntry[];
  transaction_id?: string | null;
  actor?: Actor | null;
  message?: string | null;
  created_at: string;
}

export interface StructuredDocumentRecord {
  id: string;
  ref_name: string;
  path: string;
  title: string;
  snapshot_json: unknown;
  created_at: string;
  updated_at: string;
}

export interface DocumentSnapshotRecord {
  id: string;
  document_id: string;
  snapshot_json: unknown;
  transaction_id?: string | null;
  actor?: Actor | null;
  message?: string | null;
  created_at: string;
}

export interface DocumentOpRecord {
  id: string;
  document_id: string;
  actor: Actor;
  op_json: unknown;
  created_at: string;
}

export interface PresenceRecord {
  document_id: string;
  actor: Actor;
  cursor_json: unknown;
  updated_at: string;
}

export interface DocumentState {
  document: StructuredDocumentRecord;
  snapshots: DocumentSnapshotRecord[];
  ops: DocumentOpRecord[];
  presence: PresenceRecord[];
}

export class QuarryClient {
  readonly baseUrl: string;
  readonly actor?: Actor;

  constructor(options: { baseUrl?: string; actor?: Actor } = {}) {
    this.baseUrl = (options.baseUrl ?? "http://127.0.0.1:7831").replace(/\/$/, "");
    this.actor = options.actor;
  }

  status(): Promise<WorkspaceStatus> {
    return this.get("/stats");
  }

  refs(): Promise<RefRecord[]> {
    return this.get("/refs");
  }

  tree(ref = "published/main"): Promise<RefRecord> {
    return this.get(`/tree/${encodeURIComponent(ref)}`);
  }

  read(ref: string, path: string): Promise<TreeEntryResponse> {
    return this.get(`/tree/${encodeURIComponent(ref)}/${encodePath(path)}`);
  }

  write(ref: string, path: string, request: WriteTextRequest): Promise<unknown> {
    return this.request(`/tree/${encodeURIComponent(ref)}/${encodePath(path)}`, {
      method: "PUT",
      body: JSON.stringify(request),
    });
  }

  startDraft(baseRef = "published/main", name?: string): Promise<unknown> {
    return this.request("/drafts", {
      method: "POST",
      body: JSON.stringify({ base_ref: baseRef, name, actor: this.actor }),
    });
  }

  publishDraft(sourceRef: string, targetRef = "published/main"): Promise<unknown> {
    return this.request("/drafts/publish", {
      method: "POST",
      body: JSON.stringify({ source_ref: sourceRef, target_ref: targetRef, actor: this.actor }),
    });
  }

  comment(target: string, body: string): Promise<unknown> {
    return this.request("/annotations", {
      method: "POST",
      body: JSON.stringify({ target, body, actor: this.actor }),
    });
  }

  events(options: { limit?: number; target?: string } = {}): Promise<EventRecord[]> {
    const params = new URLSearchParams();
    if (options.limit) params.set("limit", String(options.limit));
    if (options.target) params.set("target", options.target);
    const query = params.toString();
    return this.get(`/events${query ? `?${query}` : ""}`);
  }

  refSnapshots(ref: string, limit = 50): Promise<RefSnapshotRecord[]> {
    return this.get(`/refs/${encodeURIComponent(ref)}/snapshots?limit=${limit}`);
  }

  restoreRef(ref: string, snapshotId: string): Promise<RefRecord> {
    return this.request(`/refs/${encodeURIComponent(ref)}/restore`, {
      method: "POST",
      body: JSON.stringify({ snapshot_id: snapshotId, actor: this.actor }),
    });
  }

  createDocument(request: {
    ref?: string;
    path: string;
    title?: string;
    text?: string;
    snapshot?: unknown;
    message?: string;
  }): Promise<unknown> {
    return this.request("/documents", {
      method: "POST",
      body: JSON.stringify({ ...request, actor: this.actor }),
    });
  }

  documentState(id: string): Promise<DocumentState> {
    return this.get(`/documents/${encodeURIComponent(id)}/state`);
  }

  documentOp(id: string, op: unknown): Promise<unknown> {
    return this.request(`/documents/${encodeURIComponent(id)}/ops`, {
      method: "POST",
      body: JSON.stringify({ op, actor: this.actor }),
    });
  }

  updatePresence(id: string, cursor: unknown): Promise<PresenceRecord> {
    return this.request(`/documents/${encodeURIComponent(id)}/presence`, {
      method: "POST",
      body: JSON.stringify({ cursor, actor: this.actor }),
    });
  }

  binaryContentUrl(id: string): string {
    return `${this.baseUrl}/binary-objects/${encodeURIComponent(id)}/content`;
  }

  mcpRpc<TOutput = unknown>(method: string, params?: unknown, id: unknown = 1): Promise<TOutput> {
    return this.request<{ result: TOutput }>("/mcp", {
      method: "POST",
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
    }).then((response) => response.result);
  }

  mcpTool<TInput extends object, TOutput = unknown>(
    tool: string,
    input: TInput,
  ): Promise<{ tool: string; result: TOutput }> {
    return this.request(`/mcp/tools/${encodeURIComponent(tool)}`, {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  private get<T>(path: string): Promise<T> {
    return this.request<T>(path, { method: "GET" });
  }

  private async request<T>(path: string, init: RequestInit): Promise<T> {
    const headers = new Headers(init.headers);
    headers.set("content-type", "application/json");
    if (this.actor) {
      headers.set("x-quarry-actor-id", this.actor.id);
      headers.set("x-quarry-actor-name", this.actor.display_name);
      headers.set("x-quarry-actor-kind", this.actor.kind);
      if (this.actor.avatar_url) headers.set("x-quarry-actor-avatar-url", this.actor.avatar_url);
    }

    const response = await fetch(`${this.baseUrl}${path}`, { ...init, headers });
    if (!response.ok) {
      const error = await response.json().catch(() => ({ error: response.statusText }));
      throw new Error(error.error ?? response.statusText);
    }
    return response.json() as Promise<T>;
  }
}

function encodePath(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/");
}

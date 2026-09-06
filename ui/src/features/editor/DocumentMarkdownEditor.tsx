import type { DocumentSelection } from './document-presence';
import { useEffect, useMemo, useRef, useState } from 'react';
import { DocumentSession, type SessionStatus } from './document-session';
import type { Command, DocumentBatch, DocumentView, ReviewTarget } from './document-model';
import type { DocumentEditorConfig, EditorMode, ImageApi, WikiLinkApi } from './editor-types';
import { PlateMarkdownEditor } from './PlateMarkdownEditor';
import { PlateDocumentAdapter } from './plate-document-adapter';
import { slatePoint } from './plate-document-projection';
import { NativeReviewContext, type CommentDraft, type NativeReviewState } from '../review/native-review-context';
import { NativeReviewPanel } from '../review/ui/NativeReviewPanel';
import { publishReviewPanel, clearReviewPanel } from './document-review-panel';
import { registerDocumentActions } from './document-actions';
import { DocumentArchiveMenu, download } from './document-archive-menu';

interface Props { documentUrl: string; config: DocumentEditorConfig; author: string; mode: EditorMode; image?: ImageApi; wikiLink?: WikiLinkApi; reviewInPanel?: boolean }
interface Connection { session: DocumentSession; adapter?: PlateDocumentAdapter; started: boolean }

/** Plate owns editor interactions. The native document owns durable state. */
export function DocumentMarkdownEditor(props: Props) {
  const current = useRef(props); current.current = props;
  const active = useRef<Connection | undefined>(undefined);
  const [connection, setConnection] = useState<Connection>();
  const [document, setDocument] = useState<DocumentView>();
  const [status, setStatus] = useState<SessionStatus>({ state: 'saved', failed: [], blocked: false, sync: 'refreshing', readOnly: false });
  const blocked = useRef(false);
  const [error, setError] = useState('');
  const [peers, setPeers] = useState<DocumentSelection[]>([]);
  const [retry, setRetry] = useState(0);
  const [draft, setDraft] = useState<CommentDraft>();
  const [activeId, setActiveId] = useState<string | null>(null);
  const [hoverId, setHoverId] = useState<string | null>(null);

  const publish = (session: DocumentSession) => {
    const projection = session.model.view(); setDocument(projection);
    current.current.config.onTitleChange?.(projection.blocks.find((view) => view.block.kind === 'h1')?.text ?? null);
  };
  useEffect(() => {
    let cancelled = false;
    let opened: Connection | undefined;
    let unregister: (() => void) | undefined;
    setError(''); setPeers([]); setDocument(undefined); setConnection(undefined); setDraft(undefined); setActiveId(null); setHoverId(null); blocked.current = false;
    void DocumentSession.open(props.documentUrl, props.config.documentId, props.author, props.config.sessionId, props.config.token).then((session) => {
      if (cancelled) { void session.close().then(() => session.model.dispose()); return; }
      opened = { session, started: false }; active.current = opened; setConnection(opened);
      session.onPresence = setPeers;
      blocked.current = session.readOnly;
      session.onStatus = (state) => {
        blocked.current = session.readOnly || !!state.blocked;
        setStatus(state); current.current.config.onSaveStateChange?.(state.state);
        if (state.error) setError(state.error);
      };
      unregister = registerDocumentActions(props.config.documentId, {
        markdown: () => { opened?.adapter?.flush(); return session.model.markdown(); },
        acceptAllSuggestions() {
          if (blocked.current || !opened?.adapter) throw new Error('The document is not editable');
          opened.adapter.command(session.model.view().proposals.filter((view) => view.proposal.state === 'open' && !view.acceptance_error)
            .map((view) => ({ op: 'accept_proposal', id: view.proposal.id })));
        },
      });
      publish(session);
    }).catch((error) => { if (!cancelled) setError(String(error)); });
    return () => {
      cancelled = true; unregister?.();
      if (opened) { opened.adapter?.flush(); void opened.session.close().then(() => opened!.session.model.dispose()); }
      if (active.current === opened) active.current = undefined;
    };
  }, [props.documentUrl, props.config.documentId, props.config.sessionId, props.config.token, props.author, retry]);

  const attempt = (action: () => void): boolean => {
    setError('');
    try { action(); return true; } catch (error) { setError(String(error)); return false; }
  };
  const command = (...commands: Command[]) => attempt(() => {
    if (blocked.current || !active.current?.adapter) throw new Error('The document is not editable');
    active.current.adapter.command(commands);
  });
  const changed = (batch: DocumentBatch) => {
    const connection = active.current;
    if (!connection) return;
    connection.session.enqueue(batch); publish(connection.session);
  };
  const focusTarget = (target: ReviewTarget) => {
    const adapter = active.current?.adapter;
    const part = target.attachments[0];
    if (!adapter || !part) return;
    const anchor = slatePoint(adapter.editor.children, part.owner.id, part.start, part.owner.kind === 'proposal'), focus = slatePoint(adapter.editor.children, part.owner.id, part.end, part.owner.kind === 'proposal');
    if (anchor && focus) adapter.editor.tf.select({ anchor, focus });
    adapter.editor.tf.focus();
  };
  const beginComment = () => attempt(() => {
    const adapter = active.current!.adapter!;
    const selection = adapter.beginComment();
    setDraft({ ...selection, ...adapter.model.captureTarget(selection.ranges) });
    current.current.config.onReviewOpen?.();
  });
  const submitDraft = (body: string) => attempt(() => {
    if (!draft || blocked.current) throw new Error('The document is not editable');
    const id = crypto.randomUUID();
    active.current!.adapter!.command([{ op: 'add_comment', id, body, author: props.author, ranges: draft.ranges }], draft.base);
    setDraft(undefined); setActiveId(id);
  });
  const downloadDraft = () => {
    const session = active.current!.session;
    download('quarry-draft.json', JSON.stringify({ document: session.model.view(), bytes: Array.from(session.model.save()),
      drafts: status.failed.map((entry) => ({ ...entry, draft: Array.from(entry.draft) })) }));
  };

  const draftTarget = useMemo((): ReviewTarget | undefined => {
    if (!draft || !connection) return;
    try { return connection.session.model.resolveTarget(draft.fragments); }
    catch { return { state: 'unattached', attachments: [] }; }
  }, [connection, draft, document]);
  const review: NativeReviewState | undefined = document ? { document, author: props.author, readOnly: blocked.current, activeId, hoverId, draftTarget, setActiveId, setHoverId, command, focusTarget } : undefined;
  const panel = review && <NativeReviewContext.Provider value={review}>
    <NativeReviewPanel draft={draft} submitDraft={submitDraft} cancelDraft={() => setDraft(undefined)}
      acceptIncoming={(id) => attempt(() => active.current!.session.acceptIncomingConflict(id))} />
  </NativeReviewContext.Provider>;
  useEffect(() => { if (props.reviewInPanel) publishReviewPanel(props.config.documentId, panel); });
  useEffect(() => () => clearReviewPanel(props.config.documentId), [props.config.documentId]);

  return <div className="quarry-document-editor relative flex h-full min-h-0 flex-col">
    {error && <div role="alert" className="flex gap-3 border-b border-line px-4 py-2 text-sm text-danger">{error}
      {!document && <button onClick={() => setRetry((value) => value + 1)}>Try again</button>}
      <button onClick={() => setError('')}>Dismiss</button>
    </div>}
    {status.state === 'save_failed' && document && <div role="status" className="border-b border-line px-4 py-2 text-sm text-body">
      <p>{status.blocked ? 'Some changes could not be applied. Your draft is kept on this device.' : 'Waiting to reconnect. Changes are kept on this device.'}</p>
      <button onClick={downloadDraft}>Download draft</button>
      {status.blocked && <button onClick={() => void active.current!.session.useSavedVersion().catch((error) => setError(String(error)))}>Use saved version</button>}
    </div>}
    {document && (status.sync === 'reconnecting' || status.sync === 'unavailable') && <div role="status" aria-label="Update status" className="border-b border-line px-4 py-2 text-sm text-muted">
      {status.sync === 'reconnecting' ? 'Reconnecting for updates…' : 'Document updates are unavailable.'}
    </div>}
    {!document && !error && <p role="status" className="p-8">Loading document…</p>}
    {connection && document && review && <>
      <DocumentArchiveMenu session={connection.session} documentUrl={props.documentUrl} onError={(error) => setError(String(error))} />
      <div className="flex min-h-0 flex-1">
        <div className="quarry-document-body min-h-0 min-w-0 flex-1">
          <PlateMarkdownEditor model={connection.session.model} review={review} mode={blocked.current ? 'viewing' : props.mode} peers={peers} image={props.image} wikiLink={props.wikiLink}
            options={{ changed, pending: connection.session.setBufferedEdit, proposed: (id) => { setActiveId(id); current.current.config.onReviewOpen?.(); }, error: (error) => setError(String(error)), selection: (points) => connection.session.setSelection(points), readOnly: () => blocked.current,
              mode: () => current.current.mode, author: () => current.current.author }}
            onReady={(adapter) => {
              connection.adapter = adapter;
              connection.session.onRemote = (bytes, base, heads) => {
                adapter.flush(); adapter.rememberSelection(true); connection.session.model.merge(bytes, base, heads); adapter.refresh(); publish(connection.session);
              };
              if (!connection.started) { connection.started = true; connection.session.start(); }
              return () => { if (connection.adapter === adapter) connection.adapter = undefined; };
            }}
            onComment={beginComment}
            onCompositionChange={(composing) => {
              if (!composing) connection.adapter?.flush();
              connection.session.setComposing(composing);
            }} />
        </div>
        {!props.reviewInPanel && <div className="w-80 shrink-0 border-l border-line">{panel}</div>}
      </div>
    </>}
  </div>;
}

// Gate B spike: a Node-side Yjs collaborator standing in for the future
// server-side semantic mutation gateway.
//
// It connects to the same `/v1/collab/{documentId}` websocket room the
// browsers join, speaking the same y-sync v1 protocol (the browser stack wraps
// this exact y-websocket WebsocketProvider in
// `ui/src/features/collab/rust-ws-provider.ts`; the server side is yrs
// `DefaultProtocol` in `crates/quarry-server/src/collab.rs`). Because it owns
// a separate Y.Doc it automatically gets its own random Yjs client ID, and its
// block mutation runs inside a single transaction tagged with AGENT_ORIGIN.
//
// Attribution note for the findings doc: the transaction origin is local to
// this process's Y.Doc. Only update bytes cross the wire, and the server
// applies them via `DefaultProtocol::handle_update` -> `transact_mut()` with
// no origin. Checkpoint attribution therefore cannot ride Yjs origins; the
// server must track which websocket connection delivered which update.

import { WebsocketProvider } from 'y-websocket';
import * as Y from 'yjs';

export const AGENT_ORIGIN = 'quarry:spike-agent-collaborator';

interface DeltaOp {
  insert?: unknown;
}

export class AgentCollaborator {
  readonly doc: Y.Doc;
  private readonly provider: WebsocketProvider;

  constructor(baseUrl: string, documentId: string) {
    this.doc = new Y.Doc();
    this.provider = new WebsocketProvider(baseUrl, documentId, this.doc, {
      // Playwright specs run under Node, whose global WebSocket is compatible
      // with y-websocket's expectations (binaryType, on* handlers).
      WebSocketPolyfill: WebSocket,
      disableBc: true,
    });
  }

  get clientId(): number {
    return this.doc.clientID;
  }

  /** Awareness client IDs of the other participants in the room. */
  remoteClientIds(): number[] {
    return [...this.provider.awareness.getStates().keys()].filter(
      (id) => id !== this.doc.clientID
    );
  }

  /**
   * Ask the server for the room's current awareness states. y-websocket only
   * broadcasts its own state on connect — it never queries — so a client that
   * joins after the browsers would otherwise wait for their next periodic
   * awareness renewal (~15s).
   */
  queryAwareness(): void {
    const ws = this.provider.ws;
    // messageQueryAwareness = 3, encoded as a single varuint byte.
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(new Uint8Array([3]));
  }

  async whenSynced(timeoutMs = 15_000): Promise<void> {
    if (this.provider.synced) return;
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        cleanup();
        reject(new Error('agent collaborator did not sync in time'));
      }, timeoutMs);
      const onSync = (isSynced: boolean) => {
        if (!isSynced) return;
        cleanup();
        resolve();
      };
      const cleanup = () => {
        clearTimeout(timer);
        this.provider.off('sync', onSync);
      };
      this.provider.on('sync', onSync);
    });
  }

  /** Plain text of each top-level block, in document order. */
  blockTexts(): string[] {
    return rootBlocks(this.doc).map(blockText);
  }

  blockTextAt(index: number): string {
    const block = rootBlocks(this.doc)[index];
    if (!block) throw new Error(`no block at index ${index}`);
    return blockText(block);
  }

  findBlockIndex(marker: string): number {
    const index = this.blockTexts().findIndex((text) => text.includes(marker));
    if (index === -1) throw new Error(`no block contains marker: ${marker}`);
    return index;
  }

  /**
   * The future `replace_block_content` gateway op: replace one block's whole
   * text content in a single tagged transaction under this collaborator's own
   * client ID, exactly like another human's edit on the wire.
   */
  replaceBlockContent(index: number, replacement: string): void {
    const block = rootBlocks(this.doc)[index];
    if (!block) throw new Error(`no block at index ${index}`);
    this.doc.transact(() => {
      block.delete(0, block.length);
      block.insert(0, replacement);
    }, AGENT_ORIGIN);
  }

  destroy(): void {
    this.provider.destroy();
    this.doc.destroy();
  }
}

// slate-yjs document model: the shared root is a Y.XmlText (root name
// "content") whose delta embeds one Y.XmlText per top-level Slate element.
function rootBlocks(doc: Y.Doc): Y.XmlText[] {
  const root = doc.get('content', Y.XmlText);
  const blocks: Y.XmlText[] = [];
  for (const op of root.toDelta() as DeltaOp[]) {
    if (op.insert instanceof Y.XmlText) blocks.push(op.insert);
  }
  return blocks;
}

function blockText(block: Y.XmlText): string {
  let text = '';
  for (const op of block.toDelta() as DeltaOp[]) {
    if (typeof op.insert === 'string') text += op.insert;
    else if (op.insert instanceof Y.XmlText) text += blockText(op.insert);
  }
  return text;
}

// An in-test y-sync collab "server" for the mock-API Playwright suite.
//
// `page.routeWebSocket` intercepts the editor's `/v1/collab/{documentId}`
// connection and serves it from a Node-side Y.Doc per room, mirroring the
// real server's session semantics at the protocol level:
//
// - y-sync v1 handshake and update relay against the room doc;
// - a checkpoint-ack frame (MSG_QUARRY_CHECKPOINT carrying the committed
//   snapshot — see rust-ws-provider.ts) sent on join and re-broadcast on a
//   short debounce after every applied update, which is what drives the
//   Saved/Saving… header in these tests;
// - rooms outlive page reloads (they live in the test process), so reload
//   round-trips exercise real reseed-from-session behavior.
//
// One deliberate difference from production: rooms start EMPTY and the first
// client seeds them through the editor's init value (production seeds
// server-side from canonical rows). Review metadata can be pre-seeded per
// room via `reviewMeta` to model the server's rows-seeded review map.

import type { Page, WebSocketRoute } from 'playwright/test';
import * as decoding from 'lib0/decoding';
import * as encoding from 'lib0/encoding';
import * as syncProtocol from 'y-protocols/sync';
import * as Y from 'yjs';

const MSG_SYNC = 0;
const MSG_AWARENESS = 1;
const MSG_QUERY_AWARENESS = 3;
const MSG_QUARRY_CHECKPOINT = 113;
const CHECKPOINT_DEBOUNCE_MS = 150;

export interface MockReviewMetaEntry {
  by: string;
  at: string;
  body?: string;
  re?: string;
  status?: 'resolved';
}

export interface MockRoomReviewMeta {
  comments?: Record<string, MockReviewMetaEntry>;
  suggestions?: Record<string, MockReviewMetaEntry>;
}

interface Room {
  doc: Y.Doc;
  sockets: Set<WebSocketRoute>;
  checkpointTimer: NodeJS.Timeout | null;
}

export interface MockCollabServer {
  /** The room doc for a document id, if any client has connected. */
  roomDoc(documentId: string): Y.Doc | undefined;
  /** Plain text of the room's content root (block texts concatenated). */
  roomText(documentId: string): string;
  /** The room's shared review meta map as plain JSON. */
  roomReviewMeta(documentId: string): MockRoomReviewMeta;
}

export async function installMockCollabServer(
  page: Page,
  options: { reviewMeta?: Record<string, MockRoomReviewMeta> } = {}
): Promise<MockCollabServer> {
  const rooms = new Map<string, Room>();

  function ensureRoom(documentId: string): Room {
    const existing = rooms.get(documentId);
    if (existing) return existing;
    const doc = new Y.Doc();
    const room: Room = { doc, sockets: new Set(), checkpointTimer: null };
    const seededMeta = options.reviewMeta?.[documentId];
    if (seededMeta) seedReviewMeta(doc, seededMeta);
    doc.on('update', (update: Uint8Array, origin: unknown) => {
      // Relay to every peer except the socket the update came from (the
      // real server echoes too, but the echo is redundant for yjs).
      const frame = syncUpdateFrame(update);
      for (const socket of room.sockets) {
        if (socket !== origin) socket.send(frame);
      }
      scheduleCheckpoint(room);
    });
    rooms.set(documentId, room);
    return room;
  }

  function scheduleCheckpoint(room: Room) {
    if (room.checkpointTimer) clearTimeout(room.checkpointTimer);
    room.checkpointTimer = setTimeout(() => {
      room.checkpointTimer = null;
      const frame = checkpointFrame(room.doc);
      for (const socket of room.sockets) {
        socket.send(frame);
      }
    }, CHECKPOINT_DEBOUNCE_MS);
  }

  await page.routeWebSocket(/\/v1\/collab\/[^/]+$/, (ws) => {
    const documentId = decodeURIComponent(new URL(ws.url()).pathname.split('/').at(-1) ?? '');
    const room = ensureRoom(documentId);
    room.sockets.add(ws);
    // Join-time ack: the committed state as of now (matches session.rs).
    ws.send(checkpointFrame(room.doc));
    ws.onMessage((message) => {
      if (typeof message === 'string') return;
      const data = new Uint8Array(message);
      const decoder = decoding.createDecoder(data);
      const messageType = decoding.readVarUint(decoder);
      if (messageType === MSG_SYNC) {
        const encoder = encoding.createEncoder();
        encoding.writeVarUint(encoder, MSG_SYNC);
        syncProtocol.readSyncMessage(decoder, encoder, room.doc, ws);
        if (encoding.length(encoder) > 1) {
          ws.send(Buffer.from(encoding.toUint8Array(encoder)));
        }
        return;
      }
      if (messageType === MSG_AWARENESS || messageType === MSG_QUERY_AWARENESS) {
        // Presence is peer-relayed, never persisted; single-page tests have
        // no peers to relay to.
        return;
      }
    });
    ws.onClose(() => {
      room.sockets.delete(ws);
    });
  });

  return {
    roomDoc: (documentId) => rooms.get(documentId)?.doc,
    roomText: (documentId) => {
      const doc = rooms.get(documentId)?.doc;
      if (!doc) return '';
      return deepText(doc.get('content', Y.XmlText));
    },
    roomReviewMeta: (documentId) => {
      const doc = rooms.get(documentId)?.doc;
      if (!doc) return {};
      const review = doc.getMap<unknown>('review');
      return {
        comments: sectionToJson(review.get('comments')),
        suggestions: sectionToJson(review.get('suggestions')),
      };
    },
  };
}

function seedReviewMeta(doc: Y.Doc, meta: MockRoomReviewMeta) {
  const review = doc.getMap<unknown>('review');
  doc.transact(() => {
    const comments = new Y.Map<unknown>();
    const suggestions = new Y.Map<unknown>();
    review.set('comments', comments);
    review.set('suggestions', suggestions);
    for (const [id, entry] of Object.entries(meta.comments ?? {})) {
      comments.set(id, { ...entry });
    }
    for (const [id, entry] of Object.entries(meta.suggestions ?? {})) {
      suggestions.set(id, { ...entry });
    }
  });
}

function sectionToJson(value: unknown): Record<string, MockReviewMetaEntry> {
  if (!(value instanceof Y.Map)) return {};
  const entries: Record<string, MockReviewMetaEntry> = {};
  for (const [id, entry] of value.entries()) {
    entries[id] = entry as MockReviewMetaEntry;
  }
  return entries;
}

// Blocks are EMBEDDED XmlText children of the content root; their text only
// shows up by walking the tree (the root's own toString skips embeds).
function deepText(node: unknown): string {
  if (!(node instanceof Y.XmlText || node instanceof Y.Text)) return '';
  let out = '';
  const delta = node.toDelta() as Array<{ insert?: unknown }>;
  for (const op of delta) {
    if (typeof op.insert === 'string') out += op.insert;
    else out += deepText(op.insert);
  }
  return out;
}

function syncUpdateFrame(update: Uint8Array): Buffer {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MSG_SYNC);
  syncProtocol.writeUpdate(encoder, update);
  return Buffer.from(encoding.toUint8Array(encoder));
}

function checkpointFrame(doc: Y.Doc): Buffer {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MSG_QUARRY_CHECKPOINT);
  encoding.writeVarUint8Array(encoder, Y.encodeSnapshot(Y.snapshot(doc)));
  return Buffer.from(encoding.toUint8Array(encoder));
}

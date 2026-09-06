import { useLayoutEffect, useRef, useState } from 'react';
import { useEditorRef, useEditorVersion, useScrollRef } from 'platejs/react';
import type { DocumentModel } from './document-model';
import type { DocumentSelection } from './document-presence';
import { slatePoint } from './plate-document-projection';

type Rect = { left: number; top: number; width: number; height: number };
type Cursor = { id: string; name: string; color: string; caret: Rect; rects: Rect[] };

/** Remote presence is a projection of native cursors, never document content. */
export function PlatePresence({ model, peers }: { model: DocumentModel; peers: DocumentSelection[] }) {
  const editor = useEditorRef();
  const revision = useEditorVersion();
  const scrollRef = useScrollRef();
  const host = useRef<HTMLDivElement>(null);
  const [cursors, setCursors] = useState<Cursor[]>([]);
  useLayoutEffect(() => {
    if (!peers.length) { setCursors((previous) => previous.length ? [] : previous); return; }
    let frame = 0;
    const measure = () => {
      const origin = host.current?.getBoundingClientRect();
      if (!origin) return;
      const rect = (value: DOMRect): Rect => ({ left: value.left - origin.left, top: value.top - origin.top, width: value.width, height: value.height });
      const next: Cursor[] = [];
      for (const peer of peers) {
        try {
          const points = peer.points.map((point) => model.locate(point)).map((point, index) => point && slatePoint(editor.children, point.owner.id, point.offset, point.owner.kind === 'proposal', peer.points[index].source, point.proposed_block));
          if (!points[0] || !points[1]) continue;
          const range = editor.api.toDOMRange({ anchor: points[0], focus: points[1] });
          const caretRange = editor.api.toDOMRange({ anchor: points[1], focus: points[1] });
          if (!range || !caretRange) continue;
          const caret = caretRange.getBoundingClientRect();
          const hue = [...peer.client_id].reduce((hash, character) => (hash * 31 + character.charCodeAt(0)) % 360, 0);
          next.push({ id: peer.client_id, name: peer.author, color: `hsl(${hue}, 65%, 48%)`, caret: rect(caret), rects: Array.from(range.getClientRects()).map(rect) });
        } catch { /* The peer can be ahead of this replica; retry on the next document update. */ }
      }
      setCursors(next);
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure); };
    schedule();
    const observer = new ResizeObserver(schedule);
    if (scrollRef.current) observer.observe(scrollRef.current);
    window.addEventListener('resize', schedule);
    return () => { cancelAnimationFrame(frame); observer.disconnect(); window.removeEventListener('resize', schedule); };
  }, [editor, model, peers, revision, scrollRef]);
  // This element scrolls with the text. Both its origin and the measured
  // ranges move together, so scrolling needs no cursor layout work.
  return <div ref={host} className="pointer-events-none absolute inset-0" aria-hidden="true">
    {cursors.map((cursor) => <div key={cursor.id}>
      {cursor.rects.map((rect, index) => <div key={index} className="absolute rounded-[2px]" style={{ ...rect, background: cursor.color, opacity: 0.25 }} />)}
      <div className="quarry-remote-caret absolute w-0.5" data-peer-id={cursor.id} style={{ ...cursor.caret, width: 2, background: cursor.color, opacity: 0.8 }}>
        <div className="pointer-events-auto absolute top-0 -translate-y-full whitespace-nowrap rounded rounded-bl-none px-1.5 py-0.5 text-xs font-medium text-white shadow-sm" style={{ background: cursor.color }}>{cursor.name}</div>
      </div>
    </div>)}
  </div>;
}

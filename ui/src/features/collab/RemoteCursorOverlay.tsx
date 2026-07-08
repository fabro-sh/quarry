import { YjsPlugin } from '@platejs/yjs/react';
import {
  type CursorOverlayData,
  useRemoteCursorOverlayPositions,
} from '@slate-yjs/react';
import { usePluginOption } from 'platejs/react';
import { useRef, useState, type CSSProperties } from 'react';

type CursorData = {
  color: string;
  name: string;
};

export function RemoteCursorOverlay() {
  const isSynced = usePluginOption(YjsPlugin, '_isSynced');

  if (!isSynced) return null;

  return <RemoteCursorOverlayContent />;
}

function RemoteCursorOverlayContent() {
  // The editor container is itself the scroll element, and
  // useRemoteCursorOverlayPositions measures viewport-relative offsets without
  // adding scrollTop — so positions measured against the container are wrong
  // by the scroll amount. This wrapper is pinned to the container's scroll
  // origin and scrolls with the content, giving the hook (and the absolutely
  // positioned rects inside) a scroll-independent coordinate frame.
  // Seeded with a detached div (replaced on mount) because the library wants
  // a never-null RefObject under React 19's types.
  const containerRef = useRef<HTMLDivElement>(document.createElement('div'));
  const [cursors] = useRemoteCursorOverlayPositions<CursorData>({
    containerRef,
    refreshOnResize: 'debounced',
  });

  return (
    <div className="pointer-events-none absolute inset-0" ref={containerRef}>
      {cursors.map((cursor) => (
        <RemoteSelection key={cursor.clientId} {...cursor} />
      ))}
    </div>
  );
}

function RemoteSelection({ caretPosition, data, selectionRects }: CursorOverlayData<CursorData>) {
  if (!data) return null;

  const selectionStyle: CSSProperties = {
    backgroundColor: cursorColorWithAlpha(data.color, 0.35),
  };

  return (
    <>
      {selectionRects.map((position, index) => (
        <div
          aria-hidden="true"
          className="pointer-events-none absolute rounded-[2px]"
          key={index}
          style={{ ...selectionStyle, ...position }}
        />
      ))}
      {caretPosition ? <Caret caretPosition={caretPosition} data={data} /> : null}
    </>
  );
}

function Caret({
  caretPosition,
  data,
}: {
  caretPosition: NonNullable<CursorOverlayData<CursorData>['caretPosition']>;
  data: CursorData;
}) {
  const [hovered, setHovered] = useState(false);
  const opacity = hovered ? 1 : 0.78;
  const caretStyle: CSSProperties = {
    ...caretPosition,
    background: data.color,
    opacity,
    transition: 'opacity 120ms ease',
  };
  const labelStyle: CSSProperties = {
    background: data.color,
    opacity,
    transform: 'translateY(-100%)',
    transition: 'opacity 120ms ease',
  };

  return (
    <div aria-hidden="true" className="absolute w-0.5" style={caretStyle}>
      <div
        className="pointer-events-auto absolute top-0 whitespace-nowrap rounded rounded-bl-none px-1.5 py-0.5 text-xs font-medium text-white shadow-sm"
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={labelStyle}
      >
        {data.name}
      </div>
    </div>
  );
}

export function cursorColorWithAlpha(color: string, opacity: number): string {
  const normalizedOpacity = Math.min(Math.max(opacity, 0), 1);
  if (color.startsWith('hsl(')) {
    return color.replace('hsl(', 'hsla(').replace(')', `, ${normalizedOpacity})`);
  }
  if (color.startsWith('rgb(')) {
    return color.replace('rgb(', 'rgba(').replace(')', `, ${normalizedOpacity})`);
  }

  const alpha = Math.round(normalizedOpacity * 255)
    .toString(16)
    .padStart(2, '0')
    .toUpperCase();
  return `${color}${alpha}`;
}

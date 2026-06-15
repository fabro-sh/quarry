import {
  type TocSideBarProps,
  useTocSideBar,
  useTocSideBarState,
} from '@platejs/toc/react';

import { cn } from '../../lib/utils';

// Sticky table-of-contents rail, ported from the Plate "toc-pro" component
// (via fabro-sh/potion) and restyled to quarry tokens. It floats in the
// editor's left gutter: a column of heading tick-marks that expands into a
// clickable, scroll-spy'd heading list on hover. Mounted inside the document
// scroller so `position: sticky` tracks that scroll container.
//
// Hidden until the document has at least two headings — a TOC for a single
// heading is just noise.
export function TocSidebar({
  className,
  maxShowCount = 20,
  ...props
}: TocSideBarProps & { className?: string; maxShowCount?: number }) {
  const state = useTocSideBarState(props);
  const { activeContentId, headingList, open } = state;
  const { navProps, onContentClick } = useTocSideBar(state);

  if (headingList.length < 2) return null;

  return (
    <div className={cn('sticky top-0 left-0 z-10', className)}>
      {/* top-12 mirrors PlateContent's pt-12 so the rail top-aligns with the
          first line of document content rather than the scroller's top edge. */}
      <div className="group absolute top-12 left-0 z-10 max-h-[400px]">
        <div className="relative z-10 ml-2.5 flex flex-col justify-center pl-2 pb-3">
          {/* Collapsed tick-marks — always visible. */}
          <div className="flex flex-col gap-3 pb-3 pr-5">
            {headingList.slice(0, maxShowCount).map((item) => (
              <div
                key={item.id}
                className={cn(
                  'h-0.5 rounded-xs bg-faint/40',
                  activeContentId === item.id && 'bg-accent'
                )}
                style={{
                  marginLeft: `${4 * (item.depth - 1)}px`,
                  width: `${16 - 4 * (item.depth - 1)}px`,
                }}
              />
            ))}
          </div>

          {/* Expanded heading list — revealed on hover, slides in from the left. */}
          <nav
            aria-label="Table of contents"
            className={cn(
              'absolute -top-2.5 left-0 px-2.5 transition-all duration-300',
              'pointer-events-none -translate-x-[10px] opacity-0',
              'group-hover:pointer-events-auto group-hover:translate-x-0 group-hover:opacity-100'
            )}
            {...navProps}
          >
            <div
              id="toc_wrap"
              className="-ml-2.5 max-h-96 w-[242px] scroll-m-1 overflow-auto rounded-2xl border border-line bg-raised p-3 shadow-lg"
            >
              <div className={cn('relative z-10 p-1.5', !open && 'hidden')}>
                {headingList.map((item, index) => {
                  const isActive = activeContentId
                    ? activeContentId === item.id
                    : index === 0;

                  return (
                    <button
                      key={item.id}
                      type="button"
                      id={isActive ? 'toc_item_active' : 'toc_item'}
                      aria-current={isActive || undefined}
                      className={cn(
                        'block h-auto w-full rounded-sm p-0 text-left text-sm',
                        isActive
                          ? 'text-accent-ink'
                          : 'text-muted hover:text-body'
                      )}
                      style={{ paddingLeft: `${(item.depth - 1) * 12}px` }}
                      onClick={(e) => onContentClick(e, item, 'smooth')}
                    >
                      <div className="p-1">{item.title}</div>
                    </button>
                  );
                })}
              </div>
            </div>
          </nav>
        </div>
      </div>
    </div>
  );
}

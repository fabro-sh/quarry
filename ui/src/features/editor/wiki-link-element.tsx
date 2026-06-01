import { createContext, useContext, type ReactNode } from 'react';
import { TextApi } from 'platejs';
import { PlateElement, useReadOnly, type PlateElementProps } from 'platejs/react';

import { cn } from '../../lib/utils';
import { BaseWikiLinkPlugin, convertWikiLinkInText, wikiLinkDisplay, type WikiLinkNode } from './wiki-link';

export interface WikiResolution {
  resolved: boolean;
  targetPath: string | null;
}

export interface WikiLinkApi {
  // Resolve a wiki-link target to a document (via the backend's outgoing links).
  resolve?: (target: string) => WikiResolution | undefined;
  // Open the resolved document (optionally at an anchor).
  open?: (path: string, anchor?: string) => void;
}

const WikiLinkContext = createContext<WikiLinkApi>({});

export function WikiLinkProvider({ value, children }: { value: WikiLinkApi; children: ReactNode }) {
  return <WikiLinkContext.Provider value={value}>{children}</WikiLinkContext.Provider>;
}

const useWikiLink = () => useContext(WikiLinkContext);

// Atomic, void inline chip for `[[target]]`. Resolved links read as accent
// links; unresolved (broken) links are muted with a dashed underline. Clicking a
// resolved link opens the target document.
export function WikiLinkElement(props: PlateElementProps<WikiLinkNode>) {
  const { element } = props;
  const { resolve, open } = useWikiLink();
  useReadOnly();
  const resolution = resolve?.(element.target);
  const resolved = resolution?.resolved ?? false;
  const path = resolution?.targetPath ?? null;
  const display = wikiLinkDisplay(element);

  return (
    <PlateElement {...props} as="span" attributes={{ ...props.attributes, contentEditable: false }}>
      <span
        className={cn(
          'rounded-sm underline decoration-1 underline-offset-2 transition-colors',
          resolved
            ? 'cursor-pointer font-medium text-accent-ink hover:bg-accent-tint'
            : 'cursor-default text-muted decoration-dashed'
        )}
        data-testid="wikilink"
        data-resolved={resolved}
        onClick={() => {
          if (path) open?.(path, element.anchor);
        }}
        title={resolved ? (path ?? element.target) : `Unresolved: ${element.target}`}
      >
        {element.embed ? '!' : ''}
        {display}
      </span>
      {props.children}
    </PlateElement>
  );
}

// Turn a completed `[[...]]` into a chip as you type, via normalization.
export const WikiLinkPlugin = BaseWikiLinkPlugin.withComponent(WikiLinkElement).overrideEditor(
  ({ editor, tf: { normalizeNode } }) => ({
    transforms: {
      normalizeNode(entry) {
        const [node, path] = entry;
        if (TextApi.isText(node) && convertWikiLinkInText(editor, node, path)) return;
        normalizeNode(entry);
      },
    },
  })
);

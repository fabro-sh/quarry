import { useEffect, useState } from 'react';
import { Code, Eye } from 'lucide-react';
import { NodeApi, type TCodeBlockElement } from 'platejs';
import { PlateElement, type PlateElementProps } from 'platejs/react';

import { cn } from '../../lib/utils';

// Mermaid diagrams are plain ```mermaid fenced code blocks (so they round-trip
// for free). This renders one with a Code/Preview toggle: Preview shows the
// rendered SVG, Code shows the editable source.

function readTheme(): 'light' | 'dark' {
  const value = document.querySelector('[data-theme]')?.getAttribute('data-theme');
  return value === 'light' ? 'light' : 'dark';
}

// Follow the workspace's light/dark theme so diagrams re-render to match.
function useMermaidTheme(): 'light' | 'dark' {
  const [theme, setTheme] = useState(readTheme);
  useEffect(() => {
    const target = document.querySelector('[data-theme]');
    if (!target) return;
    const observer = new MutationObserver(() => setTheme(readTheme()));
    observer.observe(target, { attributes: true, attributeFilter: ['data-theme'] });
    return () => observer.disconnect();
  }, []);
  return theme;
}

let diagramCounter = 0;

function MermaidDiagram({ source }: { source: string }) {
  const theme = useMermaidTheme();
  const code = source.trim();
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!code) return;
    let cancelled = false;
    void (async () => {
      try {
        const mermaid = (await import('mermaid')).default;
        mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: theme === 'light' ? 'default' : 'dark' });
        const { svg } = await mermaid.render(`mermaid-${(diagramCounter += 1)}`, code);
        if (!cancelled) {
          setSvg(svg);
          setError(null);
        }
      } catch (err) {
        if (!cancelled) {
          setSvg(null);
          setError(err instanceof Error ? err.message : 'Could not render diagram');
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [code, theme]);

  if (!code) {
    return (
      <div className="px-3 py-6 text-center text-sm text-faint" contentEditable={false}>
        Empty diagram — switch to Code to add Mermaid syntax.
      </div>
    );
  }
  if (error) {
    return (
      <div className="rounded-sm bg-well px-3 py-2 text-sm text-danger" contentEditable={false} data-testid="mermaid-error">
        Diagram error: {error}
      </div>
    );
  }
  if (svg === null) {
    return (
      <div className="px-3 py-6 text-center text-sm text-muted" contentEditable={false}>
        Rendering diagram…
      </div>
    );
  }
  return (
    <div
      className="flex justify-center py-2 [&_svg]:h-auto [&_svg]:max-w-full"
      contentEditable={false}
      data-testid="mermaid-diagram"
      // mermaid sanitizes its output (securityLevel: 'strict').
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}

export function MermaidCodeBlock(props: PlateElementProps<TCodeBlockElement>) {
  const source = props.element.children.map((child) => NodeApi.string(child)).join('\n');
  const [preview, setPreview] = useState(() => source.trim().length > 0);
  return (
    <PlateElement {...props} className="group relative">
      <button
        aria-label={preview ? 'Edit Mermaid source' : 'Preview Mermaid diagram'}
        className="absolute right-1.5 top-1.5 z-10 inline-flex items-center gap-1 rounded border border-line bg-raised px-1.5 py-1 text-xs text-muted opacity-0 transition-opacity hover:text-body group-hover:opacity-100 focus-visible:opacity-100"
        contentEditable={false}
        data-testid="mermaid-toggle"
        onClick={() => setPreview((value) => !value)}
        onMouseDown={(event) => event.preventDefault()}
        type="button"
      >
        {preview ? <Code size={13} /> : <Eye size={13} />}
        {preview ? 'Code' : 'Preview'}
      </button>
      {/* Keep the source in the DOM for Slate; hide it in preview. */}
      <pre className={cn(preview && 'hidden')}>
        <code>{props.children}</code>
      </pre>
      {preview ? <MermaidDiagram source={source} /> : null}
    </PlateElement>
  );
}

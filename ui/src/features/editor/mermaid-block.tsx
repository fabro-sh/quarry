import { useEffect, useState } from 'react';
import { Code, Eye } from 'lucide-react';
import { PlateElement, useEditorRef, type PlateElementProps } from 'platejs/react';

import { cn } from '../../lib/utils';
import { BaseMermaidPlugin, type TMermaidElement } from './mermaid';

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
      <div className="px-3 py-6 text-center text-sm text-faint">
        Empty diagram — switch to Code to add Mermaid syntax.
      </div>
    );
  }
  if (error) {
    return (
      <div className="rounded-sm bg-well px-3 py-2 text-sm text-danger" data-testid="mermaid-error">
        Diagram error: {error}
      </div>
    );
  }
  if (svg === null) {
    return <div className="px-3 py-6 text-center text-sm text-muted">Rendering diagram…</div>;
  }
  return (
    <div
      className="flex justify-center py-2 [&_svg]:h-auto [&_svg]:max-w-full"
      data-testid="mermaid-diagram"
      // mermaid sanitizes its output (securityLevel: 'strict').
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}

// An atomic (void) Mermaid block. Preview renders the diagram; Code shows a
// textarea bound to the node's `code`. Being void, the block is a single unit to
// Slate, so neighbouring edits and cursor moves can't reach the source.
export function MermaidBlock(props: PlateElementProps<TMermaidElement>) {
  const editor = useEditorRef();
  const code = props.element.code ?? '';
  const [editing, setEditing] = useState(() => code.trim().length === 0);
  return (
    <PlateElement {...props} className="group relative my-1">
      {/* The void element's content is non-editable; only the textarea (a form
          control) takes input. Plate renders props.children as the void spacer. */}
      <div contentEditable={false}>
        <button
          aria-label={editing ? 'Preview Mermaid diagram' : 'Edit Mermaid source'}
          className="absolute right-1.5 top-1.5 z-10 inline-flex items-center gap-1 rounded border border-line bg-raised px-1.5 py-1 text-xs text-muted opacity-0 transition-opacity hover:text-body group-hover:opacity-100 focus-visible:opacity-100"
          data-testid="mermaid-toggle"
          onClick={() => setEditing((value) => !value)}
          type="button"
        >
          {editing ? <Eye size={13} /> : <Code size={13} />}
          {editing ? 'Preview' : 'Code'}
        </button>
        {editing ? (
          <textarea
            aria-label="Mermaid source"
            className={cn(
              'block w-full resize-y rounded-sm border border-line bg-canvas p-3 pr-16 font-mono text-sm text-ink outline-none',
              'focus:border-accent'
            )}
            data-testid="mermaid-source"
            defaultValue={code}
            onChange={(event) => {
              const path = editor.api.findPath(props.element);
              if (path) editor.tf.setNodes({ code: event.target.value }, { at: path });
            }}
            rows={Math.max(3, code.split('\n').length + 1)}
            spellCheck={false}
          />
        ) : (
          <MermaidDiagram source={code} />
        )}
      </div>
      {props.children}
    </PlateElement>
  );
}

export const MermaidPlugin = BaseMermaidPlugin.withComponent(MermaidBlock);

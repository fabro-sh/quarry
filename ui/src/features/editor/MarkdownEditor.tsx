import { FileText, Save } from 'lucide-react';
import {
  lazy,
  Suspense,
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from 'react';

import { cn } from '../../lib/utils';

interface MarkdownEditorProps {
  content: string;
  disabled?: boolean;
  links?: MarkdownPreviewLink[];
  loadMermaid?: MermaidLoader;
  mode: 'source' | 'rich';
  resolveDocumentHref?: (path: string) => string;
  richPreviewEnabled?: boolean;
  status: string;
  wikiSuggestions?: WikiLinkSuggestion[];
  onChange: (content: string) => void;
  onModeChange: (mode: 'source' | 'rich') => void;
  onOpenDocument?: (path: string) => void;
  onSave: () => void;
  onWikiSuggestQueryChange?: (query: string) => void;
}

const PlateMarkdownEditor = lazy(() =>
  import('./PlateMarkdownEditor').then((module) => ({ default: module.PlateMarkdownEditor }))
);

export function MarkdownEditor({
  content,
  disabled,
  links = [],
  loadMermaid = defaultMermaidLoader,
  mode,
  resolveDocumentHref,
  richPreviewEnabled = true,
  status,
  wikiSuggestions = [],
  onChange,
  onModeChange,
  onOpenDocument,
  onSave,
  onWikiSuggestQueryChange,
}: MarkdownEditorProps) {
  return (
    <section className="flex min-h-0 flex-1 flex-col bg-[#fbfaf7]" aria-label="Editor">
      <div className="flex h-11 shrink-0 items-center justify-between border-b border-[#d9d6cc] px-3">
        <div className="flex items-center gap-1 rounded-md border border-[#d9d6cc] bg-white p-0.5">
          <button
            aria-pressed={mode === 'source'}
            className={modeButton(mode === 'source')}
            onClick={() => onModeChange('source')}
            type="button"
          >
            Source
          </button>
          <button
            aria-pressed={mode === 'rich'}
            className={modeButton(mode === 'rich')}
            onClick={() => onModeChange('rich')}
            type="button"
          >
            Rich
          </button>
        </div>
        <button
          aria-label="Save document"
          className="inline-flex h-8 items-center gap-2 rounded-md bg-[#256f64] px-3 text-sm font-medium text-white hover:bg-[#1d5b52] disabled:cursor-not-allowed disabled:bg-[#9eb5af]"
          disabled={disabled}
          onClick={onSave}
          type="button"
        >
          <Save size={15} />
          Save
        </button>
      </div>

      {mode === 'source' ? (
        <MarkdownSourceArea
          content={content}
          textareaClassName="h-full w-full resize-none border-0 bg-transparent px-8 py-7 font-mono text-[14px] leading-6 text-[#1e211f] outline-none"
          wrapperClassName="min-h-0 flex-1"
          wikiSuggestions={wikiSuggestions}
          onChange={onChange}
          onWikiSuggestQueryChange={onWikiSuggestQueryChange}
        />
      ) : (
        <div className="min-h-0 flex-1 overflow-auto px-8 py-7">
          <div className="mb-3 inline-flex items-center gap-2 rounded-md border border-[#d9d6cc] bg-white px-2 py-1 text-xs text-[#62645e]">
            <FileText size={14} />
            Plate markdown mode keeps source as the save format.
          </div>
          {richPreviewEnabled ? (
          <MarkdownPreview
            content={content}
            links={links}
            loadMermaid={loadMermaid}
            onOpenDocument={onOpenDocument}
            resolveDocumentHref={resolveDocumentHref}
          />
          ) : null}
          <Suspense fallback={<div className="rounded-md border border-[#d9d6cc] bg-white p-4 text-sm">Loading editor</div>}>
            <PlateMarkdownEditor content={content} disabled={disabled} onChange={onChange} />
          </Suspense>
        </div>
      )}

      <div className="flex h-8 shrink-0 items-center gap-2 border-t border-[#d9d6cc] px-3 py-1 text-xs text-[#62645e]">
        <span>{status.split(' · ')[0]}</span>
        {status.includes(' · ') ? <span>{status.split(' · ').slice(1).join(' · ')}</span> : null}
      </div>
    </section>
  );
}

export interface WikiLinkSuggestion {
  path: string;
  title: string;
  match_type: string;
  head_version_id: string;
  matched_text?: string | null;
  target_anchor?: string | null;
}

interface WikiCompletion {
  start: number;
  end: number;
  query: string;
}

function MarkdownSourceArea({
  content,
  textareaClassName,
  wrapperClassName,
  wikiSuggestions,
  onChange,
  onWikiSuggestQueryChange,
}: {
  content: string;
  textareaClassName: string;
  wrapperClassName?: string;
  wikiSuggestions: WikiLinkSuggestion[];
  onChange: (content: string) => void;
  onWikiSuggestQueryChange?: (query: string) => void;
}) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const listId = useId();
  const [completion, setCompletion] = useState<WikiCompletion | null>(null);
  const [activeSuggestionIndex, setActiveSuggestionIndex] = useState(0);
  const visibleSuggestions = completion ? wikiSuggestions.slice(0, 8) : [];
  const activeSuggestion =
    visibleSuggestions[Math.min(activeSuggestionIndex, Math.max(visibleSuggestions.length - 1, 0))];

  useEffect(() => {
    setActiveSuggestionIndex(0);
  }, [completion?.query, wikiSuggestions]);

  function updateCompletion(textarea: HTMLTextAreaElement) {
    const next = findWikiCompletion(textarea.value, textarea.selectionStart ?? textarea.value.length);
    setCompletion(next);
    onWikiSuggestQueryChange?.(next?.query ?? '');
  }

  function applySuggestion(suggestion: WikiLinkSuggestion) {
    if (!completion) return;
    const current = textareaRef.current?.value ?? content;
    const insertText = wikiSuggestionInsertText(suggestion);
    const nextContent = `${current.slice(0, completion.start)}${insertText}]]${current.slice(completion.end)}`;
    const nextCursor = completion.start + insertText.length + 2;
    onChange(nextContent);
    setCompletion(null);
    onWikiSuggestQueryChange?.('');
    window.requestAnimationFrame(() => {
      textareaRef.current?.focus();
      textareaRef.current?.setSelectionRange(nextCursor, nextCursor);
    });
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLTextAreaElement>) {
    if (!completion || !visibleSuggestions.length) return;
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      setActiveSuggestionIndex((index) => Math.min(index + 1, visibleSuggestions.length - 1));
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      setActiveSuggestionIndex((index) => Math.max(index - 1, 0));
    } else if ((event.key === 'Enter' || event.key === 'Tab') && activeSuggestion) {
      event.preventDefault();
      applySuggestion(activeSuggestion);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      setCompletion(null);
      onWikiSuggestQueryChange?.('');
    }
  }

  return (
    <div className={cn('relative', wrapperClassName)}>
      <textarea
        aria-activedescendant={completion && activeSuggestion ? wikiSuggestionOptionId(listId, activeSuggestionIndex) : undefined}
        aria-autocomplete="list"
        aria-controls={visibleSuggestions.length ? listId : undefined}
        aria-label="Markdown source"
        className={textareaClassName}
        ref={textareaRef}
        spellCheck={false}
        value={content}
        onChange={(event) => {
          onChange(event.currentTarget.value);
          updateCompletion(event.currentTarget);
        }}
        onClick={(event) => updateCompletion(event.currentTarget)}
        onKeyDown={handleKeyDown}
        onSelect={(event) => updateCompletion(event.currentTarget)}
      />
      {visibleSuggestions.length ? (
        <div
          aria-label="Wiki-link suggestions"
          className="absolute left-4 right-4 top-12 z-20 max-h-56 overflow-auto rounded-md border border-[#cfcabc] bg-white p-1 shadow-lg"
          id={listId}
          role="listbox"
        >
          {visibleSuggestions.map((suggestion, index) => (
            <button
              aria-selected={index === activeSuggestionIndex}
              className={cn(
                'flex w-full items-center justify-between gap-3 rounded px-2 py-1.5 text-left text-sm hover:bg-[#ece9df]',
                index === activeSuggestionIndex && 'bg-[#e3eee9] text-[#123f38]'
              )}
              id={wikiSuggestionOptionId(listId, index)}
              key={`${suggestion.match_type}:${suggestion.path}:${suggestion.matched_text ?? ''}`}
              role="option"
              type="button"
              onClick={() => applySuggestion(suggestion)}
              onMouseDown={(event) => event.preventDefault()}
              onMouseEnter={() => setActiveSuggestionIndex(index)}
            >
              <span className="min-w-0">
                <span className="block truncate font-medium">{wikiSuggestionLabel(suggestion)}</span>
                <span className="block truncate text-xs text-[#62645e]">{wikiSuggestionDetail(suggestion)}</span>
              </span>
              <span className="shrink-0 rounded bg-[#f1f0ea] px-1.5 py-0.5 text-[10px] uppercase text-[#62645e]">
                {suggestion.match_type}
              </span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function findWikiCompletion(content: string, cursor: number): WikiCompletion | null {
  const beforeCursor = content.slice(0, cursor);
  const openIndex = beforeCursor.lastIndexOf('[[');
  if (openIndex < 0) return null;
  const query = beforeCursor.slice(openIndex + 2);
  if (!query || query.includes(']]') || query.includes('\n') || query.includes('[') || query.includes(']')) {
    return null;
  }
  return { start: openIndex + 2, end: cursor, query };
}

function wikiSuggestionInsertText(suggestion: WikiLinkSuggestion) {
  const pathTarget = stripMarkdownExtension(suggestion.path);
  if (suggestion.match_type === 'heading' && (suggestion.target_anchor || suggestion.matched_text)) {
    return `${pathTarget}#${suggestion.target_anchor ?? suggestion.matched_text}`;
  }
  if (suggestion.match_type === 'alias' && suggestion.matched_text) {
    return `${pathTarget}|${suggestion.matched_text}`;
  }
  return pathTarget;
}

function wikiSuggestionLabel(suggestion: WikiLinkSuggestion) {
  return suggestion.matched_text || suggestion.title || suggestion.path;
}

function wikiSuggestionDetail(suggestion: WikiLinkSuggestion) {
  if (suggestion.match_type === 'heading') return `${suggestion.path} heading`;
  if (suggestion.match_type === 'alias') return `${suggestion.path} alias`;
  return suggestion.path;
}

function wikiSuggestionOptionId(listId: string, index: number) {
  return `${listId}-wiki-suggestion-${index}`;
}

function stripMarkdownExtension(path: string) {
  return path.replace(/\.(md|markdown)$/i, '');
}

interface MermaidApi {
  initialize: (options: { securityLevel: 'strict'; startOnLoad: false }) => void;
  render: (id: string, text: string) => Promise<{ svg: string }>;
}

type MermaidLoader = () => Promise<MermaidApi>;

export interface MarkdownPreviewLink {
  target_kind: string;
  target_text: string;
  target_path: string | null;
  target_anchor?: string | null;
  alias?: string | null;
  resolved: boolean;
  resolution_status?: 'resolved' | 'unresolved' | 'ambiguous';
}

function MarkdownPreview({
  content,
  links,
  loadMermaid,
  onOpenDocument,
  resolveDocumentHref,
}: {
  content: string;
  links: MarkdownPreviewLink[];
  loadMermaid: MermaidLoader;
  onOpenDocument?: (path: string) => void;
  resolveDocumentHref?: (path: string) => string;
}) {
  const blocks = previewBlocks(content);
  if (!blocks.length) return null;
  return (
    <div
      aria-label="Rich markdown preview"
      className="mb-4 space-y-2 rounded-md border border-[#d9d6cc] bg-white p-4"
    >
      {blocks.map((block, index) => {
        if (block.kind === 'mermaid') {
          return (
            <MermaidDiagram
              content={block.content}
              key={`${block.kind}:${index}`}
              loadMermaid={loadMermaid}
            />
          );
        }
        if (block.kind === 'frontmatter') {
          return (
            <dl
              aria-label="Frontmatter"
              className="grid gap-x-3 gap-y-1 rounded-md border border-[#d9d6cc] bg-[#fbfaf7] px-3 py-2 text-xs sm:grid-cols-[96px_minmax(0,1fr)]"
              key={`${block.kind}:${index}`}
            >
              {block.entries.map((entry) => (
                <div className="contents" key={entry.key}>
                  <dt className="font-semibold uppercase text-[#62645e]">{frontmatterLabel(entry.key)}</dt>
                  <dd className="min-w-0 truncate font-mono text-[#343832]" title={entry.value}>
                    {entry.value}
                  </dd>
                </div>
              ))}
            </dl>
          );
        }
        if (block.kind === 'code') {
          return (
            <figure
              className="rounded-md border border-[#d9d6cc] bg-[#fbfaf7]"
              key={`${block.kind}:${index}`}
            >
              {block.language ? (
                <figcaption className="border-b border-[#d9d6cc] px-3 py-1 text-xs font-semibold uppercase text-[#62645e]">
                  {block.language}
                </figcaption>
              ) : null}
              <pre className="overflow-auto p-3 font-mono text-xs leading-5 text-[#343832]">
                <code>{block.content}</code>
              </pre>
            </figure>
          );
        }
        if (block.kind === 'heading') {
          return renderPreviewHeading(
            block,
            `${block.kind}:${index}`,
            links,
            onOpenDocument,
            resolveDocumentHref
          );
        }
        if (block.kind === 'blockquote') {
          return (
            <blockquote
              className="border-l-2 border-[#b8d6cd] bg-[#fbfaf7] py-1 pl-3 text-sm text-[#343832]"
              key={`${block.kind}:${index}`}
            >
              {block.lines.map((line, lineIndex) => (
                <p key={`${lineIndex}:${line}`}>
                  {renderInlinePreview(line, links, onOpenDocument, resolveDocumentHref)}
                </p>
              ))}
            </blockquote>
          );
        }
        if (block.kind === 'list') {
          const ListTag = block.ordered ? 'ol' : 'ul';
          return (
            <ListTag
              className={cn(
                'space-y-1 pl-5 text-sm text-[#343832]',
                block.ordered ? 'list-decimal' : 'list-disc'
              )}
              key={`${block.kind}:${index}`}
            >
              {block.items.map((item, itemIndex) => (
                <li key={`${itemIndex}:${item}`}>
                  {renderInlinePreview(item, links, onOpenDocument, resolveDocumentHref)}
                </li>
              ))}
            </ListTag>
          );
        }
        if (block.kind === 'thematicBreak') {
          return <hr className="border-[#d9d6cc]" key={`${block.kind}:${index}`} />;
        }
        return (
          <p className="text-sm text-[#343832]" key={`${block.kind}:${index}`}>
            {renderInlinePreview(block.content, links, onOpenDocument, resolveDocumentHref)}
          </p>
        );
      })}
    </div>
  );
}

function MermaidDiagram({
  content,
  loadMermaid,
}: {
  content: string;
  loadMermaid: MermaidLoader;
}) {
  const reactId = useId();
  const [renderedSvg, setRenderedSvg] = useState('');
  const [renderError, setRenderError] = useState('');

  useEffect(() => {
    let cancelled = false;
    setRenderedSvg('');
    setRenderError('');

    void loadMermaid()
      .then(async (mermaid) => {
        mermaid.initialize({ securityLevel: 'strict', startOnLoad: false });
        const result = await mermaid.render(`quarry-mermaid-${safeDomId(reactId)}`, content);
        if (!cancelled) setRenderedSvg(sanitizeMermaidSvg(result.svg));
      })
      .catch((error: unknown) => {
        if (!cancelled) setRenderError(error instanceof Error ? error.message : 'Unable to render diagram');
      });

    return () => {
      cancelled = true;
    };
  }, [content, loadMermaid, reactId]);

  return (
    <figure
      aria-label="Mermaid diagram"
      className="rounded-md border border-[#d9d6cc] bg-[#fbfaf7] p-3"
      role="img"
    >
      <figcaption className="mb-2 text-xs font-semibold uppercase text-[#62645e]">Mermaid</figcaption>
      {renderedSvg ? (
        <div className="overflow-auto text-[#343832]" dangerouslySetInnerHTML={{ __html: renderedSvg }} />
      ) : (
        <pre className="overflow-auto font-mono text-xs text-[#343832]">
          {renderError ? `Mermaid render failed: ${renderError}` : content}
        </pre>
      )}
    </figure>
  );
}

async function defaultMermaidLoader(): Promise<MermaidApi> {
  const { default: mermaid } = await import('mermaid');
  return mermaid;
}

function safeDomId(value: string) {
  return value.replace(/[^A-Za-z0-9_-]/g, '');
}

function sanitizeMermaidSvg(svg: string) {
  if (typeof DOMParser === 'undefined' || typeof XMLSerializer === 'undefined') {
    return stripUnsafeSvgContent(svg);
  }
  const document = new DOMParser().parseFromString(svg, 'image/svg+xml');
  if (document.querySelector('parsererror')) return stripUnsafeSvgContent(svg);
  document.querySelectorAll('script').forEach((element) => element.remove());
  document.querySelectorAll('*').forEach((element) => {
    for (const attribute of Array.from(element.attributes)) {
      if (attribute.name.toLowerCase().startsWith('on')) {
        element.removeAttribute(attribute.name);
      }
    }
  });
  return new XMLSerializer().serializeToString(document.documentElement);
}

function stripUnsafeSvgContent(svg: string) {
  return svg
    .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, '')
    .replace(/\son[a-z]+\s*=\s*(['"]).*?\1/gi, '');
}

type PreviewBlock =
  | { kind: 'blockquote'; lines: string[] }
  | { kind: 'code'; content: string; language: string }
  | { kind: 'heading'; content: string; level: 1 | 2 | 3 | 4 | 5 | 6 }
  | { kind: 'list'; items: string[]; ordered: boolean }
  | { kind: 'thematicBreak' }
  | { kind: 'paragraph'; content: string }
  | { kind: 'mermaid'; content: string }
  | { kind: 'frontmatter'; entries: FrontmatterEntry[] };

interface FrontmatterEntry {
  key: string;
  value: string;
}

function previewBlocks(content: string): PreviewBlock[] {
  const blocks: PreviewBlock[] = [];
  const lines = content.split('\n');
  const frontmatter = parseFrontmatter(lines);
  let startIndex = 0;
  if (frontmatter) {
    if (frontmatter.entries.length) {
      blocks.push({ kind: 'frontmatter', entries: frontmatter.entries });
    }
    startIndex = frontmatter.endLine + 1;
  }

  for (let index = startIndex; index < lines.length; index += 1) {
    const line = lines[index];
    const trimmedLine = line.trim();
    const fenceMatch = trimmedLine.match(/^```([A-Za-z0-9_-]*)\s*$/);
    if (fenceMatch) {
      const fencedContent: string[] = [];
      const language = fenceMatch[1] ?? '';
      index += 1;
      while (index < lines.length && lines[index].trim() !== '```') {
        fencedContent.push(lines[index]);
        index += 1;
      }
      if (language.toLowerCase() === 'mermaid') {
        blocks.push({ kind: 'mermaid', content: fencedContent.join('\n') });
      } else {
        blocks.push({ kind: 'code', content: fencedContent.join('\n'), language });
      }
      continue;
    }

    const heading = line.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      blocks.push({
        kind: 'heading',
        level: heading[1].length as 1 | 2 | 3 | 4 | 5 | 6,
        content: heading[2].replace(/\s+#+\s*$/, '').trim(),
      });
      continue;
    }

    if (/^([-*_])(?:\s*\1){2,}$/.test(trimmedLine)) {
      blocks.push({ kind: 'thematicBreak' });
      continue;
    }

    const quoteLine = line.match(/^>\s?(.*)$/);
    if (quoteLine) {
      const quoteLines = [quoteLine[1]];
      while (index + 1 < lines.length) {
        const nextLine = lines[index + 1].match(/^>\s?(.*)$/);
        if (!nextLine) break;
        quoteLines.push(nextLine[1]);
        index += 1;
      }
      blocks.push({ kind: 'blockquote', lines: quoteLines });
      continue;
    }

    const listItem = parsePreviewListItem(line);
    if (listItem) {
      const items = [listItem.content];
      while (index + 1 < lines.length) {
        const nextItem = parsePreviewListItem(lines[index + 1]);
        if (!nextItem || nextItem.ordered !== listItem.ordered) break;
        items.push(nextItem.content);
        index += 1;
      }
      blocks.push({ kind: 'list', ordered: listItem.ordered, items });
      continue;
    }

    if (trimmedLine) {
      blocks.push({ kind: 'paragraph', content: line });
    }
  }
  return blocks;
}

function parseFrontmatter(lines: string[]) {
  if (lines[0]?.trim() !== '---') return null;
  const endLine = lines.findIndex((line, index) => index > 0 && line.trim() === '---');
  if (endLine < 0) return null;
  return { endLine, entries: parseFrontmatterEntries(lines.slice(1, endLine)) };
}

function parseFrontmatterEntries(lines: string[]): FrontmatterEntry[] {
  const entries: FrontmatterEntry[] = [];
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (!line.trim()) continue;
    const field = line.match(/^([A-Za-z0-9_-]+):\s*(.*)$/);
    if (!field) continue;
    const key = field[1];
    const rawValue = field[2].trim();
    if (rawValue) {
      entries.push({ key, value: cleanFrontmatterValue(rawValue) });
      continue;
    }

    const values: string[] = [];
    while (index + 1 < lines.length) {
      const item = lines[index + 1].match(/^\s*-\s+(.+)$/);
      if (!item) break;
      values.push(cleanFrontmatterValue(item[1].trim()));
      index += 1;
    }
    if (values.length) entries.push({ key, value: values.join(', ') });
  }
  return entries;
}

function cleanFrontmatterValue(value: string) {
  return value.replace(/^["']|["']$/g, '');
}

function frontmatterLabel(key: string) {
  return key
    .replace(/[_-]+/g, ' ')
    .replace(/\w\S*/g, (word) => word.charAt(0).toUpperCase() + word.slice(1));
}

function parsePreviewListItem(line: string) {
  const unordered = line.match(/^\s*[-*+]\s+(.+)$/);
  if (unordered) return { ordered: false, content: unordered[1] };
  const ordered = line.match(/^\s*\d+\.\s+(.+)$/);
  if (ordered) return { ordered: true, content: ordered[1] };
  return null;
}

function renderPreviewHeading(
  block: Extract<PreviewBlock, { kind: 'heading' }>,
  key: string,
  links: MarkdownPreviewLink[],
  onOpenDocument?: (path: string) => void,
  resolveDocumentHref?: (path: string) => string
) {
  const content = renderInlinePreview(block.content, links, onOpenDocument, resolveDocumentHref);
  const className = previewHeadingClass(block.level);
  if (block.level === 1) return <h1 className={className} key={key}>{content}</h1>;
  if (block.level === 2) return <h2 className={className} key={key}>{content}</h2>;
  if (block.level === 3) return <h3 className={className} key={key}>{content}</h3>;
  if (block.level === 4) return <h4 className={className} key={key}>{content}</h4>;
  if (block.level === 5) return <h5 className={className} key={key}>{content}</h5>;
  return <h6 className={className} key={key}>{content}</h6>;
}

function previewHeadingClass(level: 1 | 2 | 3 | 4 | 5 | 6) {
  if (level === 1) return 'text-xl font-semibold text-[#1e211f]';
  if (level === 2) return 'text-lg font-semibold text-[#1e211f]';
  if (level === 3) return 'text-base font-semibold text-[#1e211f]';
  return 'text-sm font-semibold text-[#343832]';
}

function renderInlinePreview(
  content: string,
  links: MarkdownPreviewLink[],
  onOpenDocument?: (path: string) => void,
  resolveDocumentHref?: (path: string) => string
) {
  const parts: ReactNode[] = [];
  const matcher = /(!?)\[\[([^\]]+)\]\]|(!?)\[([^\]\n]+)\]\(([^)\n]+)\)/g;
  let cursor = 0;
  let match: RegExpExecArray | null;
  while ((match = matcher.exec(content))) {
    if (match.index > cursor) {
      parts.push(...renderInlineTextParts(content.slice(cursor, match.index), `text:${cursor}`));
    }

    if (match[2] !== undefined) {
      const isEmbed = match[1] === '!';
      const { label, target, anchor } = wikiLinkLabel(match[2]);
      parts.push(renderLinkedPreviewPart({
        anchor,
        isEmbed,
        key: `${match.index}:${match[0]}`,
        kind: isEmbed ? 'embed' : 'wiki_link',
        label,
        links,
        onOpenDocument,
        resolveDocumentHref,
        target,
      }));
    } else {
      const isImage = match[3] === '!';
      const label = match[4];
      const [target, anchor] = splitWikiAnchor(match[5].trim());
      parts.push(renderLinkedPreviewPart({
        anchor,
        isEmbed: isImage,
        key: `${match.index}:${match[0]}`,
        kind: 'markdown_link',
        label,
        links,
        onOpenDocument,
        resolveDocumentHref,
        target,
      }));
    }
    cursor = match.index + match[0].length;
  }
  if (cursor < content.length) {
    parts.push(...renderInlineTextParts(content.slice(cursor), `text:${cursor}`));
  }
  return parts.length ? parts : content;
}

function renderInlineTextParts(content: string, keyPrefix: string) {
  const parts: ReactNode[] = [];
  const matcher = /`([^`\n]+)`|\*\*([^*\n]+)\*\*|\*([^*\n]+)\*|(^|[\s([{"'])#([A-Za-z][A-Za-z0-9_/-]*)/g;
  let cursor = 0;
  let match: RegExpExecArray | null;
  while ((match = matcher.exec(content))) {
    if (match.index > cursor) parts.push(content.slice(cursor, match.index));

    const key = `${keyPrefix}:${match.index}:${match[0]}`;
    if (match[1] !== undefined) {
      parts.push(
        <code className="rounded bg-[#f1f0ea] px-1 py-0.5 font-mono text-[0.92em] text-[#1e211f]" key={key}>
          {match[1]}
        </code>
      );
    } else if (match[2] !== undefined) {
      parts.push(
        <strong className="font-semibold text-[#1e211f]" key={key}>
          {match[2]}
        </strong>
      );
    } else if (match[3] !== undefined) {
      parts.push(
        <em className="italic" key={key}>
          {match[3]}
        </em>
      );
    } else {
      const boundary = match[4] ?? '';
      const tag = match[5];
      if (boundary) parts.push(boundary);
      parts.push(
        <span
          aria-label={`Tag ${tag}`}
          className="inline-flex items-center rounded border border-[#b8d6cd] bg-[#e3eee9] px-1.5 py-0.5 text-[0.92em] font-medium text-[#143f39]"
          key={key}
        >
          #{tag}
        </span>
      );
    }
    cursor = match.index + match[0].length;
  }

  if (cursor < content.length) parts.push(content.slice(cursor));
  return parts.length ? parts : [content];
}

function renderLinkedPreviewPart({
  anchor,
  isEmbed,
  key,
  kind,
  label,
  links,
  onOpenDocument,
  resolveDocumentHref,
  target,
}: {
  anchor: string | null;
  isEmbed: boolean;
  key: string;
  kind: 'wiki_link' | 'embed' | 'markdown_link';
  label: string;
  links: MarkdownPreviewLink[];
  onOpenDocument?: (path: string) => void;
  resolveDocumentHref?: (path: string) => string;
  target: string;
}) {
  const previewLink = resolvePreviewLink(kind, target, anchor, links);
  const baseClass = cn(
    'mx-0.5 inline-flex items-center gap-1 rounded border px-1.5 py-0.5 font-medium',
    isEmbed ? 'border-[#d9d6cc] bg-[#f1f0ea]' : 'border-[#b8d6cd] bg-[#e3eee9]',
    !previewLink?.resolved && !isExternalTarget(target) && 'border-[#e3b57e] bg-[#fff4e5]'
  );
  const title = previewLink?.target_path ?? target;

  if (kind === 'markdown_link' && isEmbed) {
    const imageSrc = previewLink?.resolved && previewLink.target_path && resolveDocumentHref
      ? resolveDocumentHref(previewLink.target_path)
      : isExternalTarget(target)
        ? target
        : null;
    if (imageSrc) {
      return (
        <img
          alt={label}
          className="my-2 max-h-80 max-w-full rounded border border-[#d9d6cc] bg-[#fbfaf7] object-contain"
          key={key}
          src={imageSrc}
        />
      );
    }
  }

  if (previewLink?.resolved && previewLink.target_path && onOpenDocument) {
    return (
      <button
        className={cn(baseClass, 'text-[#143f39] hover:border-[#7aa69e] hover:bg-[#d9ebe5]')}
        key={key}
        onClick={() => onOpenDocument(previewLink.target_path!)}
        title={title}
        type="button"
      >
        {label}
      </button>
    );
  }

  if (kind === 'markdown_link' && isExternalTarget(target)) {
    return (
      <a
        className={cn(baseClass, 'text-[#143f39] underline-offset-2 hover:underline')}
        href={target}
        key={key}
        rel="noreferrer"
        target="_blank"
      >
        {label}
      </a>
    );
  }

  return (
    <span className={baseClass} key={key} title={title}>
      {label}
      {previewLinkStatus(previewLink, target) && !isEmbed ? (
        <span className="rounded bg-[#f5e5d4] px-1 text-[10px] uppercase text-[#8a4a22]">
          {previewLinkStatus(previewLink, target)}
        </span>
      ) : null}
    </span>
  );
}

function previewLinkStatus(previewLink: MarkdownPreviewLink | undefined, target: string) {
  if (isExternalTarget(target)) return null;
  if (previewLink?.resolution_status === 'ambiguous') return 'Ambiguous';
  return previewLink?.resolved ? null : 'Unresolved';
}

function wikiLinkLabel(rawTarget: string) {
  const [target, alias] = rawTarget.split('|', 2);
  const [path, anchor] = splitWikiAnchor(target.trim());
  return { target: path, anchor, label: alias?.trim() || target.trim() };
}

function splitWikiAnchor(target: string) {
  const hash = target.indexOf('#');
  const block = target.indexOf('^');
  const indexes = [hash, block].filter((index) => index >= 0);
  if (!indexes.length) return [target, null] as const;
  const index = Math.min(...indexes);
  return [target.slice(0, index), target.slice(index + 1)] as const;
}

function resolvePreviewLink(
  kind: 'wiki_link' | 'embed' | 'markdown_link',
  target: string,
  anchor: string | null,
  links: MarkdownPreviewLink[]
) {
  const normalizedTarget = normalizePreviewLinkTarget(target);
  const normalizedAnchor = anchor ? normalizePreviewLinkAnchor(anchor) : null;
  return links.find((link) => {
    if (link.target_kind !== kind) return false;
    const linkAnchor = link.target_anchor ? normalizePreviewLinkAnchor(link.target_anchor) : null;
    if (normalizedAnchor !== linkAnchor) return false;
    return normalizePreviewLinkTarget(link.target_text) === normalizedTarget;
  });
}

function normalizePreviewLinkTarget(value: string) {
  return value.trim().replace(/\.(md|markdown)$/i, '').toLowerCase();
}

function normalizePreviewLinkAnchor(value: string) {
  return value.trim().replace(/^[#^]/, '').toLowerCase();
}

function isExternalTarget(target: string) {
  return /^(https?:|mailto:|tel:)/i.test(target);
}

function modeButton(active: boolean) {
  return cn(
    'rounded px-2 py-1 text-xs font-medium',
    active ? 'bg-[#e3eee9] text-[#143f39]' : 'text-[#62645e] hover:bg-[#f1f0ea]'
  );
}

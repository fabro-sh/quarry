// Shared by the fast editor tests and the live Rust/browser suite. Coverage is
// deliberately checked against the production registry, so adding a block type
// requires defining its deletion shape here before either suite can pass.
export interface BlockDeleteConformanceCase {
  readonly blockType: string;
  readonly marker?: string;
  readonly name: string;
  readonly preserveMarkers?: readonly string[];
  readonly source: string;
}

export const BLOCK_DELETE_CASES: readonly BlockDeleteConformanceCase[] = [
  { name: 'p', blockType: 'p', marker: 'delete:p-plain', source: 'delete:p-plain' },
  { name: 'h1', blockType: 'h1', marker: 'delete:h1', source: '# delete:h1' },
  { name: 'h2', blockType: 'h2', marker: 'delete:h2', source: '## delete:h2' },
  { name: 'h3', blockType: 'h3', marker: 'delete:h3', source: '### delete:h3' },
  { name: 'h4', blockType: 'h4', marker: 'delete:h4', source: '#### delete:h4' },
  { name: 'h5', blockType: 'h5', marker: 'delete:h5', source: '##### delete:h5' },
  { name: 'h6', blockType: 'h6', marker: 'delete:h6', source: '###### delete:h6' },
  {
    name: 'blockquote',
    blockType: 'blockquote',
    marker: 'delete:blockquote',
    source: '> delete:blockquote',
  },
  {
    name: 'code_block',
    blockType: 'code_block',
    marker: 'delete:code-block',
    source: '```text\ndelete:code-block\ninside:deleted-code-block\n```',
  },
  {
    name: 'code_line',
    blockType: 'code_line',
    marker: 'delete:code-line',
    preserveMarkers: ['keep:code-line'],
    source: '```text\ndelete:code-line\nkeep:code-line\n```',
  },
  {
    name: 'mermaid',
    blockType: 'mermaid',
    marker: 'delete:mermaid',
    source: '```mermaid\ngraph TD\n%% delete:mermaid\nA-->B\n```',
  },
  {
    name: 'table',
    blockType: 'table',
    marker: 'delete:table',
    source: '| delete:table | target |\n| --- | --- |\n| inside | deleted subtree |',
  },
  {
    name: 'tr',
    blockType: 'tr',
    marker: 'delete:tr',
    preserveMarkers: ['keep:tr'],
    source:
      '| tr header a | tr header b |\n| --- | --- |\n| delete:tr | target |\n| keep:tr | sibling |',
  },
  {
    name: 'th',
    blockType: 'th',
    marker: 'delete:th',
    preserveMarkers: ['keep:th', 'body:th:a', 'body:th:b'],
    source: '| delete:th | keep:th |\n| --- | --- |\n| body:th:a | body:th:b |',
  },
  {
    name: 'td',
    blockType: 'td',
    marker: 'delete:td',
    preserveMarkers: ['keep:td', 'header:td:a', 'header:td:b'],
    source: '| header:td:a | header:td:b |\n| --- | --- |\n| delete:td | keep:td |',
  },
  {
    name: 'img',
    blockType: 'img',
    marker: 'delete:img',
    source: '![delete:img](assets/delete-img.png)',
  },
  { name: 'hr', blockType: 'hr', source: '***' },
  {
    name: 'raw_markdown',
    blockType: 'raw_markdown',
    marker: 'delete:raw-markdown',
    source: '<div data-delete="raw-markdown">\ndelete:raw-markdown\n</div>',
  },
  {
    name: 'p_bullet_list',
    blockType: 'p',
    marker: 'delete:p-bullet-list',
    preserveMarkers: ['keep:p-bullet-list'],
    source: '- delete:p-bullet-list\n- keep:p-bullet-list',
  },
  {
    name: 'p_ordered_list',
    blockType: 'p',
    marker: 'delete:p-ordered-list',
    preserveMarkers: ['keep:p-ordered-list'],
    source: '1. delete:p-ordered-list\n2. keep:p-ordered-list',
  },
  {
    name: 'p_task_list',
    blockType: 'p',
    marker: 'delete:p-task-list',
    preserveMarkers: ['keep:p-task-list'],
    source: '- [ ] delete:p-task-list\n- [x] keep:p-task-list',
  },
] as const;

export const BLOCK_DELETE_FIXTURE_MARKDOWN = `${BLOCK_DELETE_CASES.map(
  (fixture) => `${fixture.source}\n\n${guardMarker(fixture)}`
).join('\n\n')}\n`;

export const BLOCK_DELETE_PRESERVED_MARKERS = BLOCK_DELETE_CASES.flatMap((fixture) => [
  guardMarker(fixture),
  ...(fixture.preserveMarkers ?? []),
]);

export function assertBlockDeleteRegistryCoverage(knownBlockTypes: readonly string[]): void {
  const known = new Set(knownBlockTypes);
  const covered = new Set(BLOCK_DELETE_CASES.map((fixture) => fixture.blockType));
  const missing = knownBlockTypes.filter((blockType) => !covered.has(blockType));
  const unknown = [...covered].filter((blockType) => !known.has(blockType));
  const duplicateNames = duplicates(BLOCK_DELETE_CASES.map((fixture) => fixture.name));
  const markers = BLOCK_DELETE_CASES.flatMap((fixture) =>
    fixture.marker ? [fixture.marker] : []
  );
  const duplicateMarkers = duplicates(markers);

  if (
    missing.length > 0 ||
    unknown.length > 0 ||
    duplicateNames.length > 0 ||
    duplicateMarkers.length > 0
  ) {
    throw new Error(
      [
        missing.length > 0 ? `missing block types: ${missing.join(', ')}` : '',
        unknown.length > 0 ? `unknown block types: ${unknown.join(', ')}` : '',
        duplicateNames.length > 0 ? `duplicate case names: ${duplicateNames.join(', ')}` : '',
        duplicateMarkers.length > 0
          ? `duplicate target markers: ${duplicateMarkers.join(', ')}`
          : '',
      ]
        .filter(Boolean)
        .join('; ')
    );
  }
}

export function interleavedBlockDeleteCases(): BlockDeleteConformanceCase[] {
  const ordered: BlockDeleteConformanceCase[] = [];
  let left = 0;
  let right = BLOCK_DELETE_CASES.length - 1;
  while (left <= right) {
    ordered.push(BLOCK_DELETE_CASES[left]);
    left += 1;
    if (left <= right) {
      ordered.push(BLOCK_DELETE_CASES[right]);
      right -= 1;
    }
  }
  return ordered;
}

function guardMarker(fixture: BlockDeleteConformanceCase): string {
  return `keep:after:${fixture.name.replaceAll('_', '-')}`;
}

function duplicates(values: readonly string[]): string[] {
  const seen = new Set<string>();
  const duplicates = new Set<string>();
  for (const value of values) {
    if (seen.has(value)) duplicates.add(value);
    seen.add(value);
  }
  return [...duplicates];
}
